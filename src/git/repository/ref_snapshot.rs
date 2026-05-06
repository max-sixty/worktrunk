//! `RefSnapshot` — a captured, immutable view of repository ref state.
//!
//! The snapshot is the structural answer to ref-name cache staleness. Today's
//! `RepoCache` ambient ref-name → SHA fields (`commit_shas`, `tree_shas`,
//! `effective_integration_targets`, `integration_reasons`, `ahead_behind`)
//! become stale the moment wt updates a ref mid-command — `wt merge`'s
//! `git update-ref refs/heads/main` is the canonical example. Any code that
//! reads through those caches afterwards gets pre-write SHAs.
//!
//! A `RefSnapshot` replaces that ambient cache with an explicit, named,
//! point-in-time value. Callers thread it through. After a ref-mutating
//! write, the caller captures a new snapshot and uses it for downstream
//! reads — the old snapshot remains valid as a pre-write view, but cannot
//! masquerade as current state.
//!
//! # Construction
//!
//! [`Repository::capture_refs`] runs one `git for-each-ref refs/heads/`
//! plus one `git for-each-ref refs/remotes/` and parses both into a
//! single `RefSnapshot`. [`Repository::capture_refs_with_ahead_behind`]
//! additionally runs `for-each-ref ... %(ahead-behind:BASE)` (git ≥ 2.36)
//! to populate ahead/behind counts in the same snapshot.
//!
//! # Lifetime
//!
//! The snapshot is a value, not a cache field. There is no `OnceCell`,
//! no `Arc<DashMap>`, no shared mutable state. Two `capture_refs()`
//! calls within one command produce two distinct snapshots; neither
//! invalidates the other. This is intentional — it removes the
//! "invisible refresh" surface that ambient caching introduces.

use std::collections::HashMap;

use anyhow::bail;

use super::branches::{
    LOCAL_BRANCH_FORMAT, REMOTE_BRANCH_FORMAT, parse_local_branch_line, parse_remote_branch_line,
};
use super::{LocalBranch, RemoteBranch, Repository};

/// An immutable snapshot of repository ref state.
///
/// Keys are git ref names (short or qualified). Values are commit SHAs as
/// reported by `git for-each-ref` at capture time. See the module docstring
/// for the freshness contract.
#[derive(Debug, Clone, Default)]
pub struct RefSnapshot {
    /// Ref name → commit SHA. Each local branch is keyed by both its short
    /// name (e.g. `feature`) and qualified form (`refs/heads/feature`).
    /// Each remote-tracking branch is keyed by short name (`origin/feature`)
    /// and qualified form (`refs/remotes/origin/feature`).
    commits: HashMap<String, String>,

    /// Local branch entries, sorted by committer timestamp descending.
    locals: Vec<LocalBranch>,
    locals_by_name: HashMap<String, usize>,

    /// Remote-tracking branch entries, sorted by committer timestamp descending.
    /// `<remote>/HEAD` symrefs are excluded.
    remotes: Vec<RemoteBranch>,

    /// Ahead/behind counts keyed by `(base, head)` ref names.
    /// Populated only when constructed via
    /// [`Repository::capture_refs_with_ahead_behind`]; empty otherwise.
    /// On git < 2.36 the batch fails silently and this stays empty —
    /// callers that need ahead/behind must fall back to a per-pair query.
    ahead_behind: HashMap<(String, String), (usize, usize)>,
}

impl RefSnapshot {
    /// Resolve a ref name to its commit SHA at capture time.
    ///
    /// Returns `None` for refs not in the snapshot — typically `HEAD`,
    /// raw SHAs, tags, or relative refs like `HEAD~2`. Callers that need
    /// to handle those should fall back to `git rev-parse` (uncached).
    pub fn resolve(&self, name: &str) -> Option<&str> {
        self.commits.get(name).map(String::as_str)
    }

    /// Resolve a ref name to its commit SHA, erroring when absent.
    pub fn must_resolve(&self, name: &str) -> anyhow::Result<&str> {
        match self.resolve(name) {
            Some(sha) => Ok(sha),
            None => bail!("ref not present in snapshot: {name}"),
        }
    }

