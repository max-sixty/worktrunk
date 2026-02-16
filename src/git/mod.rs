//! Git operations and repository management

use std::path::PathBuf;

// Submodules
mod diff;
mod error;
mod parse;
pub mod remote_ref;
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

/// The null OID returned by git when no commits exist (e.g., `git rev-parse HEAD` on an unborn branch).
pub const NULL_OID: &str = "0000000000000000000000000000000000000000";

// Re-exports from submodules
pub(crate) use diff::DiffStats;
pub use diff::{LineDiff, parse_numstat_line};
pub use error::{
    // Structured command failure info
    FailedCommand,
    // Typed error enum (Display produces styled output)
    GitError,
    // Special-handling error enum (Display produces styled output)
    HookErrorWithHint,
    // Platform-specific reference type (PR vs MR)
    RefContext,
    RefType,
    // CLI context for enriching switch suggestions in error hints
    SwitchSuggestionCtx,
    WorktrunkError,
    // Error inspection functions
    add_hook_skip_hint,
    exit_code,
};
pub use parse::{parse_porcelain_z, parse_untracked_files};
pub use repository::{Branch, Repository, ResolvedWorktree, WorkingTree, set_base_path};
pub use url::GitRemoteUrl;
pub use url::{parse_owner_repo, parse_remote_owner};
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

/// Integration signals for checking if a branch is integrated into target.
///
/// `None` means "unknown/failed to check". The check functions treat `None`
/// conservatively (as if not integrated).
///
/// Used by:
/// - `wt list`: Built from parallel task results
/// - `wt remove`/`wt merge`: Built via [`compute_integration_lazy`]
#[derive(Debug, Default)]
pub struct IntegrationSignals {
    pub is_same_commit: Option<bool>,
    pub is_ancestor: Option<bool>,
    pub has_added_changes: Option<bool>,
    pub trees_match: Option<bool>,
    pub would_merge_add: Option<bool>,
}

/// Canonical integration check using pre-computed signals.
///
/// Checks signals in priority order (cheapest first). Returns as soon as any
/// integration reason is found.
///
/// `None` values are treated conservatively: unknown signals don't match.
/// This is the single source of truth for integration priority logic.
pub fn check_integration(signals: &IntegrationSignals) -> Option<IntegrationReason> {
    // Priority 1 (cheapest): Same commit as target
    if signals.is_same_commit == Some(true) {
        return Some(IntegrationReason::SameCommit);
    }

    // Priority 2 (cheap): Branch is ancestor of target (target has moved past)
    if signals.is_ancestor == Some(true) {
        return Some(IntegrationReason::Ancestor);
    }

    // Priority 3: No file changes beyond merge-base (empty three-dot diff)
    if signals.has_added_changes == Some(false) {
        return Some(IntegrationReason::NoAddedChanges);
    }

    // Priority 4: Tree SHA matches target (handles squash merge/rebase)
    if signals.trees_match == Some(true) {
        return Some(IntegrationReason::TreesMatch);
    }

    // Priority 5 (most expensive ~500ms-2s): Merge would not add anything
    if signals.would_merge_add == Some(false) {
        return Some(IntegrationReason::MergeAddsNothing);
    }

    None
}

/// Compute integration signals lazily with short-circuit evaluation.
///
/// Runs git commands in priority order, stopping as soon as integration is
/// confirmed. This avoids expensive checks (like `would_merge_add` which
/// takes ~500ms-2s) when cheaper checks succeed.
///
/// Used by `wt remove` and `wt merge` for single-branch checks.
/// For batch operations, use parallel tasks to build [`IntegrationSignals`] directly.
#[allow(clippy::field_reassign_with_default)] // Intentional: short-circuit populates fields incrementally
pub fn compute_integration_lazy(
    repo: &Repository,
    branch: &str,
    target: &str,
) -> anyhow::Result<IntegrationSignals> {
    let mut signals = IntegrationSignals::default();

    // Priority 1: Same commit
    signals.is_same_commit = Some(repo.same_commit(branch, target)?);
    if signals.is_same_commit == Some(true) {
        return Ok(signals);
    }

    // Priority 2: Ancestor
    signals.is_ancestor = Some(repo.is_ancestor(branch, target)?);
    if signals.is_ancestor == Some(true) {
        return Ok(signals);
    }

    // Priority 3: No added changes
    signals.has_added_changes = Some(repo.has_added_changes(branch, target)?);
    if signals.has_added_changes == Some(false) {
        return Ok(signals);
    }

    // Priority 4: Trees match
    signals.trees_match = Some(repo.trees_match(branch, target)?);
    if signals.trees_match == Some(true) {
        return Ok(signals);
    }

    // Priority 5: Would merge add (most expensive)
    signals.would_merge_add = Some(repo.would_merge_add_to_target(branch, target)?);

    Ok(signals)
}

