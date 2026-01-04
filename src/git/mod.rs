//! Git operations and repository management

use std::path::PathBuf;

// Submodules
mod diff;
mod error;
mod parse;
mod repository;
mod url;

#[cfg(test)]
mod test;

// Global semaphore for limiting concurrent heavy git operations
// to reduce mmap thrash on shared commit-graph and pack files.
//
// Permit count of 4 was chosen based on:
// - Typical CPU core counts (4-8 cores common on developer machines)
// - Empirical testing showing 25.6% improvement on 4-worktree repos
// - Balance between parallelism and mmap contention
// - With 4 permits: operations remain fast, overall throughput improves
//
// Heavy operations protected:
// - git rev-list --count (accesses commit-graph via mmap)
// - git diff --numstat (accesses pack files and indexes via mmap)
use crate::sync::Semaphore;
use std::sync::LazyLock;
static HEAVY_OPS_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

// Re-exports from submodules
pub use diff::{DiffStats, LineDiff, parse_numstat_line};
pub use error::{
    // Typed error enum (Display produces styled output)
    GitError,
    // Special-handling error enum (Display produces styled output)
    HookErrorWithHint,
    WorktrunkError,
    // Error inspection functions
    add_hook_skip_hint,
    exit_code,
};
pub use parse::{parse_porcelain_z, parse_untracked_files};
pub use repository::{Repository, ResolvedWorktree, set_base_path};
pub use url::{GitRemoteUrl, parse_owner_repo, parse_remote_host, parse_remote_owner};
/// Why branch content is considered integrated into the target branch.
///
/// Used by both `wt list` (for status symbols) and `wt remove` (for messages).
/// Each variant corresponds to a specific integration check. In `wt list`,
/// three symbols represent these checks:
/// - `_` for [`SameCommit`](Self::SameCommit) with clean working tree (empty)
/// - `–` for [`SameCommit`](Self::SameCommit) with dirty working tree
/// - `⊂` for all others (content integrated via different history)
///
/// The checks are ordered by cost (cheapest first):
/// 1. [`SameCommit`](Self::SameCommit) - commit SHA comparison (~1ms)
/// 2. [`Ancestor`](Self::Ancestor) - ancestor check (~1ms)
/// 3. [`NoAddedChanges`](Self::NoAddedChanges) - three-dot diff (~50-100ms)
/// 4. [`TreesMatch`](Self::TreesMatch) - tree SHA comparison (~100-300ms)
/// 5. [`MergeAddsNothing`](Self::MergeAddsNothing) - merge simulation (~500ms-2s)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum IntegrationReason {
    /// Branch HEAD is literally the same commit as target.
    ///
    /// Used by `wt remove` to determine if branch is safely deletable.
    /// In `wt list`, same-commit state is shown via `MainState::Empty` (`_`) or
    /// `MainState::SameCommit` (`–`) depending on working tree cleanliness.
    SameCommit,

    /// Branch HEAD is an ancestor of target (target has moved past this branch).
    ///
    /// Symbol in `wt list`: `⊂`
    Ancestor,

    /// Three-dot diff (`main...branch`) shows no files.
    /// The branch has no file changes beyond the merge-base.
    ///
    /// Symbol in `wt list`: `⊂`
    NoAddedChanges,

    /// Branch tree SHA equals target tree SHA.
    /// Commit history differs but file contents are identical.
    ///
    /// Symbol in `wt list`: `⊂`
    TreesMatch,

    /// Simulated merge (`git merge-tree`) produces the same tree as target.
    /// The branch has changes, but they're already in target via a different path.
    ///
    /// Symbol in `wt list`: `⊂`
    MergeAddsNothing,
}

impl IntegrationReason {
    /// Human-readable description for use in messages (e.g., `wt remove` output).
    ///
    /// Returns a phrase that expects the target branch name to follow
    /// (e.g., "same commit as" + "main" → "same commit as main").
    pub fn description(&self) -> &'static str {
        match self {
            Self::SameCommit => "same commit as",
            Self::Ancestor => "ancestor of",
            Self::NoAddedChanges => "no added changes on",
            Self::TreesMatch => "tree matches",
            Self::MergeAddsNothing => "all changes in",
        }
    }

    /// Status symbol used in `wt list` for this integration reason.
    ///
    /// - `SameCommit` → `_` (matches `MainState::Empty`)
    /// - Others → `⊂` (matches `MainState::Integrated`)
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::SameCommit => "_",
            _ => "⊂",
        }
    }
}