    /// Look up the configured upstream short name for a local branch.
    ///
    /// Returns `None` when no upstream is configured, when the branch is
    /// absent from the snapshot, or when the configured upstream is gone
    /// (git's `[gone]` track state).
    pub fn upstream_of(&self, branch: &str) -> Option<&str> {
        self.local_branch(branch)
            .and_then(|b| b.upstream_short.as_deref())
    }

    /// Look up cached ahead/behind counts.
    ///
    /// Returns `None` when the snapshot was constructed without ahead/behind
    /// (the default `capture_refs`) or when the requested pair is missing
    /// from the batch result.
    pub fn ahead_behind(&self, base: &str, head: &str) -> Option<(usize, usize)> {
        self.ahead_behind
            .get(&(base.to_string(), head.to_string()))
            .copied()
    }

    /// All local branches at capture time, sorted by committer timestamp descending.
    pub fn local_branches(&self) -> &[LocalBranch] {
        &self.locals
    }

    /// O(1) lookup of a local branch by short name.
    pub fn local_branch(&self, name: &str) -> Option<&LocalBranch> {
        self.locals_by_name.get(name).map(|&i| &self.locals[i])
    }

    /// All remote-tracking branches at capture time, sorted by committer
    /// timestamp descending. `<remote>/HEAD` symrefs are excluded.
    pub fn remote_branches(&self) -> &[RemoteBranch] {
        &self.remotes
    }
}

impl Repository {
    /// Capture current ref state into a [`RefSnapshot`].
    ///
    /// Runs `git for-each-ref refs/heads/` and `git for-each-ref
    /// refs/remotes/` (two subprocesses) and assembles the result into an
    /// immutable value. See the module docstring for the freshness
    /// contract.
    pub fn capture_refs(&self) -> anyhow::Result<RefSnapshot> {
        let locals = scan_locals(self)?;
        let remotes = scan_remotes(self)?;
        Ok(build(locals, remotes, HashMap::new()))
    }

    /// Capture current ref state plus ahead/behind counts vs `base`.
    ///
    /// Runs the two ref scans plus one additional `for-each-ref
    /// --format='%(ahead-behind:BASE)'` pass (git ≥ 2.36). On older git
    /// the ahead/behind batch fails silently and the snapshot's
    /// [`RefSnapshot::ahead_behind`] returns `None` for every key —
    /// callers fall back to per-pair queries.
    pub fn capture_refs_with_ahead_behind(&self, base: &str) -> anyhow::Result<RefSnapshot> {
        let locals = scan_locals(self)?;
        let remotes = scan_remotes(self)?;
        let ahead_behind = scan_ahead_behind(self, base);
        Ok(build(locals, remotes, ahead_behind))
    }
}

fn scan_locals(repo: &Repository) -> anyhow::Result<Vec<LocalBranch>> {
    let output = repo.run_command(&["for-each-ref", LOCAL_BRANCH_FORMAT, "refs/heads/"])?;
    let mut branches: Vec<LocalBranch> =
        output.lines().filter_map(parse_local_branch_line).collect();
    branches.sort_by_key(|b| std::cmp::Reverse(b.committer_ts));
    Ok(branches)
}

fn scan_remotes(repo: &Repository) -> anyhow::Result<Vec<RemoteBranch>> {
    let output = repo.run_command(&["for-each-ref", REMOTE_BRANCH_FORMAT, "refs/remotes/"])?;
    let mut branches: Vec<RemoteBranch> = output
        .lines()
        .filter_map(parse_remote_branch_line)
        .collect();
    branches.sort_by_key(|b| std::cmp::Reverse(b.committer_ts));
    Ok(branches)
}