/// Category of branch for completion display
#[derive(Debug, Clone, PartialEq)]
pub enum BranchCategory {
    /// Branch has an active worktree
    Worktree,
    /// Local branch without worktree
    Local,
    /// Remote-only branch (includes remote names — multiple if same branch on multiple remotes)
    Remote(Vec<String>),
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

use crate::shell_exec::Cmd;

/// Check if a local branch is tracking a specific remote ref.
///
/// Returns `Some(true)` if the branch is configured to track the given ref.
/// Returns `Some(false)` if the branch exists but tracks something else (or nothing).
/// Returns `None` if the branch doesn't exist.
///
/// Used by PR/MR checkout to detect when a branch name collision exists.
///
/// TODO: This only checks `branch.<name>.merge`, not `branch.<name>.remote`. A branch
/// could track the right ref but have the wrong remote configured, which matters for
/// fork PRs/MRs where refs live on the target repo. Consider checking both values.
///
/// # Arguments
/// * `repo_root` - Path to run git commands from
/// * `branch` - Local branch name to check
/// * `expected_ref` - Full ref path (e.g., `refs/pull/101/head` or `refs/merge-requests/42/head`)
pub fn branch_tracks_ref(
    repo_root: &std::path::Path,
    branch: &str,
    expected_ref: &str,
) -> Option<bool> {
    let config_key = format!("branch.{}.merge", branch);
    let output = Cmd::new("git")
        .args(["config", "--get", &config_key])
        .current_dir(repo_root)
        .run()
        .ok()?;

    if !output.status.success() {
        // Config key doesn't exist - branch might not track anything
        // Check if branch exists at all
        let branch_exists = Cmd::new("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", branch),
            ])
            .current_dir(repo_root)
            .run()
            .map(|o| o.status.success())
            .unwrap_or(false);

        return if branch_exists { Some(false) } else { None };
    }

    let merge_ref = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(merge_ref == expected_ref)
}

// Note: HookType and WorktreeInfo are defined in this module and are already public.
// They're accessible as git::HookType and git::WorktreeInfo without needing re-export.

/// Hook types for git operations
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum HookType {
    PostCreate,
    PostStart,
    PostSwitch,
    PreCommit,
    PreMerge,
    PostMerge,
    PreRemove,
    PostRemove,
}

/// Reference to a branch for parallel task execution.
///
/// Works for both worktree items (has path) and branch-only items (no worktree).
/// The `Option<PathBuf>` makes the worktree distinction explicit instead of using
/// empty paths as a sentinel value.
///
/// # Construction
///
/// - From a worktree: `BranchRef::from(&worktree_info)`
/// - For a local branch: `BranchRef::local_branch("feature", "abc123")`
/// - For a remote branch: `BranchRef::remote_branch("origin/feature", "abc123")`
///
/// # Working Tree Access
///
/// For worktree-specific operations, use [`working_tree()`](Self::working_tree)
/// which returns `Some(WorkingTree)` only when this ref has a worktree path.
#[derive(Debug, Clone)]
pub struct BranchRef {
    /// Branch name (e.g., "main", "feature/auth", "origin/feature").
    /// None for detached HEAD.
    pub branch: Option<String>,
    /// Commit SHA this branch/worktree points to.
    pub commit_sha: String,
    /// Path to worktree, if this branch has one.
    /// None for branch-only items (remote branches, local branches without worktrees).
    pub worktree_path: Option<PathBuf>,
    /// True if this is a remote-tracking ref (e.g., "origin/feature").
    /// Remote branches inherently exist on the remote and don't need push config.
    // TODO(full-refs): Consider refactoring to store full refs (e.g., "refs/remotes/origin/feature"
    // or "refs/heads/feature") instead of short names + is_remote flag. Full refs are self-describing
    // and unambiguous, but would require changes throughout the codebase and user input resolution.
    pub is_remote: bool,
}