/// Provider of integration signals for checking if a branch is integrated into target.
///
/// This trait enables short-circuit evaluation: methods are called in priority order,
/// and expensive checks (like `would_merge_add`) are skipped if cheaper checks succeed.
///
/// Implementations:
/// - [`LazyGitIntegration`]: Makes fresh git calls (for `wt remove`)
/// - [`PrecomputedIntegration`]: Uses cached data (for `wt list` progressive rendering)
pub trait IntegrationProvider {
    fn is_same_commit(&mut self) -> bool;
    fn is_ancestor(&mut self) -> bool;
    fn has_added_changes(&mut self) -> bool;
    fn trees_match(&mut self) -> bool;
    fn would_merge_add(&mut self) -> bool;
}

/// Canonical integration check with short-circuit evaluation.
///
/// Checks signals in priority order (cheapest first). Returns as soon as any
/// integration reason is found, avoiding expensive checks when possible.
///
/// This is the single source of truth for integration priority logic,
/// used by both `wt list` and `wt remove`.
pub fn check_integration(provider: &mut impl IntegrationProvider) -> Option<IntegrationReason> {
    // Priority 1 (cheapest): Same commit as target
    if provider.is_same_commit() {
        return Some(IntegrationReason::SameCommit);
    }

    // Priority 2 (cheap): Branch is ancestor of target (target has moved past)
    if provider.is_ancestor() {
        return Some(IntegrationReason::Ancestor);
    }

    // Priority 3: No file changes beyond merge-base (empty three-dot diff)
    if !provider.has_added_changes() {
        return Some(IntegrationReason::NoAddedChanges);
    }

    // Priority 4: Tree SHA matches target (handles squash merge/rebase)
    if provider.trees_match() {
        return Some(IntegrationReason::TreesMatch);
    }

    // Priority 5 (most expensive ~500ms-2s): Merge would not add anything
    if !provider.would_merge_add() {
        return Some(IntegrationReason::MergeAddsNothing);
    }

    None
}

/// Lazy integration provider that makes fresh git calls.
///
/// Used by `wt remove` where short-circuit evaluation matters:
/// expensive checks are skipped if cheaper ones succeed.
pub struct LazyGitIntegration<'a> {
    repo: &'a Repository,
    branch: &'a str,
    target: &'a str,
}

impl<'a> LazyGitIntegration<'a> {
    pub fn new(repo: &'a Repository, branch: &'a str, target: &'a str) -> Self {
        Self {
            repo,
            branch,
            target,
        }
    }
}

impl IntegrationProvider for LazyGitIntegration<'_> {
    fn is_same_commit(&mut self) -> bool {
        self.repo
            .same_commit(self.branch, self.target)
            .unwrap_or(false)
    }

    fn is_ancestor(&mut self) -> bool {
        self.repo
            .is_ancestor(self.branch, self.target)
            .unwrap_or(false)
    }

    fn has_added_changes(&mut self) -> bool {
        self.repo
            .has_added_changes(self.branch, self.target)
            .unwrap_or(true) // Conservative: assume has changes
    }

    fn trees_match(&mut self) -> bool {
        self.repo
            .trees_match(self.branch, self.target)
            .unwrap_or(false)
    }

    fn would_merge_add(&mut self) -> bool {
        self.repo
            .would_merge_add_to_target(self.branch, self.target)
            .unwrap_or(true) // Conservative: assume would add
    }
}

/// Pre-computed integration provider for cached data.
///
/// Used by `wt list` where signals are pre-computed during progressive rendering.
/// Short-circuit doesn't help here since data is already computed.
pub struct PrecomputedIntegration {
    pub is_same_commit: bool,
    pub is_ancestor: bool,
    pub has_added_changes: bool,
    pub trees_match: bool,
    pub would_merge_add: bool,
}

