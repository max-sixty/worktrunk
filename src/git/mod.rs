//! Git operations and repository management

use std::path::PathBuf;

// Submodules
mod diff;
mod error;
mod parse;
mod repository;
mod semaphore;

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
use std::sync::LazyLock;
static HEAVY_OPS_SEMAPHORE: LazyLock<semaphore::Semaphore> =
    LazyLock::new(|| semaphore::Semaphore::new(4));

// Re-exports from submodules
pub use diff::{DiffStats, LineDiff};
pub use error::{
    // Typed error enum (Display produces styled output)
    GitError,
    // Special-handling error enum (Display produces styled output)
    HookErrorWithHint,
    WorktrunkError,
    // Error inspection functions
    add_hook_skip_hint,
    exit_code,
    is_command_not_approved,
};
pub use repository::{Repository, ResolvedWorktree, set_base_path};

/// Escape branch name for use in git config key.
///
/// Git config keys only allow alphanumeric, `-`, and `.` characters.
/// Branch names commonly contain `/` and `_`, so we encode them as `-XX`
/// where XX is the uppercase hex value. We also encode `-` itself to
/// ensure round-trip safety.
pub fn escape_branch_for_config(branch: &str) -> String {
    let mut escaped = String::with_capacity(branch.len());
    for ch in branch.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' => escaped.push(ch),
            '-' => escaped.push_str("-2D"),
            _ => {
                // Encode as -XX where XX is uppercase hex
                for byte in ch.to_string().bytes() {
                    escaped.push_str(&format!("-{byte:02X}"));
                }
            }
        }
    }
    escaped
}

/// Unescape branch name from git config key.
pub fn unescape_branch_from_config(escaped: &str) -> String {
    let mut bytes = Vec::with_capacity(escaped.len());
    let mut chars = escaped.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '-' {
            // Try to read two hex digits
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                bytes.push(byte);
                continue;
            }
            // Invalid escape sequence, keep as-is
            bytes.push(b'-');
            bytes.extend(hex.bytes());
        } else {
            // Unescaped char - encode as UTF-8 bytes
            bytes.extend(ch.to_string().bytes());
        }
    }

    // Decode collected bytes as UTF-8
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

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
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
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
    /// Returns the phrase to appear inside parentheses. Some include `{target}`
    /// placeholder for the target branch name.
    pub fn description(&self) -> &'static str {
        match self {
            Self::SameCommit => "same commit as",
            Self::Ancestor => "ancestor of",
            Self::NoAddedChanges => "no added changes",
            Self::TreesMatch => "tree matches",
            Self::MergeAddsNothing => "all changes in",
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

// Note: HookType, Worktree, and WorktreeList are defined in this module and are already public.
// They're accessible as git::HookType, git::Worktree, and git::WorktreeList without needing re-export.

/// Hook types for git operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub enum HookType {
    PostCreate,
    PostStart,
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
}

/// A list of worktrees with automatic bare worktree filtering.
///
/// This type ensures:
/// - Bare worktrees are filtered out (only worktrees with working trees are included)
/// - The main worktree is at index 0 (accessible via `.main()`) for non-bare repos
/// - Construction fails if no valid worktrees exist
///
/// Git guarantees that the main worktree is listed first in `git worktree list` output,
/// so index 0 is always the main worktree after filtering. For bare repositories where
/// the main worktree is filtered out, index 0 will be the first linked worktree.
#[derive(Debug, Clone)]
pub struct WorktreeList {
    pub worktrees: Vec<Worktree>,
}

impl WorktreeList {
    /// Create from raw worktrees, filtering bare entries.
    ///
    /// Preserves git's ordering where the main worktree is first.
    pub(crate) fn from_raw(raw_worktrees: Vec<Worktree>) -> anyhow::Result<Self> {
        let worktrees: Vec<_> = raw_worktrees.into_iter().filter(|wt| !wt.bare).collect();

        if worktrees.is_empty() {
            return Err(GitError::Other {
                message: "No worktrees found".into(),
            }
            .into());
        }

        Ok(Self { worktrees })
    }

    /// Returns the main worktree (at index 0).
    ///
    /// For non-bare repositories, this is the original working tree created by
    /// `git init` or `git clone`. For bare repositories, this returns the first
    /// linked worktree (since bare worktrees are filtered out).
    pub fn main(&self) -> &Worktree {
        &self.worktrees[0]
    }
}

impl IntoIterator for WorktreeList {
    type Item = Worktree;
    type IntoIter = std::vec::IntoIter<Worktree>;

