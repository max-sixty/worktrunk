//! Branch-related operations for Repository.
//!
//! For single-branch operations, see [`super::Branch`].
//! This module contains multi-branch operations (listing, filtering, etc.).
//!
//! # Branch inventory
//!
//! Every multi-branch operation in this file reads from one of two
//! inventories — [`Repository::local_branches`] and
//! [`Repository::remote_branches`]. Each is populated by a single
//! `git for-each-ref` scan that's cached on `RepoCache` for the lifetime of
//! this `Repository` instance (shared across clones via `Arc`):
//!
//! - `refs/heads/` scan fetches name, SHA, committer timestamp, and upstream
//!   tracking info — enough to satisfy every local-branch accessor (name
//!   listing, SHA priming, upstream resolution, completion ordering). The
//!   inventory also carries a name → index map so single-branch lookups
//!   (e.g. [`super::Branch::upstream`]) are O(1) without a separate scan.
//! - `refs/remotes/` scan fetches the same fields for remote-tracking refs.
//!
//! Repeated accessors within a single command share the cached data. This
//! consolidation replaces what used to be five overlapping `for-each-ref`
//! calls (one per accessor) with at most two.
//!
//! Both scans are idempotent: their results depend only on the repository's
//! ref state at the moment of the first call. Branches created mid-command
//! by wt itself (e.g., after `git worktree add -b ...`) will not appear —
//! but no caller needs to observe its own mutations through these accessors.

use std::collections::{HashMap, HashSet};

use super::{BranchCategory, CompletionBranch, LocalBranch, RemoteBranch, Repository};

/// Local-branch inventory: an ordered `Vec<LocalBranch>` plus a `HashMap`
/// for O(1) single-branch lookups.
///
/// Built from [`Repository::scan_local_branch_records`] and stored on
/// `RepoCache` (first scan wins). Iteration order is the scan's own sort —
/// committer timestamp, most recent first.
#[derive(Debug, Default)]
pub(in crate::git) struct LocalBranchInventory {
    entries: Vec<LocalBranch>,
    by_name: HashMap<String, usize>,
}

impl LocalBranchInventory {
    pub(super) fn new(entries: Vec<LocalBranch>) -> Self {
        let by_name = entries
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        Self { entries, by_name }
    }

    pub(super) fn entries(&self) -> &[LocalBranch] {
        &self.entries
    }

    fn get(&self, name: &str) -> Option<&LocalBranch> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }
}

/// Field separator emitted by our `for-each-ref` format strings.
///
/// Use `%00` (git's format escape for a NUL byte) rather than a literal NUL
/// in the Rust string: Rust's `Command::arg` rejects arguments containing
/// interior NUL bytes (they can't survive the `CString` conversion to
/// `execve`), so passing `\0` through `args()` would error before git runs.
const FIELD_SEP: char = '\0';

/// Format string for the local-branch scan.
///
/// Fields, in order: short name, object SHA, committer Unix timestamp,
/// upstream short name (empty if none), upstream track (`[gone]` if the
/// configured upstream no longer exists on the remote).
const LOCAL_BRANCH_FORMAT: &str = "--format=%(refname:lstrip=2)%00%(objectname)%00%(committerdate:unix)%00%(upstream:short)%00%(upstream:track)";

/// Format string for the remote-branch scan.
///
/// Fields, in order: remote-qualified short name (e.g. `origin/feature`),
/// object SHA, committer Unix timestamp.
const REMOTE_BRANCH_FORMAT: &str =
    "--format=%(refname:lstrip=2)%00%(objectname)%00%(committerdate:unix)";