impl IntegrationProvider for PrecomputedIntegration {
    fn is_same_commit(&mut self) -> bool {
        self.is_same_commit
    }
    fn is_ancestor(&mut self) -> bool {
        self.is_ancestor
    }
    fn has_added_changes(&mut self) -> bool {
        self.has_added_changes
    }
    fn trees_match(&mut self) -> bool {
        self.trees_match
    }
    fn would_merge_add(&mut self) -> bool {
        self.would_merge_add
    }
}

/// Category of branch for completion display
#[derive(Debug, Clone, PartialEq)]
pub enum BranchCategory {
    /// Branch has an active worktree
    Worktree,
    /// Local branch without worktree
    Local,
    /// Remote-only branch (includes remote name)
    Remote(String),
}

/// Branch information for shell completions
#[derive(Debug, Clone)]
pub struct CompletionBranch {
    /// Branch name (local name for remotes, e.g., "fix" not "origin/fix")
    pub name: String,
    /// Unix timestamp of last commit
    pub timestamp: i64,
    /// Category for sorting and display
    pub category: BranchCategory,
}

// Re-export parsing helpers for internal use
pub(crate) use parse::DefaultBranchName;

// Note: HookType and Worktree are defined in this module and are already public.
// They're accessible as git::HookType and git::Worktree without needing re-export.

/// Hook types for git operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum HookType {
    PostCreate,
    PostStart,
    PostSwitch,
    PreCommit,
    PreMerge,
    PostMerge,
    PreRemove,
}

/// Worktree information
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

/// Extract the directory name from a path for display purposes.
///
/// Returns the last component of the path as a string, or "(unknown)" if
/// the path has no filename or contains invalid UTF-8.
pub fn path_dir_name(path: &std::path::Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
}

impl Worktree {
    /// Returns the worktree directory name.
    ///
    /// This is the filesystem directory name (e.g., "repo.feature" from "/path/to/repo.feature").
    /// For user-facing display with context (branch consistency, detached state),
    /// use `worktree_display_name()` from the commands module instead.
    pub fn dir_name(&self) -> &str {
        path_dir_name(&self.path)
    }

    /// Find the "home" worktree - the default branch's worktree if it exists,
    /// otherwise the first worktree in the list.
    ///
    /// This is the preferred destination when we need to cd somewhere
    /// (e.g., after removing the current worktree, or after merge removes the worktree).
    ///
    /// For normal repos, `worktrees[0]` is usually the default branch's worktree,
    /// so the fallback rarely matters. For bare repos, there's no semantic "main"
    /// worktree, so preferring the default branch's worktree provides consistency.
    ///
    /// Returns `None` only if `worktrees` is empty. If `default_branch` is empty
    /// or doesn't match any worktree, returns the first worktree.
    pub fn find_home<'a>(worktrees: &'a [Worktree], default_branch: &str) -> Option<&'a Worktree> {
        worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(default_branch))
            .or_else(|| worktrees.first())
    }
}

// Helper functions for worktree parsing
//
// These live in mod.rs rather than parse.rs because they bridge multiple concerns:
// - read_rebase_branch() uses Repository (from repository.rs) to access git internals
// - finalize_worktree() operates on Worktree (defined here in mod.rs)
// - Both are tightly coupled to the Worktree type definition
//
// Placing them here avoids circular dependencies and keeps them close to Worktree.

/// Helper function to read rebase branch information
fn read_rebase_branch(worktree_path: &PathBuf) -> Option<String> {
    // Create a Repository instance to get the correct git directory
    let repo = Repository::at(worktree_path);
    let git_dir = repo.git_dir().ok()?;

    // Check both rebase-merge and rebase-apply
    for rebase_dir in ["rebase-merge", "rebase-apply"] {
        let head_name_path = git_dir.join(rebase_dir).join("head-name");
        if let Ok(content) = std::fs::read_to_string(head_name_path) {
            let branch_ref = content.trim();
            // Strip refs/heads/ prefix if present
            let branch = branch_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(branch_ref)
                .to_string();
            return Some(branch);
        }
    }

    None
}