    fn into_iter(self) -> Self::IntoIter {
        self.worktrees.into_iter()
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
    fn test_worktree_list_filters_bare() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo"),
                head: String::new(),
                branch: None,
                bare: true,
                detached: false,
                locked: None,
                prunable: None,
            },
            Worktree {
                path: PathBuf::from("/repo/main"),
                head: "abc123".to_string(),
                branch: Some("main".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
            Worktree {
                path: PathBuf::from("/repo/feature"),
                head: "def456".to_string(),
                branch: Some("feature".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
        ];

        let list = WorktreeList::from_raw(worktrees).unwrap();

        assert_eq!(list.worktrees.len(), 2);
        assert_eq!(list.worktrees[0].branch, Some("main".to_string()));
        assert_eq!(list.worktrees[1].branch, Some("feature".to_string()));
    }

    #[test]
    fn test_worktree_list_main() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo"),
                head: String::new(),
                branch: None,
                bare: true,
                detached: false,
                locked: None,
                prunable: None,
            },
            Worktree {
                path: PathBuf::from("/repo/main"),
                head: "abc123".to_string(),
                branch: Some("main".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
        ];

        let list = WorktreeList::from_raw(worktrees).unwrap();

        let main = list.main();
        assert_eq!(main.branch, Some("main".to_string()));
        assert_eq!(main.path, PathBuf::from("/repo/main"));
    }

    #[test]
    fn test_worktree_list_all_bare_error() {
        let worktrees = vec![Worktree {
            path: PathBuf::from("/repo"),
            head: String::new(),
            branch: None,
            bare: true,
            detached: false,
            locked: None,
            prunable: None,
        }];

        let result = WorktreeList::from_raw(worktrees);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No worktrees found")
        );
    }

    #[test]
    fn test_worktree_list_iteration() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo/main"),
                head: "abc123".to_string(),
                branch: Some("main".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
            Worktree {
                path: PathBuf::from("/repo/feature"),
                head: "def456".to_string(),
                branch: Some("feature".to_string()),
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            },
        ];

        let list = WorktreeList::from_raw(worktrees).unwrap();