impl Repository {
    /// Check if a git reference exists (branch, tag, commit SHA, HEAD, etc.).
    ///
    /// Accepts any valid commit-ish: branch names, tags, HEAD, commit SHAs,
    /// and relative refs like HEAD~2.
    pub fn ref_exists(&self, reference: &str) -> anyhow::Result<bool> {
        // Use rev-parse to check if the reference resolves to a valid commit
        // The ^{commit} suffix ensures we get the commit object, not a tag
        Ok(self
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", reference),
            ])
            .is_ok())
    }

    /// Access the local-branch inventory, scanning on first call.
    ///
    /// Returns every local branch (under `refs/heads/`) sorted by committer
    /// timestamp, most recent first. Result is cached for the lifetime of
    /// this `Repository` instance (shared across clones via `Arc`).
    ///
    /// `commit_sha` on each entry is a snapshot at scan time. Code that
    /// needs a current SHA for a ref must resolve through a `RefSnapshot`
    /// captured at the moment of read, not through this inventory. The
    /// inventory itself is used for branch listing and upstream-tracking
    /// metadata, both of which are stable for the duration of a command.
    pub fn local_branches(&self) -> anyhow::Result<&[LocalBranch]> {
        Ok(self.local_branch_inventory()?.entries())
    }

    /// O(1) lookup of a single local branch by name.
    ///
    /// Returns `None` if no branch with that exact name exists. First call
    /// triggers the `refs/heads/` scan the same way
    /// [`local_branches`](Self::local_branches) would.
    pub(super) fn local_branch(&self, name: &str) -> anyhow::Result<Option<&LocalBranch>> {
        Ok(self.local_branch_inventory()?.get(name))
    }

    /// Commit SHA the default branch points at, sourced from the local-branch
    /// inventory.
    ///
    /// Returns `None` when the default branch can't be determined (see
    /// [`default_branch`](Self::default_branch)) or when the configured
    /// default branch isn't a local branch (e.g. stale
    /// `worktrunk.default-branch` config pointing at a deleted branch). On
    /// first call, populates the inventory cache via the same single
    /// `for-each-ref refs/heads/` scan that [`local_branches`] would —
    /// no extra subprocess.
    ///
    /// **Snapshot at first scan; do not use in ref-mutating commands.**
    /// Same staleness contract as [`local_branches`]: the SHA is captured
    /// when the inventory is first scanned and never refreshed. Code that
    /// runs after wt itself has updated `refs/heads/<default>` (e.g.
    /// `wt merge`'s `git update-ref`) must capture a fresh
    /// [`crate::git::RefSnapshot`] instead — this accessor will keep
    /// returning the pre-update SHA. Safe in read-only contexts (the
    /// interactive picker, list rendering) where wt itself doesn't move
    /// refs.
    ///
    /// Intended for cache keying: when many parallel tasks all need to
    /// answer "what SHA is the default branch at *right now, for this
    /// command's worth of work*", this lets them share one inventory scan
    /// instead of each forking `git rev-parse <name>` independently.
    ///
    /// [`local_branches`]: Self::local_branches
    pub fn default_branch_sha(&self) -> Option<String> {
        let name = self.default_branch()?;
        self.local_branch(&name)
            .ok()
            .flatten()
            .map(|b| b.commit_sha.clone())
    }

    /// Access the local-branch inventory (entries + name index).
    fn local_branch_inventory(&self) -> anyhow::Result<&LocalBranchInventory> {
        self.cache
            .local_branches
            .get_or_try_init(|| Ok(LocalBranchInventory::new(self.scan_local_branch_records()?)))
    }

    /// Access the remote-tracking branch inventory, scanning on first call.
    ///
    /// Returns every remote-tracking branch (under `refs/remotes/`) sorted
    /// by committer timestamp, most recent first. `<remote>/HEAD` symrefs
    /// are excluded. Result is cached for the lifetime of this `Repository`
    /// instance.
    pub fn remote_branches(&self) -> anyhow::Result<&[RemoteBranch]> {
        self.cache
            .remote_branches
            .get_or_try_init(|| self.scan_remote_branch_records())
            .map(Vec::as_slice)
    }

    /// One `for-each-ref refs/heads/` scan, parsed and sorted by committer
    /// timestamp (most recent first). The shared scan primitive behind both
    /// the cache accessor ([`local_branches`](Self::local_branches), via
    /// [`local_branch_inventory`](Self::local_branch_inventory)) and the
    /// snapshot path ([`crate::git::RefSnapshot`] capture). No cache
    /// side-effect — each caller decides how to store the result
    /// (first-scan-wins cell vs. fresh snapshot).
    ///
    /// `commit_sha` is a snapshot at scan time — callers that need a
    /// current SHA must resolve through a [`crate::git::RefSnapshot`]
    /// captured at the moment of the read, not through this list.
    pub(super) fn scan_local_branch_records(&self) -> anyhow::Result<Vec<LocalBranch>> {
        let output = self.run_command(&["for-each-ref", LOCAL_BRANCH_FORMAT, "refs/heads/"])?;
        let mut branches: Vec<LocalBranch> =
            output.lines().filter_map(parse_local_branch_line).collect();
        branches.sort_by_key(|b| std::cmp::Reverse(b.committer_ts));
        Ok(branches)
    }

    /// One `for-each-ref refs/remotes/` scan, parsed (excluding
    /// `<remote>/HEAD` symrefs) and sorted by committer timestamp. The
    /// shared scan primitive behind both [`remote_branches`](Self::remote_branches)
    /// and the snapshot path. No cache side-effect.
    pub(super) fn scan_remote_branch_records(&self) -> anyhow::Result<Vec<RemoteBranch>> {
        let output = self.run_command(&["for-each-ref", REMOTE_BRANCH_FORMAT, "refs/remotes/"])?;
        let mut branches: Vec<RemoteBranch> = output
            .lines()
            .filter_map(parse_remote_branch_line)
            .collect();
        branches.sort_by_key(|b| std::cmp::Reverse(b.committer_ts));
        Ok(branches)
    }

    /// List all local branch names, sorted by most recent commit first.
    pub fn all_branches(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .local_branches()?
            .iter()
            .map(|b| b.name.clone())
            .collect())
    }

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> anyhow::Result<Vec<String>> {
        let worktrees = self.list_worktrees()?;
        let branches_with_worktrees: HashSet<String> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();
        Ok(self
            .local_branches()?
            .iter()
            .filter(|b| !branches_with_worktrees.contains(&b.name))
            .map(|b| b.name.clone())
            .collect())
    }

    /// Get branches with metadata for shell completions.
    ///
    /// Returns branches in completion order: worktrees first, then local branches,
    /// then remote-only branches. Each category is sorted by recency.
    ///
    /// Searches all remotes (matching git's checkout behavior). If the same branch
    /// exists on multiple remotes, all remote names are included in the result so
    /// completions can show that the branch is ambiguous.
    ///
    /// For remote branches, returns the local name (e.g., "fix" not "origin/fix")
    /// since `git worktree add path fix` auto-creates a tracking branch.
    pub fn branches_for_completion(&self) -> anyhow::Result<Vec<CompletionBranch>> {
        let worktrees = self.list_worktrees()?;
        let worktree_branches: HashSet<String> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        let locals = self.local_branches()?;
        let local_names: HashSet<&str> = locals.iter().map(|b| b.name.as_str()).collect();

        // Group remote branches by local name, collecting all remotes that
        // have each branch. Skip remotes that have a same-named local branch
        // (users should use the local one). Keeps the most recent timestamp
        // across remotes to preserve recency ordering.
        let mut branch_remotes: HashMap<String, (Vec<String>, i64)> = HashMap::new();
        for remote in self.remote_branches()? {
            if local_names.contains(remote.local_name.as_str()) {
                continue;
            }
            branch_remotes
                .entry(remote.local_name.clone())
                .and_modify(|(remotes, ts)| {
                    remotes.push(remote.remote_name.clone());
                    *ts = (*ts).max(remote.committer_ts);
                })
                .or_insert_with(|| (vec![remote.remote_name.clone()], remote.committer_ts));
        }
        let mut remote_only: Vec<(String, Vec<String>, i64)> = branch_remotes
            .into_iter()
            .map(|(name, (mut remotes, ts))| {
                remotes.sort(); // Deterministic remote ordering within each branch
                (name, remotes, ts)
            })
            .collect();
        remote_only.sort_by_key(|b| std::cmp::Reverse(b.2));

        let mut result = Vec::with_capacity(locals.len() + remote_only.len());

        // Worktree branches (already sorted by recency via locals order).
        for branch in locals {
            if worktree_branches.contains(&branch.name) {
                result.push(CompletionBranch {
                    name: branch.name.clone(),
                    timestamp: branch.committer_ts,
                    category: BranchCategory::Worktree,
                });
            }
        }

        // Local branches without worktrees.
        for branch in locals {
            if !worktree_branches.contains(&branch.name) {
                result.push(CompletionBranch {
                    name: branch.name.clone(),
                    timestamp: branch.committer_ts,
                    category: BranchCategory::Local,
                });
            }
        }

        // Remote-only branches.
        for (name, remotes, timestamp) in remote_only {
            result.push(CompletionBranch {
                name,
                timestamp,
                category: BranchCategory::Remote(remotes),
            });
        }

        Ok(result)
    }
}

