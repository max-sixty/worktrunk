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
    WorktrunkError,
    // Error inspection functions
    exit_code,
    is_command_not_approved,
};
pub use repository::{Repository, ResolvedWorktree, set_base_path};

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

/// A list of worktrees with automatic bare worktree filtering.
///
/// This type ensures:
/// - Bare worktrees are filtered out (only worktrees with working trees are included)
/// - The main worktree is always at index 0 (accessible via `.main()`)
/// - Construction fails if no valid worktrees exist
#[derive(Debug, Clone)]
pub struct WorktreeList {
    pub worktrees: Vec<Worktree>,
}

impl WorktreeList {
    /// Create from raw worktrees, filtering bare entries.
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

    /// Returns the main worktree (always at index 0).
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
}