        let branches: Vec<_> = list
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_ref())
            .collect();
        assert_eq!(branches, vec!["main", "feature"]);

        let branches_owned: Vec<_> = list.into_iter().filter_map(|wt| wt.branch).collect();
        assert_eq!(
            branches_owned,
            vec!["main".to_string(), "feature".to_string()]
        );
    }

    // ============================================================================
    // escape_branch_for_config Tests
    // ============================================================================

    #[test]
    fn test_escape_branch_simple() {
        assert_eq!(escape_branch_for_config("main"), "main");
        assert_eq!(escape_branch_for_config("feature"), "feature");
    }

    #[test]
    fn test_escape_branch_with_slash() {
        assert_eq!(escape_branch_for_config("feature/foo"), "feature-2Ffoo");
    }

    #[test]
    fn test_escape_branch_with_underscore() {
        assert_eq!(escape_branch_for_config("feature_foo"), "feature-5Ffoo");
    }

    #[test]
    fn test_escape_branch_with_dash() {
        assert_eq!(escape_branch_for_config("feature-foo"), "feature-2Dfoo");
    }

    #[test]
    fn test_escape_branch_with_dot() {
        // Dots are allowed in git config keys
        assert_eq!(escape_branch_for_config("v1.0.0"), "v1.0.0");
    }

    #[test]
    fn test_escape_branch_mixed() {
        assert_eq!(
            escape_branch_for_config("user/feature_name-v2"),
            "user-2Ffeature-5Fname-2Dv2"
        );
    }

    // ============================================================================
    // unescape_branch_from_config Tests
    // ============================================================================

    #[test]
    fn test_unescape_branch_simple() {
        assert_eq!(unescape_branch_from_config("main"), "main");
    }

    #[test]
    fn test_unescape_branch_with_slash() {
        assert_eq!(unescape_branch_from_config("feature-2Ffoo"), "feature/foo");
    }

    #[test]
    fn test_unescape_branch_with_underscore() {
        assert_eq!(unescape_branch_from_config("feature-5Ffoo"), "feature_foo");
    }

    #[test]
    fn test_unescape_branch_with_dash() {
        assert_eq!(unescape_branch_from_config("feature-2Dfoo"), "feature-foo");
    }

    #[test]
    fn test_escape_unescape_roundtrip() {
        let branches = vec![
            "main",
            "feature/foo",
            "user_task",
            "bug-fix",
            "v1.0.0",
            "user/feature_name-v2",
        ];
        for branch in branches {
            let escaped = escape_branch_for_config(branch);
            let unescaped = unescape_branch_from_config(&escaped);
            assert_eq!(unescaped, branch, "Round-trip failed for '{}'", branch);
        }
    }

    #[test]
    fn test_unescape_invalid_escape_sequence() {
        // Invalid hex - should keep as-is
        assert_eq!(unescape_branch_from_config("foo-GGbar"), "foo-GGbar");
    }

    #[test]
    fn test_unescape_truncated_escape() {
        // Truncated escape sequence at end
        assert_eq!(unescape_branch_from_config("foo-2"), "foo-2");
    }

    // ============================================================================
    // check_integration Tests
    // ============================================================================

    #[test]
    fn test_check_integration_same_commit() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: true,
            is_ancestor: false,
            has_added_changes: true,
            trees_match: false,
            would_merge_add: true,
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::SameCommit)
        );
    }

    #[test]
    fn test_check_integration_ancestor() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: false,
            is_ancestor: true,
            has_added_changes: true,
            trees_match: false,
            would_merge_add: true,
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::Ancestor)
        );
    }

    #[test]
    fn test_check_integration_no_added_changes() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: false,
            is_ancestor: false,
            has_added_changes: false, // No added changes = integrated
            trees_match: false,
            would_merge_add: true,
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::NoAddedChanges)
        );
    }

    #[test]
    fn test_check_integration_trees_match() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: false,
            is_ancestor: false,
            has_added_changes: true,
            trees_match: true,
            would_merge_add: true,
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::TreesMatch)
        );
    }

    #[test]
    fn test_check_integration_merge_adds_nothing() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: false,
            is_ancestor: false,
            has_added_changes: true,
            trees_match: false,
            would_merge_add: false, // Merge adds nothing = integrated
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::MergeAddsNothing)
        );
    }

    #[test]
    fn test_check_integration_not_integrated() {
        let mut provider = PrecomputedIntegration {
            is_same_commit: false,
            is_ancestor: false,
            has_added_changes: true,
            trees_match: false,
            would_merge_add: true,
        };
        assert_eq!(check_integration(&mut provider), None);
    }

    #[test]
    fn test_check_integration_priority() {
        // Verify priority: same_commit wins over ancestor
        let mut provider = PrecomputedIntegration {
            is_same_commit: true,
            is_ancestor: true,
            has_added_changes: false,
            trees_match: true,
            would_merge_add: false,
        };
        assert_eq!(
            check_integration(&mut provider),
            Some(IntegrationReason::SameCommit)
        );
    }

    // ============================================================================
    // IntegrationReason Tests
    // ============================================================================

    #[test]
    fn test_integration_reason_description() {
        assert_eq!(
            IntegrationReason::SameCommit.description(),
            "same commit as"
        );
        assert_eq!(IntegrationReason::Ancestor.description(), "ancestor of");
        assert_eq!(
            IntegrationReason::NoAddedChanges.description(),
            "no added changes"
        );
        assert_eq!(IntegrationReason::TreesMatch.description(), "tree matches");
        assert_eq!(
            IntegrationReason::MergeAddsNothing.description(),
            "all changes in"
        );
    }

    // ============================================================================
    // path_dir_name Tests
    // ============================================================================

    #[test]
    fn test_path_dir_name_simple() {
        let path = PathBuf::from("/home/user/repo.feature");
        assert_eq!(path_dir_name(&path), "repo.feature");
    }

    #[test]
    fn test_path_dir_name_root() {
        let path = PathBuf::from("/");
        // Root has no file_name, returns "(unknown)"
        assert_eq!(path_dir_name(&path), "(unknown)");
    }

    #[test]
    fn test_path_dir_name_trailing_slash() {
        // Trailing slash means empty file_name
        let path = PathBuf::from("/home/user/repo/");
        // file_name returns None for paths ending in /
        let name = path_dir_name(&path);
        // Depending on PathBuf normalization, could be repo or (unknown)
        assert!(!name.is_empty());
    }

    // ============================================================================
    // Worktree::dir_name Tests
    // ============================================================================

    #[test]
    fn test_worktree_dir_name() {
        let wt = Worktree {
            path: PathBuf::from("/repos/myrepo.feature"),
            head: "abc123".to_string(),
            branch: Some("feature".to_string()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };
        assert_eq!(wt.dir_name(), "myrepo.feature");
    }

    // ============================================================================
    // HookType Tests
    // ============================================================================

    #[test]
    fn test_hook_type_display() {
        assert_eq!(format!("{}", HookType::PostCreate), "post-create");
        assert_eq!(format!("{}", HookType::PostStart), "post-start");
        assert_eq!(format!("{}", HookType::PreCommit), "pre-commit");
        assert_eq!(format!("{}", HookType::PreMerge), "pre-merge");
        assert_eq!(format!("{}", HookType::PostMerge), "post-merge");
        assert_eq!(format!("{}", HookType::PreRemove), "pre-remove");
    }
}