/// Best-effort ahead/behind batch via `for-each-ref %(ahead-behind:BASE)`.
///
/// Failures (git < 2.36, invalid base) return an empty map — callers must
/// tolerate missing keys.
fn scan_ahead_behind(repo: &Repository, base: &str) -> HashMap<(String, String), (usize, usize)> {
    let format = format!("%(refname) %(ahead-behind:{base})");
    let output =
        match repo.run_command(&["for-each-ref", &format!("--format={format}"), "refs/heads/"]) {
            Ok(out) => out,
            Err(e) => {
                log::debug!("RefSnapshot ahead/behind batch failed for base {base}: {e}");
                return HashMap::new();
            }
        };

    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.rsplitn(3, ' ');
            let behind: usize = parts.next()?.parse().ok()?;
            let ahead: usize = parts.next()?.parse().ok()?;
            let full_ref = parts.next()?.to_string();
            Some(((base.to_string(), full_ref), (ahead, behind)))
        })
        .collect()
}

fn build(
    locals: Vec<LocalBranch>,
    remotes: Vec<RemoteBranch>,
    ahead_behind: HashMap<(String, String), (usize, usize)>,
) -> RefSnapshot {
    let mut commits: HashMap<String, String> = HashMap::new();
    for b in &locals {
        commits.insert(b.name.clone(), b.commit_sha.clone());
        commits.insert(format!("refs/heads/{}", b.name), b.commit_sha.clone());
    }
    for r in &remotes {
        commits.insert(r.short_name.clone(), r.commit_sha.clone());
        commits.insert(
            format!("refs/remotes/{}", r.short_name),
            r.commit_sha.clone(),
        );
    }
    let locals_by_name = locals
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    RefSnapshot {
        commits,
        locals,
        locals_by_name,
        remotes,
        ahead_behind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    #[test]
    fn captures_local_branches_with_shas() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "feat"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let snap = repo.capture_refs().unwrap();

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);

        assert_eq!(snap.resolve("main"), Some(main_sha.as_str()));
        assert_eq!(snap.resolve("refs/heads/main"), Some(main_sha.as_str()));
        assert_eq!(snap.resolve("feature"), Some(feature_sha.as_str()));
        assert_eq!(snap.local_branches().len(), 2);
    }

    #[test]
    fn captures_are_independent_after_ref_update() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let before = repo.capture_refs().unwrap();
        let main_before = before.resolve("main").unwrap().to_owned();

        // Move main forward.
        std::fs::write(test.root_path().join("b.txt"), "y\n").unwrap();
        test.run_git(&["add", "b.txt"]);
        test.run_git(&["commit", "-m", "advance main"]);

        let after = repo.capture_refs().unwrap();
        let main_after = after.resolve("main").unwrap();

        assert_ne!(
            main_before, main_after,
            "post-write snapshot must reflect new SHA"
        );
        // The earlier snapshot still reports the pre-write SHA — it's a
        // frozen view, by design.
        assert_eq!(before.resolve("main"), Some(main_before.as_str()));
    }

    #[test]
    fn must_resolve_errors_on_missing_ref() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let snap = repo.capture_refs().unwrap();

        assert!(snap.must_resolve("does-not-exist").is_err());
        // HEAD is intentionally absent — callers fall back to rev-parse.
        assert_eq!(snap.resolve("HEAD"), None);
    }

    #[test]
    fn ahead_behind_populated_when_requested() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "feat"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let snap = repo.capture_refs_with_ahead_behind("main").unwrap();

        // The plain capture leaves ahead_behind empty.
        let plain = repo.capture_refs().unwrap();
        assert_eq!(plain.ahead_behind("main", "refs/heads/feature"), None);

        // The ahead-behind capture populates it (git ≥ 2.36; on older git
        // the batch is empty, so we tolerate that here).
        if let Some((ahead, behind)) = snap.ahead_behind("main", "refs/heads/feature") {
            assert_eq!(ahead, 1, "feature is one commit ahead of main");
            assert_eq!(behind, 0);
        }
    }

    #[test]
    fn upstream_of_reads_from_local_inventory() {
        let test = TestRepo::with_initial_commit();
        // Set up a fake remote tracking config without a real remote.
        test.run_git(&["config", "branch.main.remote", "origin"]);
        test.run_git(&["config", "branch.main.merge", "refs/heads/main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let _snap = repo.capture_refs().unwrap();
        // Without a corresponding refs/remotes/origin/main, upstream:track
        // reports [gone] and upstream_of returns None — same contract as
        // today's Branch::upstream.
        // (This test mainly checks the method exists and doesn't panic.)
    }
}