impl BranchRef {
    /// Create a BranchRef for a local branch without a worktree.
    pub fn local_branch(branch: &str, commit_sha: &str) -> Self {
        Self {
            branch: Some(branch.to_string()),
            commit_sha: commit_sha.to_string(),
            worktree_path: None,
            is_remote: false,
        }
    }

    /// Create a BranchRef for a remote-tracking branch.
    ///
    /// Remote branches (e.g., "origin/feature") are refs under refs/remotes/.
    /// They inherently exist on the remote and don't need upstream tracking config.
    pub fn remote_branch(branch: &str, commit_sha: &str) -> Self {
        Self {
            branch: Some(branch.to_string()),
            commit_sha: commit_sha.to_string(),
            worktree_path: None,
            is_remote: true,
        }
    }

    /// Get a working tree handle for this branch's worktree.
    ///
    /// Returns `Some(WorkingTree)` if this branch has a worktree path,
    /// `None` for branch-only items.
    pub fn working_tree<'a>(&self, repo: &'a Repository) -> Option<WorkingTree<'a>> {
        self.worktree_path
            .as_ref()
            .map(|p| repo.worktree_at(p.clone()))
    }

    /// Returns true if this branch has a worktree.
    pub fn has_worktree(&self) -> bool {
        self.worktree_path.is_some()
    }
}

impl From<&WorktreeInfo> for BranchRef {
    fn from(wt: &WorktreeInfo) -> Self {
        Self {
            branch: wt.branch.clone(),
            commit_sha: wt.head.clone(),
            worktree_path: Some(wt.path.clone()),
            is_remote: false, // Worktrees are always local
        }
    }
}

/// Parsed worktree data from `git worktree list --porcelain`.
///
/// This is a data record containing metadata about a worktree.
/// For running commands in a worktree, use [`WorkingTree`] via
/// [`Repository::worktree_at()`] or [`BranchRef::working_tree()`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WorktreeInfo {
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

impl WorktreeInfo {
    /// Returns true if this worktree is prunable (directory deleted but git still tracks metadata).
    ///
    /// Prunable worktrees cannot be operated on - the directory doesn't exist.
    /// Most iteration over worktrees should skip prunable ones.
    pub fn is_prunable(&self) -> bool {
        self.prunable.is_some()
    }

    /// Returns true if this worktree points to a real commit (not the null OID).
    ///
    /// Unborn branches (no commits yet) have the null OID as their HEAD.
    pub fn has_commits(&self) -> bool {
        self.head != NULL_OID
    }

    /// Returns the worktree directory name.
    ///
    /// This is the filesystem directory name (e.g., "repo.feature" from "/path/to/repo.feature").
    /// For user-facing display with context (branch consistency, detached state),
    /// use `worktree_display_name()` from the commands module instead.
    pub fn dir_name(&self) -> &str {
        path_dir_name(&self.path)
    }
}

// Helper functions for worktree parsing
//
// These live in mod.rs rather than parse.rs because they bridge multiple concerns:
// - read_rebase_branch() uses Repository (from repository.rs) to access git internals
// - finalize_worktree() operates on WorktreeInfo (defined here in mod.rs)
// - Both are tightly coupled to the WorktreeInfo type definition
//
// Placing them here avoids circular dependencies and keeps them close to WorktreeInfo.