/// Parse one record from the local-branch scan.
///
/// Returns `None` for malformed lines — e.g. a future git format change or
/// a control character snuck through. Callers skip those entries rather
/// than fail the whole scan.
fn parse_local_branch_line(line: &str) -> Option<LocalBranch> {
    let mut parts = line.split(FIELD_SEP);
    let name = parts.next()?.to_string();
    let commit_sha = parts.next()?.to_string();
    let committer_ts: i64 = parts.next()?.parse().ok()?;
    let upstream_short_raw = parts.next()?;
    let upstream_track = parts.next()?;
    let upstream_short = if upstream_short_raw.is_empty() || upstream_track == "[gone]" {
        None
    } else {
        Some(upstream_short_raw.to_string())
    };
    Some(LocalBranch {
        name,
        commit_sha,
        committer_ts,
        upstream_short,
    })
}

/// Parse one record from the remote-branch scan.
///
/// Skips `<remote>/HEAD` symrefs — they duplicate another ref and would
/// confuse callers that key by local name.
fn parse_remote_branch_line(line: &str) -> Option<RemoteBranch> {
    let mut parts = line.split(FIELD_SEP);
    let short_name = parts.next()?;
    let commit_sha = parts.next()?.to_string();
    let committer_ts: i64 = parts.next()?.parse().ok()?;

    // `<remote>/HEAD` is a symref to the remote's default branch; skip it.
    let (remote_name, local_name) = short_name.split_once('/')?;
    if local_name == "HEAD" {
        return None;
    }

    Some(RemoteBranch {
        short_name: short_name.to_string(),
        commit_sha,
        committer_ts,
        remote_name: remote_name.to_string(),
        local_name: local_name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    #[test]
    fn default_branch_sha_returns_inventory_sha() {
        // The accessor must return the same SHA `git rev-parse <default>`
        // would, sourced from the local-branch inventory rather than its
        // own subprocess.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let expected = test.git_output(&["rev-parse", "main"]);
        assert_eq!(repo.default_branch_sha(), Some(expected));
    }

    #[test]
    fn default_branch_sha_none_when_branch_missing_from_inventory() {
        // Stale `worktrunk.default-branch` config points at a branch that
        // doesn't exist locally — `default_branch()` returns the configured
        // name, but `local_branch(name)` finds nothing, so the accessor
        // returns None. Callers (e.g. the picker's BranchDiff preview)
        // treat None as "fall through to the uncached path."
        let test = TestRepo::with_initial_commit();
        test.run_git(&["config", "worktrunk.default-branch", "ghost"]);

        let repo = Repository::at(test.root_path()).unwrap();
        assert_eq!(repo.default_branch().as_deref(), Some("ghost"));
        assert_eq!(repo.default_branch_sha(), None);
    }

    #[test]
    fn default_branch_sha_is_snapshot_at_first_scan() {
        // Documenting the staleness contract: the inventory is scanned once
        // per `Repository` instance, so a SHA captured before a ref-mutating
        // operation stays put. Callers in mutating commands must capture a
        // fresh `RefSnapshot` instead of trusting this accessor across the
        // mutation.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let before = repo.default_branch_sha().expect("main resolves");

        // Move main forward outside `repo`'s knowledge.
        std::fs::write(test.root_path().join("after.txt"), "after\n").unwrap();
        test.run_git(&["add", "after.txt"]);
        test.run_git(&["commit", "-m", "advance main"]);
        let real_after = test.git_output(&["rev-parse", "main"]);
        assert_ne!(before, real_after, "test setup: main should have moved");

        // Same `repo`: the cached inventory still serves the pre-move SHA.
        assert_eq!(repo.default_branch_sha(), Some(before));

        // A fresh `Repository::at` scans again and sees the new SHA.
        let repo2 = Repository::at(test.root_path()).unwrap();
        assert_eq!(repo2.default_branch_sha(), Some(real_after));
    }
}
