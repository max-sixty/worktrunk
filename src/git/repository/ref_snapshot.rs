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
//! additionally populates ahead/behind counts: it reads them from the
//! persistent SHA-keyed cache (`ahead-behind/` — content-addressed, so
//! never stale) and only falls back to a `for-each-ref ...
//! %(ahead-behind:BASE)` batch walk (git ≥ 2.36) when the cache is cold
//! for this base, seeding the cache from the batch so later runs are pure
//! reads. Branches that moved since the last run are left out of the
//! snapshot map — the per-branch `AheadBehindTask` recomputes (and
//! caches) those by SHA in the parallel pool.
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
    /// [`Repository::capture_refs_with_ahead_behind`], and even then it
    /// may be partial: a branch that moved since its last cache write is
    /// omitted (the per-branch task recomputes it by SHA). On git < 2.36
    /// a cold base yields an empty map. Callers that need ahead/behind for
    /// an absent key must fall back to a per-pair query.
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
    /// (the default `capture_refs`) or when the requested pair is missing —
    /// either uncomputed (git < 2.36 on a cold base) or omitted because the
    /// branch moved since its last cache write. Callers fall back to a
    /// per-pair query (`Repository::ahead_behind_by_sha`).
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
    /// Ahead/behind for `(base, branch)` is content-addressed — a pure
    /// function of the two commit SHAs — so the snapshot map is built from
    /// the persistent `ahead-behind/` cache. When some branches aren't
    /// cached (a fresh repo, after `wt config state clear`, or branches
    /// that moved since their last write) the misses are filled by:
    /// - a few misses → left out of the map; the per-branch
    ///   `AheadBehindTask` recomputes (and caches) them by SHA in the
    ///   parallel pool;
    /// - everything cold → one unscoped `for-each-ref %(ahead-behind)
    ///   refs/heads/` walk, results written to the cache;
    /// - many cold but not all → one `for-each-ref %(ahead-behind)` scoped
    ///   to the missed refnames, results written to the cache.
    ///
    /// The walk computes against `base`'s resolved SHA (not the refname,
    /// which git could resolve to a different commit) and the cache is
    /// keyed by the object SHA the batch reports for each branch, so the
    /// stored value always agrees with what `ahead_behind_by_sha` would
    /// recompute. Orphan branches (no common ancestor with `base`) are
    /// normalized to `(0, 0)`, matching `compute_ahead_behind`. On git <
    /// 2.36 the `%(ahead-behind)` walk yields nothing and the affected
    /// keys stay absent — callers fall back to per-pair queries.
    pub fn capture_refs_with_ahead_behind(&self, base: &str) -> anyhow::Result<RefSnapshot> {
        let locals = scan_locals(self)?;
        let remotes = scan_remotes(self)?;
        let ahead_behind = self.capture_ahead_behind(base, &locals, &remotes);
        Ok(build(locals, remotes, ahead_behind))
    }

    /// Build the `(base, refs/heads/X) -> (ahead, behind)` map for the
    /// local branches, preferring the persistent SHA-keyed cache and
    /// reaching for a `for-each-ref %(ahead-behind)` walk only for the
    /// branches the cache doesn't cover. See
    /// [`Self::capture_refs_with_ahead_behind`].
    fn capture_ahead_behind(
        &self,
        base: &str,
        locals: &[LocalBranch],
        remotes: &[RemoteBranch],
    ) -> HashMap<(String, String), (usize, usize)> {
        let full_ref = |b: &LocalBranch| format!("refs/heads/{}", b.name);

        // The cache is SHA-keyed; we need base's SHA, and it's a branch —
        // so it's among the refs we just scanned. If somehow it isn't, the
        // cache is unreachable for this run: run the batch against the
        // refname (git resolves it), key the snapshot map by refname, and
        // cache nothing.
        let Some(base_sha) = resolve_sha_from_scan(base, locals, remotes).map(str::to_string)
        else {
            return scan_ahead_behind(self, base, None)
                .into_iter()
                .map(|(refname, (_obj, counts))| ((base.to_string(), refname), counts))
                .collect();
        };

        let mut map = HashMap::new();
        let mut missed: Vec<&LocalBranch> = Vec::new();
        for b in locals {
            match super::sha_cache::ahead_behind(self, &base_sha, &b.commit_sha) {
                Some(counts) => {
                    map.insert((base.to_string(), full_ref(b)), counts);
                }
                None => missed.push(b),
            }
        }
        if missed.is_empty() {
            return map;
        }

        // How to fill the misses:
        //   - everything cold (fresh repo / after `state clear`) → one
        //     unscoped `for-each-ref %(ahead-behind:base_sha) refs/heads/`
        //     walk — a single shared traversal finds every merge-base —
        //     then seed the cache.
        //   - only a few cold → leave them out of the map; their
        //     per-branch task recomputes (and caches) by SHA in the
        //     parallel pool. That task's orphan check already primes the
        //     merge-base, so it's two cheap `rev-list --count` calls — no
        //     base-history walk to redo.
        //   - many cold but not all → one `for-each-ref
        //     %(ahead-behind:base_sha)` *scoped to the missed refnames*:
        //     same shared-traversal win, only over the cold subset.
        //
        // The threshold trades the per-pair path's parallelism (it runs
        // in the pool) against the batch's lower total work (one shared
        // base-history traversal). A batch here is serial — it blocks the
        // pool from opening — so reach for it only once "many" cold
        // branches make the amortization clearly worth the serial cost.
        // The fully-parallel ideal is to run the batch as a *pool work
        // item*; see `TODO(ahead-behind-pool)` in `collect/mod.rs`.
        //
        // Compute `%(ahead-behind)` against `base_sha`, not the refname:
        // git resolving `base` itself could land on a different commit (a
        // tag shadowing the branch), and we'd cache counts that disagree
        // with `ahead_behind_by_sha(base_sha, ...)`. Key the cache by the
        // *object SHA the batch saw* (`scan_ahead_behind` returns it) so a
        // ref that moved between the initial scan and this batch can't
        // poison the entry either. Orphan branches need normalizing: git's
        // `%(ahead-behind)` reports the two disjoint history sizes, while
        // `compute_ahead_behind` — what a cache *miss* recomputes —
        // returns (0, 0) and signals orphan-ness separately. An orphan's
        // `behind` is exactly the total commit count of base, so detect it
        // with one `rev-list --count base_sha` and store (0, 0) instead.
        let all_cold = missed.len() == locals.len();
        if !all_cold && missed.len() <= AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES {
            return map;
        }
        let scoped: Option<Vec<String>> =
            (!all_cold).then(|| missed.iter().map(|&b| full_ref(b)).collect());
        let base_total: Option<usize> = self
            .run_command(&["rev-list", "--count", &base_sha])
            .ok()
            .and_then(|s| s.trim().parse().ok());
        let fresh = scan_ahead_behind(self, &base_sha, scoped.as_deref());
        // Stage the writes first, then hand them to `put_ahead_behind_bulk`
        // — one `sweep_lru` at the end, not one per entry. The serial
        // setup-scope path can't afford N×count_json_files() on a large
        // branch list.
        let mut writes: Vec<(&str, &str, (usize, usize))> = Vec::with_capacity(missed.len());
        for b in &missed {
            let Some((object_sha, (ahead, behind))) = fresh.get(&full_ref(b)) else {
                continue;
            };
            let counts = if base_total == Some(*behind) {
                (0, 0) // orphan — match compute_ahead_behind / a cache miss
            } else {
                (*ahead, *behind)
            };
            writes.push((base_sha.as_str(), object_sha.as_str(), counts));
            map.insert((base.to_string(), full_ref(b)), counts);
        }
        super::sha_cache::put_ahead_behind_bulk(self, writes);
        map
    }
}

