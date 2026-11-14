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
/// configured upstream no longer exists on the remote), push remote (a remote
/// name or URL; empty if none), upstream remote (empty if none).
const LOCAL_BRANCH_FORMAT: &str = "--format=%(refname:lstrip=2)%00%(objectname)%00%(committerdate:unix)%00%(upstream:short)%00%(upstream:track)%00%(push:remotename)%00%(upstream:remotename)";

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
        // Quiet verification uses status 1 for a missing commit-ish. Other
        // failures, including native interruption, are not evidence of absence.
        self.run_command_check(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ])
    }

    /// Access the local-branch inventory, scanning on first call.
    ///
    /// Returns every local branch (under `refs/heads/`) sorted by committer
    /// timestamp, most recent first. Result is cached for the lifetime of
    /// this `Repository` instance (shared across clones via `Arc`).
    ///
    /// `commit_sha` on each entry is a snapshot at scan time. Code that
    /// needs a current SHA for a ref must resolve through
    /// [`Repository::capture_refs`] at the moment of read, not through this
    /// inventory or a snapshot built from it. The
    /// inventory itself is used for branch listing and upstream-tracking
    /// metadata, stable for the duration of a command that runs no hook.
    /// A hook runs inside the command, so one that touches refs leaves the
    /// rest of the inventory stale too — which branches are in it, and where
    /// each one's upstream points.
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
    /// current SHA must resolve through [`Repository::capture_refs`] at the
    /// moment of the read, not through this list.
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
    /// and the snapshot path. Does not populate the remote-branch cache.
    pub(super) fn scan_remote_branch_records(&self) -> anyhow::Result<Vec<RemoteBranch>> {
        let output = self.run_command(&["for-each-ref", REMOTE_BRANCH_FORMAT, "refs/remotes/"])?;
        let remotes = self.all_remote_urls();
        let mut branches: Vec<RemoteBranch> = output
            .lines()
            .filter_map(|line| parse_remote_branch_line(line, &remotes))
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
    ///
    /// `include_remote_only` is the caller's answer to "could a remote-only
    /// branch reach the user from here?". A completer that can never offer one
    /// (`wt remove`, worktree-only arguments) passes `false` and the
    /// `refs/remotes/` scan — the most expensive of the three, since it reads a
    /// commit object per ref and a fetched-into clone has far more of those
    /// than local branches — never runs. Filtering the results afterwards
    /// instead would pay for the scan and then throw it away.
    pub fn branches_for_completion(
        &self,
        include_remote_only: bool,
    ) -> anyhow::Result<Vec<CompletionBranch>> {
        // The three scans are independent git calls that this function only
        // joins in memory, and completion is a process that does nothing else
        // — run in sequence the user waits for their sum, run concurrently for
        // their max. Best-effort, on the same contract as
        // `Repository::prewarm`: a thread that fails leaves its cache slot
        // empty, and the accessor below re-runs the call and surfaces the
        // error. `RepoCache`'s slots are separate thread-safe `OnceCell`s, so
        // the three fill independently.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = self.list_worktrees();
            });
            scope.spawn(|| {
                let _ = self.local_branches();
            });
            if include_remote_only {
                scope.spawn(|| {
                    let _ = self.remote_branches();
                });
            }
        });

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
        let remotes: &[RemoteBranch] = if include_remote_only {
            self.remote_branches()?
        } else {
            &[]
        };
        for remote in remotes {
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
        // Recency first, then name. The name is not a cosmetic tiebreak: these
        // entries come out of a `HashMap`, whose iteration order varies per
        // process, and `sort_by_key` is stable — so on the timestamps that tie
        // (branches cut from one commit, a bulk import) sorting on recency
        // alone leaves the surviving order randomized, and the completion list
        // reshuffles between one Tab press and the next.
        remote_only.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

        let mut result = Vec::with_capacity(locals.len() + remote_only.len() + worktrees.len());

        // Unborn worktree branches first: a worktree whose branch isn't in
        // `refs/heads/` yet (a fresh `git init`'d repo whose default branch
        // exists only as a `symbolic-ref` target, or a `wt switch --create`
        // off such a repo). Pinned to `i64::MAX` so they sort to the top of
        // the Worktree category — there's no commit to pull a real timestamp
        // from, and the user typically just created them.
        for wt in worktrees {
            if let Some(branch) = &wt.branch
                && !local_names.contains(branch.as_str())
            {
                result.push(CompletionBranch {
                    name: branch.clone(),
                    timestamp: i64::MAX,
                    category: BranchCategory::Worktree,
                });
            }
        }

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
    let push_remote = Some(parts.next()?)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let upstream_remote = Some(parts.next()?)
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_string);
    Some(LocalBranch {
        name,
        commit_sha,
        committer_ts,
        upstream_short,
        upstream_remote,
        push_remote,
    })
}