/// Finalize a worktree after parsing, filling in branch name from rebase state if needed.
pub(crate) fn finalize_worktree(mut wt: Worktree) -> Worktree {
    // If detached but no branch, check if we're rebasing
    if wt.detached
        && wt.branch.is_none()
        && let Some(branch) = read_rebase_branch(&wt.path)
    {
        wt.branch = Some(branch);
    }
    wt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_integration() {
        // Each integration reason + not integrated
        let cases = [
            (
                (true, false, true, false, true),
                Some(IntegrationReason::SameCommit),
            ),
            (
                (false, true, true, false, true),
                Some(IntegrationReason::Ancestor),
            ),
            (
                (false, false, false, false, true),
                Some(IntegrationReason::NoAddedChanges),
            ),
            (
                (false, false, true, true, true),
                Some(IntegrationReason::TreesMatch),
            ),
            (
                (false, false, true, false, false),
                Some(IntegrationReason::MergeAddsNothing),
            ),
            ((false, false, true, false, true), None), // Not integrated
            (
                (true, true, false, true, false),
                Some(IntegrationReason::SameCommit),
            ), // Priority test
        ];
        for ((same, ancestor, added, trees, merge), expected) in cases {
            let mut provider = PrecomputedIntegration {
                is_same_commit: same,
                is_ancestor: ancestor,
                has_added_changes: added,
                trees_match: trees,
                would_merge_add: merge,
            };
            assert_eq!(
                check_integration(&mut provider),
                expected,
                "case: {same},{ancestor},{added},{trees},{merge}"
            );
        }
    }

    #[test]
    fn test_integration_reason_description() {
        assert_eq!(
            IntegrationReason::SameCommit.description(),
            "same commit as"
        );
        assert_eq!(IntegrationReason::Ancestor.description(), "ancestor of");
        assert_eq!(
            IntegrationReason::NoAddedChanges.description(),
            "no added changes on"
        );
        assert_eq!(IntegrationReason::TreesMatch.description(), "tree matches");
        assert_eq!(
            IntegrationReason::MergeAddsNothing.description(),
            "all changes in"
        );
    }

    #[test]
    fn test_path_dir_name() {
        assert_eq!(
            path_dir_name(&PathBuf::from("/home/user/repo.feature")),
            "repo.feature"
        );
        assert_eq!(path_dir_name(&PathBuf::from("/")), "(unknown)");
        assert!(!path_dir_name(&PathBuf::from("/home/user/repo/")).is_empty());

        // Worktree::dir_name
        let wt = Worktree {
            path: PathBuf::from("/repos/myrepo.feature"),
            head: "abc123".into(),
            branch: Some("feature".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };
        assert_eq!(wt.dir_name(), "myrepo.feature");
    }

    #[test]
    fn test_hook_type_display() {
        let cases = [
            (HookType::PostCreate, "post-create"),
            (HookType::PostStart, "post-start"),
            (HookType::PreCommit, "pre-commit"),
            (HookType::PreMerge, "pre-merge"),
            (HookType::PostMerge, "post-merge"),
            (HookType::PreRemove, "pre-remove"),
        ];
        for (hook, expected) in cases {
            assert_eq!(format!("{hook}"), expected);
        }
    }

    #[test]
    fn test_find_home() {
        let make_wt = |branch: Option<&str>| Worktree {
            path: PathBuf::from(format!("/repo.{}", branch.unwrap_or("detached"))),
            head: "abc123".into(),
            branch: branch.map(String::from),
            bare: false,
            detached: branch.is_none(),
            locked: None,
            prunable: None,
        };

        // Empty list returns None
        assert!(Worktree::find_home(&[], "main").is_none());

        // Single worktree on default branch
        let wts = vec![make_wt(Some("main"))];
        assert_eq!(
            Worktree::find_home(&wts, "main").unwrap().path.to_str(),
            Some("/repo.main")
        );

        // Default branch not first - should still find it
        let wts = vec![make_wt(Some("feature")), make_wt(Some("main"))];
        assert_eq!(
            Worktree::find_home(&wts, "main").unwrap().path.to_str(),
            Some("/repo.main")
        );

        // No default branch match - returns first
        let wts = vec![make_wt(Some("feature")), make_wt(Some("bugfix"))];
        assert_eq!(
            Worktree::find_home(&wts, "main").unwrap().path.to_str(),
            Some("/repo.feature")
        );

        // Empty default branch - returns first
        let wts = vec![make_wt(Some("feature"))];
        assert_eq!(
            Worktree::find_home(&wts, "").unwrap().path.to_str(),
            Some("/repo.feature")
        );
    }
}