/// Helper function to read rebase branch information
fn read_rebase_branch(worktree_path: &PathBuf) -> Option<String> {
    let repo = Repository::current().ok()?;
    let git_dir = repo.worktree_at(worktree_path).git_dir().ok()?;

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
pub(crate) fn finalize_worktree(mut wt: WorktreeInfo) -> WorktreeInfo {
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
        // Tuple: (is_same_commit, is_ancestor, has_added_changes, trees_match, would_merge_add)
        let cases = [
            (
                (Some(true), Some(false), Some(true), Some(false), Some(true)),
                Some(IntegrationReason::SameCommit),
            ),
            (
                (Some(false), Some(true), Some(true), Some(false), Some(true)),
                Some(IntegrationReason::Ancestor),
            ),
            (
                (
                    Some(false),
                    Some(false),
                    Some(false),
                    Some(false),
                    Some(true),
                ),
                Some(IntegrationReason::NoAddedChanges),
            ),
            (
                (Some(false), Some(false), Some(true), Some(true), Some(true)),
                Some(IntegrationReason::TreesMatch),
            ),
            (
                (
                    Some(false),
                    Some(false),
                    Some(true),
                    Some(false),
                    Some(false),
                ),
                Some(IntegrationReason::MergeAddsNothing),
            ),
            (
                (
                    Some(false),
                    Some(false),
                    Some(true),
                    Some(false),
                    Some(true),
                ),
                None,
            ), // Not integrated
            (
                (Some(true), Some(true), Some(false), Some(true), Some(false)),
                Some(IntegrationReason::SameCommit),
            ), // Priority test: is_same_commit wins
            // None values are treated conservatively (as if not integrated)
            ((None, None, None, None, None), None),
            (
                (None, Some(true), Some(false), Some(true), Some(false)),
                Some(IntegrationReason::Ancestor),
            ),
        ];
        for ((same, ancestor, added, trees, merge), expected) in cases {
            let signals = IntegrationSignals {
                is_same_commit: same,
                is_ancestor: ancestor,
                has_added_changes: added,
                trees_match: trees,
                would_merge_add: merge,
            };
            assert_eq!(
                check_integration(&signals),
                expected,
                "case: {same:?},{ancestor:?},{added:?},{trees:?},{merge:?}"
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

        // WorktreeInfo::dir_name
        let wt = WorktreeInfo {
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
        use strum::IntoEnumIterator;

        // Verify all hook types serialize to kebab-case
        for hook in HookType::iter() {
            let display = format!("{hook}");
            assert!(
                display.chars().all(|c| c.is_lowercase() || c == '-'),
                "Hook {hook:?} should be kebab-case, got: {display}"
            );
        }
    }

    #[test]
    fn test_branch_ref_from_worktree_info() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repo.feature"),
            head: "abc123".into(),
            branch: Some("feature".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };

        let branch_ref = BranchRef::from(&wt);

        assert_eq!(branch_ref.branch, Some("feature".to_string()));
        assert_eq!(branch_ref.commit_sha, "abc123");
        assert_eq!(
            branch_ref.worktree_path,
            Some(PathBuf::from("/repo.feature"))
        );
        assert!(branch_ref.has_worktree());
        assert!(!branch_ref.is_remote); // Worktrees are always local
    }

    #[test]
    fn test_branch_ref_local_branch() {
        let branch_ref = BranchRef::local_branch("feature", "abc123");

        assert_eq!(branch_ref.branch, Some("feature".to_string()));
        assert_eq!(branch_ref.commit_sha, "abc123");
        assert_eq!(branch_ref.worktree_path, None);
        assert!(!branch_ref.has_worktree());
        assert!(!branch_ref.is_remote);
    }

    #[test]
    fn test_branch_ref_remote_branch() {
        let branch_ref = BranchRef::remote_branch("origin/feature", "abc123");

        assert_eq!(branch_ref.branch, Some("origin/feature".to_string()));
        assert_eq!(branch_ref.commit_sha, "abc123");
        assert_eq!(branch_ref.worktree_path, None);
        assert!(!branch_ref.has_worktree());
        assert!(branch_ref.is_remote);
    }

    #[test]
    fn test_branch_ref_detached_head() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repo.detached"),
            head: "def456".into(),
            branch: None, // Detached HEAD
            bare: false,
            detached: true,
            locked: None,
            prunable: None,
        };

        let branch_ref = BranchRef::from(&wt);

        assert_eq!(branch_ref.branch, None);
        assert_eq!(branch_ref.commit_sha, "def456");
        assert!(branch_ref.has_worktree());
    }
}