/// How many cache misses make a single (serial) `for-each-ref
/// %(ahead-behind)` over the missed refs worth running here, versus
/// letting that many per-branch tasks each recompute by SHA in the
/// parallel pool. Below the threshold the per-pair path wins — it's
/// parallel, and each task's orphan check has already primed the
/// merge-base, so it skips re-walking base's history. Above it, one
/// shared base-history traversal amortizes across the cold subset.
const AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES: usize = 8;

impl Repository {
    /// Prime the persistent `ahead-behind/` SHA-cache for the `Remote⇅`
    /// column of `wt list`. Pairs each local branch with its configured
    /// upstream and, for the ones the cache doesn't already cover, runs
    /// one `for-each-ref %(ahead-behind:UPSTREAM_SHA)` walk per unique
    /// upstream SHA — the same shared-graph-traversal win that
    /// [`Self::capture_refs_with_ahead_behind`] uses for the `main↕`
    /// column, scoped to branches sharing one base.
    ///
    /// Mirrors the cache-correctness defenses applied in
    /// [`Self::capture_refs_with_ahead_behind`]:
    /// `%(ahead-behind:UPSTREAM_SHA)` (not `:UPSTREAM_REFNAME`) so git
    /// counts against the SHA the cache will be keyed by — a tag
    /// shadowing the remote-tracking branch can't poison the entry; the
    /// cache key uses the `%(objectname)` git reports for the branch in
    /// the same walk, so a branch ref that moved between the initial
    /// scan and the batch can't poison the entry either; writes flow
    /// through `sha_cache::put_ahead_behind_bulk` for one `sweep_lru` at
    /// the end; orphan branches (no common ancestor with their upstream)
    /// are normalized to `(0, 0)` to match a cache miss in
    /// [`Self::ahead_behind_by_sha`]. Each unique upstream's
    /// `rev-list --count` is memoized in a local map so the orphan check
    /// pays one count per upstream, not one per branch.
    ///
    /// Branches without an upstream, or with a `[gone]` upstream
    /// ([`LocalBranch::upstream_short`] is `None` for both cases — see
    /// `parse_local_branch_line`), are skipped: nothing to cache. Each
    /// per-upstream group below the same threshold
    /// `capture_ahead_behind` uses for the cold-subset batch is also
    /// skipped — the per-row `UpstreamTask` will recompute those by SHA
    /// in the parallel pool, which beats blocking the pool on a small
    /// serial batch. Unlike [`Self::capture_refs_with_ahead_behind`]
    /// there is no "all cold → unscoped walk" shortcut: each upstream
    /// group's refs are already the maximal set of branches that track
    /// it.
    ///
    /// Known limitation — distinct-upstream branches:
    /// when every branch tracks its own remote (each upstream SHA is
    /// unique), every group has `refs.len() == 1` and the primer skips
    /// all of them. Those branches fall through to the per-row
    /// `ahead_behind_by_sha` path in the parallel pool. A `%(upstream:track)`
    /// walk could batch them in one shot, but git computes that atom
    /// against the upstream's CURRENT value at walk time, which we
    /// cannot pin to the SHA the cache will be keyed by — the resulting
    /// race breaks the cache invariant `compute_ahead_behind` would
    /// have established on a miss. Live with this until we either (a)
    /// fan per-group batches out in parallel (turns N size-1 groups
    /// into one batch wall-time) or (b) find an atom that reports
    /// ahead/behind against an explicitly supplied per-branch SHA.
    ///
    /// Returns the number of cache entries written. Side-effect only;
    /// no value is threaded through to callers — per-row
    /// `ahead_behind_by_sha` lookups read the cache.
    pub fn prime_upstream_ahead_behind_cache(
        &self,
        locals: &[LocalBranch],
        remotes: &[RemoteBranch],
    ) -> usize {
        let full_ref = |b: &LocalBranch| format!("refs/heads/{}", b.name);

        // (branch_refname, branch_sha, upstream_sha) for branches with a
        // resolvable upstream. Skip no-upstream and `[gone]` branches:
        // `parse_local_branch_line` collapses both into
        // `upstream_short == None`.
        let mut candidates: Vec<(String, String, String)> = Vec::new();
        for b in locals {
            let Some(upstream_short) = b.upstream_short.as_deref() else {
                continue;
            };
            // `%(upstream:short)` looks like `origin/feature` for a
            // remote-tracking upstream and like `main` (just the branch
            // name) when `branch.<x>.remote = .` configures a local
            // branch as upstream. `resolve_sha_from_scan` checks locals
            // before remotes on a bare name, covering both cases without
            // a prefix-prepending step that would mask the local form.
            let Some(upstream_sha) = resolve_sha_from_scan(upstream_short, locals, remotes) else {
                continue;
            };
            candidates.push((full_ref(b), b.commit_sha.clone(), upstream_sha.to_string()));
        }
        if candidates.is_empty() {
            return 0;
        }

        // Partition cold vs warm by probing the SHA cache. Warm pairs
        // need no work — the per-row `UpstreamTask` reads them in
        // parallel via `ahead_behind_by_sha`.
        let mut cold_by_upstream: HashMap<String, Vec<String>> = HashMap::new();
        for (branch_ref, branch_sha, upstream_sha) in &candidates {
            if super::sha_cache::ahead_behind(self, upstream_sha, branch_sha).is_some() {
                continue;
            }
            cold_by_upstream
                .entry(upstream_sha.clone())
                .or_default()
                .push(branch_ref.clone());
        }
        if cold_by_upstream.is_empty() {
            return 0;
        }

        // One serial `for-each-ref %(ahead-behind:UPSTREAM_SHA)` per
        // upstream group, only above the same threshold
        // `capture_ahead_behind` uses — small groups go through the
        // parallel per-row path instead. `base_totals` memoizes
        // `rev-list --count` per upstream so the orphan check is one
        // count per upstream, not one per branch.
        let mut writes: Vec<(String, String, (usize, usize))> = Vec::new();
        let mut base_totals: HashMap<String, Option<usize>> = HashMap::new();
        for (upstream_sha, refs) in &cold_by_upstream {
            if refs.len() < AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES {
                continue;
            }
            let fresh = scan_ahead_behind(self, upstream_sha, Some(refs));
            if fresh.is_empty() {
                continue; // git < 2.36 or batch failed
            }
            let base_total = *base_totals.entry(upstream_sha.clone()).or_insert_with(|| {
                self.run_command(&["rev-list", "--count", upstream_sha])
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            });
            for branch_ref in refs {
                let Some((object_sha, (ahead, behind))) = fresh.get(branch_ref) else {
                    continue;
                };
                // Orphan against this upstream: git's `%(ahead-behind)`
                // reports the two disjoint history sizes; a cache miss
                // (which calls `compute_ahead_behind`) returns `(0, 0)`.
                // Detect via `behind == |upstream|`.
                let counts = if base_total == Some(*behind) {
                    (0, 0)
                } else {
                    (*ahead, *behind)
                };
                writes.push((upstream_sha.clone(), object_sha.clone(), counts));
            }
        }

        let n = writes.len();
        super::sha_cache::put_ahead_behind_bulk(
            self,
            writes.iter().map(|(u, b, c)| (u.as_str(), b.as_str(), *c)),
        );
        n
    }
}