/// Parse one record from the remote-branch scan.
///
/// Skips `<remote>/HEAD` symrefs, which duplicate another ref and would
/// confuse callers that key by local name.
fn parse_remote_branch_line(line: &str, remotes: &[(String, String)]) -> Option<RemoteBranch> {
    let mut parts = line.split(FIELD_SEP);
    let short_name = parts.next()?;
    let commit_sha = parts.next()?.to_string();
    let committer_ts: i64 = parts.next()?.parse().ok()?;
    let (remote_name, local_name) = super::remotes::split_remote_branch_name(short_name, remotes)?;
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

    /// Build `refs/remotes/origin/<name>` for each name, all at HEAD — so
    /// every one carries the same committer timestamp and the recency sort
    /// between them is a tie.
    fn add_remote_refs_at_head(test: &TestRepo, names: &[&str]) {
        let head = test.git_output(&["rev-parse", "HEAD"]);
        for name in names {
            test.run_git(&["update-ref", &format!("refs/remotes/origin/{name}"), &head]);
        }
    }

    #[test]
    fn branches_for_completion_omits_remote_only_when_not_requested() {
        // `include_remote_only` is what keeps a completer that can never
        // offer a remote-only branch (`wt remove`, worktree-only arguments)
        // from running the `refs/remotes/` scan at all. Filtering afterwards
        // would look the same from here but pay for the scan first, which on
        // a long-lived clone is the most expensive call on the path.
        let test = TestRepo::with_initial_commit();
        add_remote_refs_at_head(&test, &["only-on-remote"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let with = repo.branches_for_completion(true).unwrap();
        assert!(
            with.iter()
                .any(|b| b.name == "only-on-remote"
                    && matches!(b.category, BranchCategory::Remote(_))),
            "requested but absent: {with:?}",
        );

        // A separate Repository, so the first call's cached scan can't stand
        // in for the second's.
        let repo = Repository::at(test.root_path()).unwrap();
        let without = repo.branches_for_completion(false).unwrap();
        assert!(
            !without
                .iter()
                .any(|b| matches!(b.category, BranchCategory::Remote(_))),
            "not requested but present: {without:?}",
        );
    }

    #[test]
    fn branches_for_completion_orders_tied_remote_only_branches_by_name() {
        // Remote-only entries are collected through a `HashMap`, whose
        // iteration order differs per process, and the recency sort is
        // stable — so branches sharing a timestamp keep whatever random order
        // they were iterated in unless the sort breaks the tie itself. Left
        // unbroken, a user's completion list reshuffles between Tab presses.
        let test = TestRepo::with_initial_commit();
        add_remote_refs_at_head(&test, &["cherry", "apple", "banana"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let remote_only: Vec<String> = repo
            .branches_for_completion(true)
            .unwrap()
            .into_iter()
            .filter(|b| matches!(b.category, BranchCategory::Remote(_)))
            .map(|b| b.name)
            .collect();
        assert_eq!(remote_only, vec!["apple", "banana", "cherry"]);
    }

    #[test]
    fn branches_for_completion_includes_unborn_default_branch() {
        // Regression for #3094: on a fresh `git init -b main` with no
        // commits, `refs/heads/` is empty (main exists only as a
        // `symbolic-ref` target), so the local-branch scan returns nothing.
        // The worktree fallback keeps tab-completion responsive by emitting
        // a Worktree-category candidate for each worktree whose branch
        // isn't yet a ref.
        let test = TestRepo::new();
        let repo = Repository::at(test.root_path()).unwrap();

        let branches = repo.branches_for_completion(true).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main"], "completion candidates: {:?}", branches);
        assert!(
            matches!(branches[0].category, BranchCategory::Worktree),
            "category: {:?}",
            branches[0].category,
        );
    }

    #[test]
    fn branches_for_completion_includes_unborn_linked_worktree() {
        // After `git worktree add -b feature ../path` on a still-unborn
        // repo, both `main` and `feature` exist only in `worktree list
        // --porcelain`, not under `refs/heads/`. Both must surface in
        // completion.
        let test = TestRepo::new();
        let feature_path = test.root_path().parent().unwrap().join("feature");
        test.run_git(&[
            "worktree",
            "add",
            "-b",
            "feature",
            feature_path.to_str().unwrap(),
        ]);

        let repo = Repository::at(test.root_path()).unwrap();
        let branches = repo.branches_for_completion(true).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(
            names.contains(&"main") && names.contains(&"feature"),
            "completion candidates: {:?}",
            branches,
        );
        for b in &branches {
            assert!(
                matches!(b.category, BranchCategory::Worktree),
                "non-worktree category for {:?}",
                b,
            );
        }
    }
}