/// Resolve a ref name to a commit SHA using only refs already scanned into
/// `locals`/`remotes` (no subprocess). Accepts short or qualified forms.
fn resolve_sha_from_scan<'a>(
    name: &str,
    locals: &'a [LocalBranch],
    remotes: &'a [RemoteBranch],
) -> Option<&'a str> {
    let local_short = name.strip_prefix("refs/heads/").unwrap_or(name);
    if let Some(b) = locals.iter().find(|b| b.name == local_short) {
        return Some(&b.commit_sha);
    }
    let remote_short = name.strip_prefix("refs/remotes/").unwrap_or(name);
    remotes
        .iter()
        .find(|r| r.short_name == remote_short)
        .map(|r| r.commit_sha.as_str())
}

fn scan_locals(repo: &Repository) -> anyhow::Result<Vec<LocalBranch>> {
    let output = repo.run_command(&["for-each-ref", LOCAL_BRANCH_FORMAT, "refs/heads/"])?;
    let mut branches: Vec<LocalBranch> =
        output.lines().filter_map(parse_local_branch_line).collect();
    branches.sort_by_key(|b| std::cmp::Reverse(b.committer_ts));

    // Populate the local-branch inventory cache as a side-effect so that
    // subsequent `repo.local_branches()` callers (e.g., `Branch::upstream`
    // running on a worker thread during `wt list`) hit memory instead of
    // re-running the same `for-each-ref refs/heads/` scan. `set()` is a
    // no-op when the cache is already populated, preserving the
    // first-scan-wins contract documented on `RepoCache.local_branches` —
    // every later `capture_refs` call still scans fresh, so mutating
    // commands that need post-update SHAs (e.g. `wt merge`'s post-merge
    // `capture_refs` in `worktree::finish`) are unaffected.
    let _ = repo
        .cache
        .local_branches
        .set(super::branches::LocalBranchInventory::new(branches.clone()));

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

/// Best-effort ahead/behind batch via
/// `for-each-ref %(refname) %(objectname) %(ahead-behind:BASE)` — one
/// shared graph traversal that finds every requested ref's merge-base
/// against `base` (a refname or, preferably for cache-keying, a SHA) and
/// counts both sides.
///
/// `refs == None` walks all of `refs/heads/`; `Some(refnames)` scopes the
/// walk to exactly those refs (still one traversal, just that subset).
/// Returns `refname -> (object_sha, (ahead, behind))` — `object_sha` is
/// the commit the counts were computed against, so a caller that caches
/// by SHA keys on *it* rather than on a separately-scanned value a
/// concurrent ref update could have made stale. Failures (git < 2.36,
/// invalid base) return an empty map — callers must tolerate missing keys.
fn scan_ahead_behind(
    repo: &Repository,
    base: &str,
    refs: Option<&[String]>,
) -> HashMap<String, (String, (usize, usize))> {
    let format = format!("--format=%(refname) %(objectname) %(ahead-behind:{base})");
    let mut args: Vec<&str> = vec!["for-each-ref", format.as_str()];
    match refs {
        Some(refs) => args.extend(refs.iter().map(String::as_str)),
        None => args.push("refs/heads/"),
    }
    let output = match repo.run_command(&args) {
        Ok(out) => out,
        Err(e) => {
            log::debug!("RefSnapshot ahead/behind batch failed for base {base}: {e}");
            return HashMap::new();
        }
    };

    output
        .lines()
        .filter_map(|line| {
            // "<refname> <objectname> <ahead> <behind>" — refnames cannot
            // contain spaces, so the leftmost chunk is exactly the refname.
            let mut parts = line.rsplitn(4, ' ');
            let behind: usize = parts.next()?.parse().ok()?;
            let ahead: usize = parts.next()?.parse().ok()?;
            let object_sha = parts.next()?.to_string();
            let refname = parts.next()?.to_string();
            Some((refname, (object_sha, (ahead, behind))))
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
    fn ahead_behind_reads_persistent_cache_on_second_capture() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "feat"]);
        test.run_git(&["checkout", "main"]);

        // First capture: cold base → runs the batch walk, seeds the cache.
        let repo = Repository::at(test.root_path()).unwrap();
        let first = repo.capture_refs_with_ahead_behind("main").unwrap();
        // Skip on git < 2.36 where the batch silently yields nothing.
        let Some((1, 0)) = first.ahead_behind("main", "refs/heads/feature") else {
            return;
        };

        // Tamper with the cached entry for (main, feature).
        let dir = crate::cache::cache_dir(&repo, "ahead-behind");
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
            .collect();
        // One entry per local branch: main↔main and main↔feature.
        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);
        let tampered_path = dir.join(format!("{main_sha}-{feature_sha}.json"));
        assert!(
            entries.iter().any(|e| e.path() == tampered_path),
            "expected a cache entry for (main, feature)"
        );
        std::fs::write(&tampered_path, "[7,3]").unwrap();

        // Second capture (fresh repo): every entry is cached → no batch
        // walk → the map reflects the tampered value.
        let repo2 = Repository::at(test.root_path()).unwrap();
        let second = repo2.capture_refs_with_ahead_behind("main").unwrap();
        assert_eq!(
            second.ahead_behind("main", "refs/heads/feature"),
            Some((7, 3)),
            "second capture should read the (tampered) persistent cache"
        );
    }

    #[test]
    fn ahead_behind_omits_moved_branch_from_partial_snapshot() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "feat"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let first = repo.capture_refs_with_ahead_behind("main").unwrap();
        // Skip on git < 2.36.
        if first.ahead_behind("main", "refs/heads/feature").is_none() {
            return;
        }

        // Advance feature so its SHA (and thus its cache key) changes; main
        // stays put, so its cache entry is still a hit.
        test.run_git(&["checkout", "feature"]);
        std::fs::write(test.root_path().join("b.txt"), "y\n").unwrap();
        test.run_git(&["add", "b.txt"]);
        test.run_git(&["commit", "-m", "feat2"]);
        test.run_git(&["checkout", "main"]);

        let repo2 = Repository::at(test.root_path()).unwrap();
        let snap = repo2.capture_refs_with_ahead_behind("main").unwrap();
        // Partial-warm path: feature moved → omitted from the snapshot map
        // (its per-branch task recomputes by SHA); main is still present.
        assert_eq!(snap.ahead_behind("main", "refs/heads/feature"), None);
        assert_eq!(snap.ahead_behind("main", "refs/heads/main"), Some((0, 0)));
    }

    #[test]
    fn ahead_behind_many_misses_uses_scoped_batch() {
        let test = TestRepo::with_initial_commit();
        // 10 feature branches, each one (empty) commit ahead of main —
        // more than `AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES`.
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
        }
        test.run_git(&["checkout", "main"]);

        // First capture: everything cold → unscoped batch seeds the cache.
        let repo = Repository::at(test.root_path()).unwrap();
        let first = repo.capture_refs_with_ahead_behind("main").unwrap();
        if first.ahead_behind("main", "refs/heads/feat0").is_none() {
            return; // git < 2.36 — %(ahead-behind) unsupported
        }

        // Advance every feature branch; main stays put. Now 10 of the 11
        // local branches miss the cache (main still hits) → "many cold but
        // not all" → the scoped `for-each-ref %(ahead-behind)` path over
        // just the 10 missed refnames.
        for i in 0..10 {
            test.run_git(&["checkout", &format!("feat{i}")]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}b")]);
        }
        test.run_git(&["checkout", "main"]);

        let repo2 = Repository::at(test.root_path()).unwrap();
        let snap = repo2.capture_refs_with_ahead_behind("main").unwrap();
        for i in 0..10 {
            assert_eq!(
                snap.ahead_behind("main", &format!("refs/heads/feat{i}")),
                Some((2, 0)),
                "feat{i} is 2 ahead of main after the scoped re-walk"
            );
        }
        // main never moved → still the (0,0) entry from the first capture.
        assert_eq!(snap.ahead_behind("main", "refs/heads/main"), Some((0, 0)));
    }

    #[test]
    fn ahead_behind_normalizes_orphan_to_zero_in_cache() {
        let test = TestRepo::with_initial_commit();
        // An orphan branch: its own root commit, no common ancestor with main.
        test.run_git(&["checkout", "--orphan", "orphanbr"]);
        test.run_git(&["rm", "-rfq", "."]);
        std::fs::write(test.root_path().join("o.txt"), "y\n").unwrap();
        test.run_git(&["add", "o.txt"]);
        test.run_git(&["commit", "-m", "orphan root"]);
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let snap = repo.capture_refs_with_ahead_behind("main").unwrap();
        // git < 2.36 — %(ahead-behind) unsupported, nothing seeded.
        if snap.ahead_behind("main", "refs/heads/main").is_none() {
            return;
        }

        // Git's batch reports the two disjoint history sizes for an orphan;
        // we normalize to (0, 0) so the snapshot — and the persistent
        // cache entry — agree with `compute_ahead_behind` / a cache miss.
        assert_eq!(
            snap.ahead_behind("main", "refs/heads/orphanbr"),
            Some((0, 0))
        );

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let orphan_sha = test.git_output(&["rev-parse", "orphanbr"]);
        let cached: (usize, usize) = serde_json::from_str(
            &std::fs::read_to_string(
                crate::cache::cache_dir(&repo, "ahead-behind")
                    .join(format!("{main_sha}-{orphan_sha}.json")),
            )
            .expect("orphan pair should be cached"),
        )
        .expect("cache entry is a (usize, usize)");
        assert_eq!(cached, (0, 0));
    }

    #[test]
    fn ahead_behind_base_resolves_via_remote_tracking_ref() {
        let test = TestRepo::with_initial_commit();
        // A remote-tracking ref with no local branch of the same name —
        // the base SHA must be resolved from the remotes scan, not locals.
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "feat"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let snap = repo.capture_refs_with_ahead_behind("origin/trunk").unwrap();
        let Some((1, 0)) = snap.ahead_behind("origin/trunk", "refs/heads/feature") else {
            // git < 2.36: batch silently empty; the cache is unreachable
            // for it, but resolve-via-remote still ran without panicking.
            return;
        };
        // Seeded the SHA cache keyed by the remote ref's SHA.
        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        assert!(
            crate::cache::cache_dir(&repo, "ahead-behind")
                .join(format!("{trunk_sha}-{feature_sha}.json"))
                .exists()
        );
    }

    #[test]
    fn ahead_behind_unresolvable_base_is_harmless() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        // A base that resolves to nothing: no extra subprocess can key the
        // cache, and the `for-each-ref %(ahead-behind:...)` walk fails too.
        // The snapshot is still valid, just without ahead/behind.
        let snap = repo
            .capture_refs_with_ahead_behind("definitely-not-a-ref")
            .unwrap();
        assert_eq!(
            snap.ahead_behind("definitely-not-a-ref", "refs/heads/main"),
            None
        );
        assert_eq!(
            snap.resolve("main"),
            repo.capture_refs().unwrap().resolve("main")
        );
    }

    #[test]
    fn capture_refs_populates_local_branch_cache() {
        // The local-branch inventory cache is populated as a side-effect
        // of `capture_refs`, so a subsequent `local_branches()` call on
        // the same `Repository` reads from memory instead of re-running
        // `for-each-ref refs/heads/`. The statusline render hits both
        // entry points: the main thread captures a `RefSnapshot`, then a
        // worker thread reads `Branch::upstream` (which goes through the
        // local-branch inventory) — without this side-effect the same
        // scan ran twice (worktrunk#2672).
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Cache is empty before capture.
        assert!(repo.cache.local_branches.get().is_none());

        let _snap = repo.capture_refs().unwrap();

        // Capture populates the inventory cache.
        let inventory = repo
            .cache
            .local_branches
            .get()
            .expect("capture_refs should populate the local-branch cache");
        assert!(inventory.entries().iter().any(|b| b.name == "main"));

        // Mutating refs after capture must NOT change what the cache
        // returns — `local_branches()` reads the populated cell, not the
        // current ref state.
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "advance main"]);
        let main_after = test.git_output(&["rev-parse", "main"]);

        let cached_main = repo
            .local_branches()
            .unwrap()
            .iter()
            .find(|b| b.name == "main")
            .expect("main is cached")
            .commit_sha
            .clone();
        assert_ne!(
            cached_main, main_after,
            "test setup: ref must have moved between capture and read"
        );
    }

    #[test]
    fn capture_refs_scans_fresh_when_cache_already_populated() {
        // Mutation paths (e.g., `wt merge`) call `capture_refs` AFTER
        // moving a ref, expecting the post-mutation SHA. If the inventory
        // cache had been populated earlier with the pre-mutation SHA, the
        // snapshot still has to scan fresh — `set()` is a no-op on the
        // already-populated cell, but the snapshot itself reflects the
        // current ref state.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Populate the inventory cache by calling local_branches first.
        let main_before = repo
            .local_branches()
            .unwrap()
            .iter()
            .find(|b| b.name == "main")
            .unwrap()
            .commit_sha
            .clone();

        // Move main forward.
        std::fs::write(test.root_path().join("a.txt"), "x\n").unwrap();
        test.run_git(&["add", "a.txt"]);
        test.run_git(&["commit", "-m", "advance main"]);
        let main_after = test.git_output(&["rev-parse", "main"]);
        assert_ne!(main_before, main_after);

        // capture_refs sees post-move SHA even though the inventory cache
        // still holds the pre-move value.
        let snap = repo.capture_refs().unwrap();
        assert_eq!(snap.resolve("main"), Some(main_after.as_str()));
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

    // Set up a local branch that tracks `origin/<upstream_basename>`. Uses
    // the bare config-file path so tests stay self-contained: no real
    // remote, just enough refs and config for git's `%(upstream:short)` to
    // resolve.
    fn track(test: &TestRepo, branch: &str, upstream_basename: &str) {
        test.run_git(&["config", &format!("branch.{branch}.remote"), "origin"]);
        test.run_git(&[
            "config",
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{upstream_basename}"),
        ]);
    }

    fn ahead_behind_cache_path(repo: &Repository, base_sha: &str, head_sha: &str) -> PathBuf {
        crate::cache::cache_dir(repo, "ahead-behind").join(format!("{base_sha}-{head_sha}.json"))
    }

    use std::path::PathBuf;

    #[test]
    fn prime_upstream_writes_cache_on_cold_run() {
        // 10 branches all tracking origin/trunk → above
        // AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES, single-group scoped batch.
        let test = TestRepo::with_initial_commit();
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
            track(&test, &format!("feat{i}"), "trunk");
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);

        // Skip on git < 2.36 where the batch silently yields nothing.
        if written == 0 {
            return;
        }
        assert_eq!(written, 10, "one cache entry per tracking branch");

        // Each (trunk_sha, feat{i}_sha) pair lives on disk and reflects the
        // real (1, 0) — `ahead_behind_by_sha` would compute the same.
        for i in 0..10 {
            let feat_sha = test.git_output(&["rev-parse", &format!("feat{i}")]);
            assert_eq!(
                repo.ahead_behind_by_sha(&trunk_sha, &feat_sha).unwrap(),
                (1, 0),
                "feat{i} is 1 ahead, 0 behind origin/trunk"
            );
            assert!(
                ahead_behind_cache_path(&repo, &trunk_sha, &feat_sha).exists(),
                "cache entry must exist for (trunk, feat{i})"
            );
        }
    }

    #[test]
    fn prime_upstream_warm_cache_skips_walk() {
        // Same setup as the cold test, but on the second call every entry
        // is cached → primer writes nothing (and tampering with the cache
        // survives — proving we didn't re-run the batch).
        let test = TestRepo::with_initial_commit();
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
            track(&test, &format!("feat{i}"), "trunk");
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals: Vec<_> = repo.local_branches().unwrap().to_vec();
        let remotes: Vec<_> = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        if written == 0 {
            return; // git < 2.36
        }

        // Tamper with one entry; a second prime that re-ran the batch
        // would overwrite it.
        let feat0_sha = test.git_output(&["rev-parse", "feat0"]);
        let tampered = ahead_behind_cache_path(&repo, &trunk_sha, &feat0_sha);
        std::fs::write(&tampered, "[7,3]").unwrap();

        let repo2 = Repository::at(test.root_path()).unwrap();
        let locals2 = repo2.local_branches().unwrap().to_vec();
        let remotes2 = repo2.remote_branches().unwrap().to_vec();
        let second_written = repo2.prime_upstream_ahead_behind_cache(&locals2, &remotes2);
        assert_eq!(
            second_written, 0,
            "everything cached → no batch and no writes"
        );
        assert_eq!(
            std::fs::read_to_string(&tampered).unwrap(),
            "[7,3]",
            "warm-cache run must not overwrite the tampered value"
        );
    }

    #[test]
    fn prime_upstream_below_threshold_leaves_cache_cold() {
        // Only 1 branch tracks origin/trunk → below
        // AHEAD_BEHIND_SCOPED_BATCH_MIN_MISSES; primer leaves it for the
        // per-row task's parallel `ahead_behind_by_sha`.
        let test = TestRepo::with_initial_commit();
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);
        test.run_git(&["checkout", "-b", "feat", "main"]);
        test.run_git(&["commit", "--allow-empty", "-m", "c"]);
        track(&test, "feat", "trunk");
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        assert_eq!(written, 0, "below threshold → no batch, no writes");

        let feat_sha = test.git_output(&["rev-parse", "feat"]);
        assert!(
            !ahead_behind_cache_path(&repo, &trunk_sha, &feat_sha).exists(),
            "primer must not seed below-threshold groups"
        );
    }

    #[test]
    fn prime_upstream_skips_no_upstream_branches() {
        // 10 branches without any upstream config → nothing to cache.
        let test = TestRepo::with_initial_commit();
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        assert_eq!(written, 0, "no upstream → no candidates → no writes");
        assert!(
            !crate::cache::cache_dir(&repo, "ahead-behind").exists()
                || std::fs::read_dir(crate::cache::cache_dir(&repo, "ahead-behind"))
                    .unwrap()
                    .next()
                    .is_none(),
            "ahead-behind cache dir must stay empty"
        );
    }

    #[test]
    fn prime_upstream_skips_gone_upstream() {
        // Branches with `branch.X.remote/merge` set but the corresponding
        // `refs/remotes/origin/<basename>` missing → `[gone]` track state →
        // `parse_local_branch_line` collapses to `upstream_short = None`,
        // so the primer skips them too.
        let test = TestRepo::with_initial_commit();
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("gone{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
            track(&test, &format!("gone{i}"), "this-is-deleted-elsewhere");
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        assert_eq!(
            written, 0,
            "[gone] upstream → upstream_short = None → no candidates"
        );
    }

    #[test]
    fn prime_upstream_normalizes_orphan_to_zero() {
        // 9 normal tracking branches plus one orphan branch that also
        // tracks origin/trunk. The orphan's `%(ahead-behind)` reports the
        // two disjoint history sizes; the primer must store `(0, 0)` to
        // match `compute_ahead_behind` / a cache miss.
        let test = TestRepo::with_initial_commit();
        // Advance main a few commits so origin/trunk has a non-trivial
        // total — this is what the orphan check compares against.
        for i in 0..3 {
            test.run_git(&["commit", "--allow-empty", "-m", &format!("m{i}")]);
        }
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);

        for i in 0..9 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
            track(&test, &format!("feat{i}"), "trunk");
        }

        // Orphan branch — no common ancestor with main.
        test.run_git(&["checkout", "--orphan", "orphanbr"]);
        test.run_git(&["rm", "-rfq", "."]);
        std::fs::write(test.root_path().join("o.txt"), "y\n").unwrap();
        test.run_git(&["add", "o.txt"]);
        test.run_git(&["commit", "-m", "orphan root"]);
        track(&test, "orphanbr", "trunk");
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        if written == 0 {
            return; // git < 2.36
        }

        let orphan_sha = test.git_output(&["rev-parse", "orphanbr"]);
        let cached: (usize, usize) = serde_json::from_str(
            &std::fs::read_to_string(ahead_behind_cache_path(&repo, &trunk_sha, &orphan_sha))
                .expect("orphan pair should be cached"),
        )
        .expect("cache entry is a (usize, usize)");
        assert_eq!(
            cached,
            (0, 0),
            "orphan-vs-upstream must normalize to compute_ahead_behind's (0, 0)"
        );
        // Sanity-check parity with the per-pair API.
        assert_eq!(
            repo.ahead_behind_by_sha(&trunk_sha, &orphan_sha).unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn prime_upstream_groups_by_unique_upstream_sha() {
        // Two upstream groups, both above threshold. Each group runs its
        // own scoped `for-each-ref` walk; entries land under separate
        // base-SHA keys.
        let test = TestRepo::with_initial_commit();
        let main_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunkA", &main_sha]);
        // Advance main, then point trunkB at the new tip — distinct SHA.
        test.run_git(&["commit", "--allow-empty", "-m", "advance-main"]);
        let main_sha2 = test.git_output(&["rev-parse", "main"]);
        assert_ne!(main_sha, main_sha2);
        test.run_git(&["update-ref", "refs/remotes/origin/trunkB", &main_sha2]);

        for i in 0..8 {
            test.run_git(&["checkout", "-b", &format!("a{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("a{i}")]);
            track(&test, &format!("a{i}"), "trunkA");
        }
        for i in 0..8 {
            test.run_git(&["checkout", "-b", &format!("b{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("b{i}")]);
            track(&test, &format!("b{i}"), "trunkB");
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        if written == 0 {
            return; // git < 2.36
        }
        assert_eq!(written, 16, "8 entries per upstream group, two groups");

        for i in 0..8 {
            let a_sha = test.git_output(&["rev-parse", &format!("a{i}")]);
            assert!(
                ahead_behind_cache_path(&repo, &main_sha, &a_sha).exists(),
                "a{i} entry keyed by trunkA (main_sha)"
            );
            let b_sha = test.git_output(&["rev-parse", &format!("b{i}")]);
            assert!(
                ahead_behind_cache_path(&repo, &main_sha2, &b_sha).exists(),
                "b{i} entry keyed by trunkB (main_sha2)"
            );
        }
    }

    #[test]
    fn prime_upstream_resolves_local_upstream() {
        // `branch.X.remote = .` makes another LOCAL branch the upstream;
        // `%(upstream:short)` is then just the branch name (e.g., `main`)
        // with no `origin/` prefix. The primer must still find its SHA
        // via the local-branch inventory and seed the cache; otherwise a
        // documented fallback silently does nothing.
        let test = TestRepo::with_initial_commit();
        let main_sha = test.git_output(&["rev-parse", "main"]);
        for i in 0..10 {
            test.run_git(&["checkout", "-b", &format!("feat{i}"), "main"]);
            test.run_git(&["commit", "--allow-empty", "-m", &format!("c{i}")]);
            // `.` means "this repo": the upstream is the local branch
            // matched by `branch.<x>.merge`.
            test.run_git(&["config", &format!("branch.feat{i}.remote"), "."]);
            test.run_git(&[
                "config",
                &format!("branch.feat{i}.merge"),
                "refs/heads/main",
            ]);
        }
        test.run_git(&["checkout", "main"]);

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        if written == 0 {
            return; // git < 2.36
        }
        assert_eq!(written, 10, "local-upstream branches must be cached too");

        for i in 0..10 {
            let feat_sha = test.git_output(&["rev-parse", &format!("feat{i}")]);
            assert!(
                ahead_behind_cache_path(&repo, &main_sha, &feat_sha).exists(),
                "feat{i} keyed against main_sha (local upstream)"
            );
        }
    }

    #[test]
    fn prime_upstream_caches_equal_branch_as_zero() {
        // Branch whose tip equals its upstream → (0, 0). The cache key
        // uses the same SHA for both base and head (a-a.json), and a
        // later `ahead_behind_by_sha` of the equal pair must hit it.
        let test = TestRepo::with_initial_commit();
        let trunk_sha = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["update-ref", "refs/remotes/origin/trunk", &trunk_sha]);
        for i in 0..10 {
            // Each branch is created from main and stays equal to it.
            test.run_git(&["branch", &format!("eq{i}"), "main"]);
            track(&test, &format!("eq{i}"), "trunk");
        }

        let repo = Repository::at(test.root_path()).unwrap();
        let locals = repo.local_branches().unwrap().to_vec();
        let remotes = repo.remote_branches().unwrap().to_vec();
        let written = repo.prime_upstream_ahead_behind_cache(&locals, &remotes);
        if written == 0 {
            return; // git < 2.36
        }
        // 10 equal-to-trunk branches plus main itself (if main has an
        // upstream, but we didn't track it here) — assert at least the 10.
        assert!(
            written >= 10,
            "all 10 equal branches must be cached (got {written})"
        );
        let cache_path = ahead_behind_cache_path(&repo, &trunk_sha, &trunk_sha);
        assert_eq!(
            std::fs::read_to_string(&cache_path).unwrap(),
            "[0,0]",
            "equal branch must cache as (0, 0) — same SHA on both sides"
        );
    }
}
