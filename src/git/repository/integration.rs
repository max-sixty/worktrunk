//! Integration detection operations for Repository.
//!
//! Methods for determining if a branch has been integrated into the target
//! (same commit, ancestor, trees match, etc.).

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use super::{RefSnapshot, Repository};
use crate::git::{IntegrationReason, check_integration, compute_integration_lazy};
use crate::shell_exec::Cmd;

/// Integration targets for `wt list`'s status column.
///
/// In the diverged case (local and upstream both have unique commits), the
/// safety path ORs over both — so the column needs to consider both, too.
/// Every other case collapses to a single target. See
/// [`Repository::integration_targets`] for the resolution rules.
#[derive(Debug, Clone)]
pub struct IntegrationTargets {
    /// Primary target — the only ref to check unless `secondary` is set.
    pub primary: String,
    /// Secondary target, only set in the diverged case.
    pub secondary: Option<String>,
}

/// Result of the combined merge-tree + patch-id integration probe.
///
/// Encapsulates the two-step sequence: first try `merge-tree --write-tree` to
/// check if merging would add anything, then fall back to patch-id matching
/// when merge-tree conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeProbeResult {
    /// Whether merging the branch into target would change the target's tree.
    /// Always `true` when merge-tree conflicts (conservative).
    pub would_merge_add: bool,
    /// Whether patch-id matching found the branch's squashed diff in a target commit.
    /// Only `true` when merge-tree conflicted AND patch-id found a match.
    pub is_patch_id_match: bool,
}

/// How many commits the patch-id squash-merge fallback is willing to walk on
/// the target side before giving up.
///
/// [`Repository::is_squash_merged_via_patch_id`] runs
/// `git rev-list {merge-base}..{target} | git diff-tree --stdin -p | git
/// patch-id` — one patch per commit in the range. That range is unbounded:
/// on a fast-moving repo an old branch can sit tens of thousands of commits
/// behind the tip, and a single such check then dominates `wt list`,
/// `wt remove`, and `wt step prune` (the latter goes visibly silent waiting
/// for it). A cheap graph-only `git rev-list --count` pre-flight enforces
/// this cap.
///
/// `500` is conservative because count is only a rough proxy for cost — the
/// per-commit work scales with `changed_files × changed_lines`, so a few
/// hundred lockfile-bump or large-refactor squashes can be slower than a few
/// thousand tiny commits. Working back from "keep one check under a few
/// seconds" on a heavy monorepo (~50-100 KB patches), 500 holds. On a typical
/// repo (~5-20 KB patches), 500 is well under a second. Branches squash-merged
/// within a normal review-and-cleanup cycle sit well inside this.
///
/// # Limitation
///
/// A branch that was squash-merged but whose merge point now sits more than
/// `PATCH_ID_SCAN_MAX_COMMITS` commits behind the default-branch tip is
/// reported as *not* integrated. This is the safe direction — the branch is
/// kept rather than wrongly deleted — and `wt remove -D` still removes it. The
/// fallback also only runs when `git merge-tree` itself conflicts (the same
/// files were modified again after the squash), so the affected set is already
/// narrow. There is no config knob; bump this constant if the trade-off needs
/// tuning.
const PATCH_ID_SCAN_MAX_COMMITS: usize = 500;

/// Outcome of `git merge-tree --write-tree`, classified by exit code.
///
/// Exit 0 → `Clean` carrying the resulting tree SHA; exit 1 → `Conflict`;
/// anything else is an error (the helper bails). Both merge-tree callers
/// share this dispatch; they differ only in what they do with a clean tree.
enum MergeTreeOutcome {
    Clean { tree: String },
    Conflict,
}

impl Repository {
    /// Check if `base_sha` is an ancestor of `head_sha`, taking commit SHAs
    /// directly. Hits the persistent SHA-keyed cache without going through
    /// the (stale-prone) ambient ref→SHA cache.
    ///
    /// Callers that hold SHAs (e.g., from a `RefSnapshot` or a
    /// `BranchRef.commit_sha`) should prefer this form.
    pub fn is_ancestor_by_sha(&self, base_sha: &str, head_sha: &str) -> anyhow::Result<bool> {
        if let Some(cached) = super::sha_cache::is_ancestor(self, base_sha, head_sha) {
            return Ok(cached);
        }

        let result =
            self.run_command_check(&["merge-base", "--is-ancestor", base_sha, head_sha])?;
        super::sha_cache::put_is_ancestor(self, base_sha, head_sha, result);
        Ok(result)
    }

    /// Check if a branch has file changes beyond the merge-base with target.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff to
    /// check for file changes. Returns false when the diff is empty (no added changes).
    ///
    /// For orphan branches (no common ancestor with target), returns true since all
    /// their changes are unique.
    ///
    /// Bypasses the ambient ref→SHA cache so callers holding SHAs from a
    /// [`RefSnapshot`] (or `BranchRef.commit_sha`) can short-circuit to the
    /// persistent SHA-keyed cache directly.
    pub fn has_added_changes_by_sha(
        &self,
        branch_sha: &str,
        target_sha: &str,
    ) -> anyhow::Result<bool> {
        if let Some(cached) = super::sha_cache::has_added_changes(self, branch_sha, target_sha) {
            return Ok(cached);
        }

        // Orphan branches have no common ancestor, so all their changes are unique
        let Some(merge_base) = self.merge_base(target_sha, branch_sha)? else {
            super::sha_cache::put_has_added_changes(self, branch_sha, target_sha, true);
            return Ok(true);
        };

        let range = format!("{merge_base}..{branch_sha}");
        let output = self.run_command(&["diff", "--name-only", &range])?;
        let result = !output.trim().is_empty();
        super::sha_cache::put_has_added_changes(self, branch_sha, target_sha, result);
        Ok(result)
    }

    /// Check if two commits have identical tree content (same files/directories).
    /// Returns true when content is identical even if commit history differs.
    ///
    /// Useful for detecting squash merges or rebases where the content has been
    /// integrated but commit ancestry doesn't show the relationship.
    ///
    /// Both commit→tree resolutions go through `commit_to_tree_sha`, so the
    /// shared target side (the default-branch tip, identical across every
    /// `wt list` row) is peeled once per process and shared with
    /// `would_merge_add_to_target`'s lookup. Inputs are commit SHAs, so the
    /// mapping is content-addressed and immune to ref-name staleness.
    pub fn trees_match_by_sha(&self, commit_sha1: &str, commit_sha2: &str) -> anyhow::Result<bool> {
        Ok(self.commit_to_tree_sha(commit_sha1)? == self.commit_to_tree_sha(commit_sha2)?)
    }

    /// Check if merging `head_sha` into `base_sha` would result in conflicts.
    ///
    /// Uses `git merge-tree` to simulate a merge without touching the working tree.
    /// Returns true if conflicts would occur, false for a clean merge.
    pub fn has_merge_conflicts_by_sha(
        &self,
        base_sha: &str,
        head_sha: &str,
    ) -> anyhow::Result<bool> {
        if let Some(cached) = super::sha_cache::merge_conflicts(self, base_sha, head_sha) {
            return Ok(cached);
        }

        self.run_merge_tree(base_sha, head_sha, base_sha, head_sha)
    }

    /// Check merge conflicts for a working tree represented by a tree SHA.
    ///
    /// Unlike [`Self::has_merge_conflicts_by_sha`] which takes commit SHAs,
    /// this accepts a raw tree SHA (from `git write-tree`) and the branch HEAD
    /// commit SHA. On cache miss, creates an ephemeral commit via
    /// `git commit-tree` to feed `merge-tree` (which requires commit objects
    /// for merge-base resolution). On cache hit, no commit is created.
    ///
    /// The cache key is `(base_commit_sha, branch_head_sha+tree_sha)` — a
    /// composite that captures all three inputs to the three-way merge:
    /// the base tree, the merge-base (via branch HEAD ancestry), and the
    /// working tree content.
    pub fn has_merge_conflicts_by_tree_with_base_sha(
        &self,
        base_sha: &str,
        branch_head_sha: &str,
        tree_sha: &str,
    ) -> anyhow::Result<bool> {
        let cache_head = format!("{branch_head_sha}+{tree_sha}");
        if let Some(cached) = super::sha_cache::merge_conflicts(self, base_sha, &cache_head) {
            return Ok(cached);
        }

        // Cache miss — create an ephemeral commit so merge-tree can resolve
        // the merge-base. The commit is unreferenced and will be GC'd.
        let head_commit =
            self.run_command(&["commit-tree", tree_sha, "-p", branch_head_sha, "-m", ""])?;
        let head_commit = head_commit.trim();

        self.run_merge_tree(base_sha, head_commit, base_sha, &cache_head)
    }

    /// Run `git merge-tree --write-tree <a> <b>` and classify the outcome by
    /// exit code. Exit 1 → [`MergeTreeOutcome::Conflict`]; anything other than
    /// success is an error; exit 0 → [`MergeTreeOutcome::Clean`] with the
    /// resulting tree SHA (first line of stdout).
    fn merge_tree_outcome(&self, a: &str, b: &str) -> anyhow::Result<MergeTreeOutcome> {
        // Exit codes: 0 = clean merge, 1 = conflicts, 128+ = error (invalid ref, corrupt repo)
        let output = self.run_command_output(&["merge-tree", "--write-tree", a, b])?;

        if output.status.code() == Some(1) {
            return Ok(MergeTreeOutcome::Conflict);
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git merge-tree failed for {a} {b}: {}", stderr.trim());
        }

        // Clean merge — first line of stdout is the resulting tree SHA.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tree = stdout.lines().next().unwrap_or("").trim().to_string();
        Ok(MergeTreeOutcome::Clean { tree })
    }

    /// Run merge-tree and cache the result.
    ///
    /// `base_sha` and `head_sha` are passed to `git merge-tree` (must be
    /// commit SHAs). `cache_base` and `cache_head` are used as the
    /// sha_cache key pair.
    fn run_merge_tree(
        &self,
        base_sha: &str,
        head_sha: &str,
        cache_base: &str,
        cache_head: &str,
    ) -> anyhow::Result<bool> {
        // Unrelated histories (no common ancestor) can't be merged — that's a conflict.
        if self.merge_base(base_sha, head_sha)?.is_none() {
            super::sha_cache::put_merge_conflicts(self, cache_base, cache_head, true);
            return Ok(true);
        }

        let conflicts = matches!(
            self.merge_tree_outcome(base_sha, head_sha)?,
            MergeTreeOutcome::Conflict
        );
        super::sha_cache::put_merge_conflicts(self, cache_base, cache_head, conflicts);
        Ok(conflicts)
    }

    /// Check if merging a branch into target would add anything (not already integrated).
    ///
    /// Caller must pass commit SHAs for both `branch` and `target`.
    ///
    /// Returns:
    /// - `Ok(Some(true))` if merging would change the target
    /// - `Ok(Some(false))` if merging would NOT change target (branch is already integrated)
    /// - `Ok(None)` if merge-tree conflicted (caller should try patch-id fallback)
    fn would_merge_add_to_target(
        &self,
        branch: &str,
        target: &str,
    ) -> anyhow::Result<Option<bool>> {
        match self.merge_tree_outcome(target, branch)? {
            // Conflicts — caller should try patch-id fallback.
            MergeTreeOutcome::Conflict => Ok(None),
            MergeTreeOutcome::Clean { tree } => {
                // If the merge result differs from target's tree, merging would
                // add something (the branch is not already integrated). The
                // target is shared across every `wt list` row, so resolve its
                // tree through the in-memory repo cache — peeled once, not once
                // per row (see [`Self::commit_to_tree_sha`]).
                let target_tree = self.commit_to_tree_sha(target)?;
                Ok(Some(tree != target_tree))
            }
        }
    }

    /// Detect squash merges via patch-id matching.
    ///
    /// Computes the combined diff of the entire branch (`diff-tree -p merge-base branch`)
    /// and checks if any single commit on the target has the same patch-id. A match means
    /// the target has a commit containing the exact same file changes as the whole branch
    /// — i.e., a squash merge.
    ///
    /// Both sides of the comparison generate their diffs with `git diff-tree`
    /// (plumbing), so the patch-ids are immune to the user's `diff.*` git
    /// config — see [`Self::patch_ids_from`].
    ///
    /// Only runs when `merge-tree` conflicts (both sides modified the same files),
    /// since `MergeAddsNothing` handles the non-conflict case. Cost scales with the
    /// number of commits on target since the merge-base (`git diff-tree`), so it is
    /// capped at [`PATCH_ID_SCAN_MAX_COMMITS`].
    ///
    /// Returns `Ok(true)` if a matching squash-merge commit is found on the target,
    /// `Ok(false)` otherwise (including when the target history is too deep to scan,
    /// or when patch-id computation fails — both conservative).
    fn is_squash_merged_via_patch_id(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        let Some(merge_base) = self.merge_base(target, branch)? else {
            return Ok(false);
        };

        // Bound the target-side history walk. The patch-id scan diffs every
        // commit landed on the default branch since this branch diverged; on
        // a fast-moving repo with an old branch that is tens of thousands of
        // commits, turning one integration check into seconds (or tens of
        // seconds) of work — visible as `wt step prune` / `wt list` going
        // silent. A `git rev-list --count` pre-flight (graph walk only, no
        // diffs) is cheap; bail above the cap. See the limitation note on
        // [`PATCH_ID_SCAN_MAX_COMMITS`].
        let target_commit_count: usize = self
            .run_command(&["rev-list", "--count", &format!("{merge_base}..{target}")])?
            .trim()
            .parse()
            .unwrap_or(0);
        if target_commit_count > PATCH_ID_SCAN_MAX_COMMITS {
            tracing::debug!(
                target_commit_count,
                merge_base = %merge_base,
                target = %target,
                "skipping patch-id squash-merge check: {target_commit_count} commits in {merge_base}..{target} exceeds cap of {PATCH_ID_SCAN_MAX_COMMITS}"
            );
            return Ok(false);
        }

        // Compute the squashed patch-id (combined diff of all branch changes).
        let branch_pids = self.patch_ids_from(&["diff-tree", "-p", &merge_base, branch], None)?;
        let Some(branch_pid) = branch_pids.split_whitespace().next() else {
            return Ok(false);
        };

        // Get all target commits' patch-ids in one pass. The diffs are
        // generated by `git diff-tree` — plumbing, like the branch side —
        // never `git log -p` (porcelain). Both sides must use the same
        // generator: `git patch-id --verbatim` hashes context lines, and
        // `log -p` honors `diff.context` / `diff.algorithm` from the user's
        // git config while `diff-tree` ignores them, so a mismatched pair
        // never agrees on a byte-identical change. `diff-tree --stdin` reads
        // the commit list on stdin and emits one diff per commit.
        let target_commits = self.run_command(&["rev-list", &format!("{merge_base}..{target}")])?;
        let target_pids = self.patch_ids_from(
            &["diff-tree", "--stdin", "-p"],
            Some(target_commits.into_bytes()),
        )?;

        Ok(target_pids
            .lines()
            .any(|line| line.split_whitespace().next() == Some(branch_pid)))
    }

    /// Pipe the output of `git <args>` directly into `git patch-id --verbatim`
    /// and return the patch-id output. `stdin`, when set, feeds `git <args>`'s
    /// standard input — used to pass a `git rev-list` commit list into
    /// `git diff-tree --stdin`.
    ///
    /// The intermediate diff never passes through this process — it flows from
    /// one git child to the other via an OS pipe. Keeps raw diffs out of our
    /// `-vv` debug stream (where `log_output` would otherwise dump every line
    /// of `git diff-tree -p`).
    ///
    /// Uses `--verbatim` (not `--stable`) to avoid false positives from
    /// whitespace normalization — `--stable` strips whitespace, so
    /// tabs-vs-spaces would produce matching patch-ids even though the content
    /// differs.
    fn patch_ids_from(&self, args: &[&str], stdin: Option<Vec<u8>>) -> anyhow::Result<String> {
        let mut source = Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context());
        if let Some(data) = stdin {
            source = source.stdin_bytes(data);
        }
        let sink = Cmd::new("git")
            .args(["patch-id", "--verbatim"])
            .current_dir(&self.discovery_path)
            .context(self.logging_context());
        let (source_output, sink_output) = source
            .pipe_into(sink)
            .context("Failed to compute patch-id")?;
        // A failed source (bad ref, I/O error) truncates the stream fed to
        // patch-id, which would then emit a bogus non-empty patch-id.
        if !source_output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&source_output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&sink_output.stdout).into_owned())
    }

    /// Combined merge-tree + patch-id integration probe.
    ///
    /// Single implementation of the merge-tree → patch-id fallback sequence,
    /// used by both `wt list` (parallel tasks) and `wt remove`/`wt merge`
    /// (sequential via [`compute_integration_lazy`]).
    ///
    /// The probe result depends only on the two committed trees (the patch-id
    /// fallback reads commits in `merge_base..target`, also a pure function of
    /// the two SHAs). Asymmetric: branch first, then target — the merge-tree
    /// result is compared against target's tree.
    pub fn merge_integration_probe_by_sha(
        &self,
        branch_sha: &str,
        target_sha: &str,
    ) -> anyhow::Result<MergeProbeResult> {
        if let Some(cached) = super::sha_cache::merge_add_probe(self, branch_sha, target_sha) {
            return Ok(cached);
        }

        // Orphan branches (no common ancestor) can't be merge-tree simulated
        // (git exits 128 with "refusing to merge unrelated histories") and have
        // no merge-base for patch-id either. Short-circuit: they always have changes.
        if self.merge_base(target_sha, branch_sha)?.is_none() {
            let result = MergeProbeResult {
                would_merge_add: true,
                is_patch_id_match: false,
            };
            super::sha_cache::put_merge_add_probe(self, branch_sha, target_sha, result);
            return Ok(result);
        }

        let merge_result = self.would_merge_add_to_target(branch_sha, target_sha)?;
        let result = match merge_result {
            Some(would_add) => MergeProbeResult {
                would_merge_add: would_add,
                is_patch_id_match: false,
            },
            None => {
                // merge-tree conflicted — try patch-id fallback.
                // Patch-id errors are non-fatal: if we can't compute patch-ids,
                // conservatively report no match (branch appears not integrated).
                let matched = self
                    .is_squash_merged_via_patch_id(branch_sha, target_sha)
                    .unwrap_or(false);
                MergeProbeResult {
                    would_merge_add: true,
                    is_patch_id_match: matched,
                }
            }
        };
        super::sha_cache::put_merge_add_probe(self, branch_sha, target_sha, result);
        Ok(result)
    }

    /// Resolve the integration targets for `wt list`'s status column.
    ///
    /// Mirrors the local/upstream branching in
    /// [`Self::integration_reason`] so the column's yes/no answer agrees
    /// with the safety path in `wt remove` / `wt merge`. The relationship
    /// determines whether a single ref or a (primary, secondary) pair is
    /// returned:
    ///
    /// - No upstream / local==upstream: primary = local, secondary = None.
    /// - Local strictly behind upstream: primary = upstream (superset).
    /// - Upstream strictly behind local: primary = local (superset).
    /// - Diverged: primary = local, secondary = upstream — both are checked
    ///   so a branch merged into either side counts as integrated.
    ///
    /// Ancestry probes resolve through the snapshot so a just-updated
    /// local target's relationship is observed correctly, matching
    /// [`Self::integration_reason`].
    ///
    /// Returns `None` when the default branch cannot be determined.
    pub fn integration_targets(&self, snapshot: &RefSnapshot) -> Option<IntegrationTargets> {
        let target = self.default_branch()?;
        let upstream = snapshot.upstream_of(&target).map(str::to_string);

        let target_sha = snapshot_resolve(self, snapshot, &target).ok()?;

        let (primary, secondary) = match upstream {
            None => (target, None),
            Some(u) => {
                let upstream_sha = snapshot_resolve(self, snapshot, &u).ok()?;
                if target_sha == upstream_sha {
                    (target, None)
                } else if self
                    .is_ancestor_by_sha(&target_sha, &upstream_sha)
                    .unwrap_or(false)
                {
                    (u, None)
                } else if self
                    .is_ancestor_by_sha(&upstream_sha, &target_sha)
                    .unwrap_or(false)
                {
                    (target, None)
                } else {
                    (target, Some(u))
                }
            }
        };

        Some(IntegrationTargets { primary, secondary })
    }

    /// Resolve a tree spec (e.g. `"refs/heads/main^{tree}"`) to its tree SHA.
    /// Uncached — tree resolution is cheap (~1 ms) and stale-prone if memoized
    /// against a moving ref name. When the input is a commit SHA, prefer
    /// [`Self::commit_to_tree_sha`], which memoizes the immutable commit→tree
    /// mapping.
    pub(super) fn rev_parse_tree(&self, spec: &str) -> anyhow::Result<String> {
        Ok(self
            .run_command(&["rev-parse", "--verify", "--end-of-options", spec])?
            .trim()
            .to_string())
    }

    /// Resolve a commit SHA to its tree SHA, memoized in the in-memory repo
    /// cache (get-or-create — the same lock-free pattern as
    /// [`Self::merge_base_by_sha`]).
    ///
    /// Unlike [`Self::rev_parse_tree`], the input is a commit SHA, so the
    /// commit→tree mapping is content-addressed and never stale within a
    /// process — a `git` object's tree is immutable. Caching it dedups the
    /// `wt list` hot path, where every branch row peels the *same*
    /// default-branch tip to its tree inside
    /// [`Self::would_merge_add_to_target`]; without the cache that is one
    /// identical `rev-parse` subprocess per row.
    ///
    /// In-memory rather than the persistent `sha_cache`: the resolution is
    /// ~1 ms, so cross-run persistence saves almost nothing, while the `Entry`
    /// match holds the shard lock across check-and-insert so the first miss
    /// computes once and every concurrent row reads it — no cold-start race.
    /// The disk cache is reserved for results expensive enough that persisting
    /// them across invocations outweighs a one-off recompute.
    fn commit_to_tree_sha(&self, commit_sha: &str) -> anyhow::Result<String> {
        use dashmap::mapref::entry::Entry;
        match self.cache.commit_tree.entry(commit_sha.to_string()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let tree = self.rev_parse_tree(&format!("{commit_sha}^{{tree}}"))?;
                Ok(e.insert(tree).clone())
            }
        }
    }

    /// Resolve a ref to its commit SHA. Uncached — callers that want
    /// SHA-stable lookups for an entire command should capture a
    /// [`RefSnapshot`] up front and resolve through it.
    pub(super) fn rev_parse_commit(&self, r: &str) -> anyhow::Result<String> {
        Ok(self
            .run_command(&["rev-parse", "--verify", "--end-of-options", r])?
            .trim()
            .to_string())
    }

    /// Resolve a ref to its commit SHA, skipping git when the input already
    /// looks like a 40-char hex SHA.
    pub(super) fn resolve_to_commit_sha(&self, r: &str) -> anyhow::Result<String> {
        if is_hex_commit_sha(r) {
            return Ok(r.to_string());
        }
        self.rev_parse_commit(r)
    }

    /// Check if a branch is integrated into a target.
    ///
    /// Combines [`compute_integration_lazy()`] and [`check_integration()`], and
    /// considers both the local target and its upstream so a branch counts as
    /// integrated whether it was merged locally OR remotely.
    ///
    /// Behavior depends on how local and upstream relate:
    ///
    /// - No upstream, or upstream at the same commit: check local only.
    /// - Local is an ancestor of upstream (local strictly behind): upstream is
    ///   a superset, so check upstream only — anything in local is also in
    ///   upstream.
    /// - Upstream is an ancestor of local (upstream strictly behind): local is
    ///   a superset, so check local only.
    /// - Diverged (neither is an ancestor): check local first; if not
    ///   integrated, also check upstream. The branch is integrated if either
    ///   matches.
    ///
    /// The "diverged → OR" case is the safety-critical one. After `wt merge`
    /// updates local main, local and upstream diverge until the merge is
    /// pushed; checking only one side would falsely report the just-merged
    /// branch as unmerged.
    ///
    /// Uses lazy evaluation with short-circuit: stops as soon as any check
    /// confirms integration, avoiding expensive operations like merge
    /// simulation when cheaper checks succeed.
    ///
    /// Returns `(effective_target, reason)` where:
    /// - `effective_target` is the ref that actually matched (the local target
    ///   when local matches or the branch is unmerged; the upstream ref when
    ///   only upstream matches).
    /// - `reason` is `Some(reason)` if integrated, `None` if not.
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let snapshot = repo.capture_refs()?;
    /// let (effective_target, reason) = repo.integration_reason(&snapshot, "feature", "main")?;
    /// if let Some(r) = reason {
    ///     println!("Branch integrated into {}: {}", effective_target, r.description());
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn integration_reason(
        &self,
        snapshot: &RefSnapshot,
        branch: &str,
        target: &str,
    ) -> anyhow::Result<(String, Option<IntegrationReason>)> {
        self.compute_integration_reason_uncached(snapshot, branch, target)
    }

    fn compute_integration_reason_uncached(
        &self,
        snapshot: &RefSnapshot,
        branch: &str,
        target: &str,
    ) -> anyhow::Result<(String, Option<IntegrationReason>)> {
        // Resolve upstream once. Errors and "no upstream" both collapse to None.
        let upstream = snapshot
            .upstream_of(target)
            .map(str::to_string)
            .or_else(|| self.branch(target).upstream().ok().flatten());

        // Decide whether to check local, upstream, or both. All ancestor
        // probes go through SHAs resolved at the snapshot — by construction,
        // the snapshot reflects ref state at capture time, so a freshly
        // captured snapshot post-`update-ref` observes the new local target
        // SHA correctly. (PR #2507's `ref_is_ancestor` workaround dissolves
        // here.)
        let target_sha = snapshot_resolve(self, snapshot, target)?;

        let (check_local, fallback_upstream) = match upstream {
            None => (true, None),
            Some(u) => {
                let upstream_sha = snapshot_resolve(self, snapshot, &u)?;
                if target_sha == upstream_sha {
                    (true, None)
                } else if self.is_ancestor_by_sha(&target_sha, &upstream_sha)? {
                    // Local strictly behind upstream — upstream is the superset.
                    let signals = compute_integration_lazy(self, snapshot, branch, &u)?;
                    return Ok((u, check_integration(&signals)));
                } else if self.is_ancestor_by_sha(&upstream_sha, &target_sha)? {
                    // Upstream strictly behind local — local is the superset.
                    (true, None)
                } else {
                    // Diverged — check local first, fall back to upstream.
                    (true, Some(u))
                }
            }
        };

        if check_local
            && let Some(reason) =
                check_integration(&compute_integration_lazy(self, snapshot, branch, target)?)
        {
            return Ok((target.to_string(), Some(reason)));
        }

        if let Some(upstream) = fallback_upstream
            && let Some(reason) = check_integration(&compute_integration_lazy(
                self, snapshot, branch, &upstream,
            )?)
        {
            return Ok((upstream, Some(reason)));
        }

        Ok((target.to_string(), None))
    }
}

/// Resolve a ref name through `snapshot`, falling back to an uncached
/// `git rev-parse` for refs the snapshot doesn't carry.
fn snapshot_resolve(
    repo: &Repository,
    snapshot: &RefSnapshot,
    name: &str,
) -> anyhow::Result<String> {
    if let Some(sha) = snapshot.resolve(name) {
        return Ok(sha.to_string());
    }
    Ok(repo
        .run_command(&["rev-parse", "--verify", "--end-of-options", name])?
        .trim()
        .to_string())
}

/// Returns true when `s` is a 40-character hex string — the canonical form
/// of a git commit SHA-1.
fn is_hex_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod snapshot_resolve_tests {
    use super::*;
    use crate::testing::TestRepo;

    /// Exercises the `git rev-parse` fallback when the snapshot doesn't
    /// carry the ref (e.g. `HEAD`, tags, raw SHAs). The protective
    /// `--verify --end-of-options` is on this call site too — keep this
    /// test honest about what shape rev-parse returns.
    #[test]
    fn falls_back_to_rev_parse_for_refs_not_in_snapshot() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let snapshot = repo.capture_refs().unwrap();

        // `HEAD` isn't captured into the snapshot; the fallback resolves it
        // through git, producing the same SHA HEAD points at.
        let head_sha_via_fallback = snapshot_resolve(&repo, &snapshot, "HEAD").unwrap();
        let head_sha_direct = repo.run_command(&["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head_sha_via_fallback, head_sha_direct.trim());

        // Bogus ref → error (not a confusing "unknown option" — `--verify`
        // surfaces a clean "Needed a single revision" / "bad revision").
        assert!(snapshot_resolve(&repo, &snapshot, "no-such-ref").is_err());
    }
}

#[cfg(test)]
mod commit_tree_cache_tests {
    use super::*;
    use crate::testing::TestRepo;

    /// The integration probe peels the target commit to its tree once and
    /// memoizes it in the in-memory repo cache, keyed by the target commit
    /// SHA — so the many `wt list` rows sharing one target don't each spawn
    /// `git rev-parse <target>^{tree}`.
    #[test]
    fn probe_caches_target_tree_in_memory() {
        let test = TestRepo::with_initial_commit();

        // Feature adds a file; main is unchanged. The merge is clean but adds
        // changes, so the probe reaches the Clean arm of
        // would_merge_add_to_target and resolves the target (main) tree.
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "Feature"]);
        test.run_git(&["checkout", "main"]);

        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);
        let main_tree = test.git_output(&["rev-parse", "main^{tree}"]);

        let repo = Repository::at(test.root_path()).unwrap();
        assert!(!repo.cache.commit_tree.contains_key(&main_sha));

        let probe = repo
            .merge_integration_probe_by_sha(&feature_sha, &main_sha)
            .unwrap();
        assert!(probe.would_merge_add, "clean merge adds the new file");

        // Target tree resolved once and cached under the target commit SHA.
        let cached = repo
            .cache
            .commit_tree
            .get(&main_sha)
            .map(|e| e.value().clone());
        assert_eq!(cached, Some(main_tree));
    }
}

#[cfg(test)]
mod hex_sha_tests {
    use super::is_hex_commit_sha;

    #[test]
    fn detects_full_hex_sha() {
        assert!(is_hex_commit_sha(
            "273f078bd20a09f1a524aae48fcb1771ceac9b5d"
        ));
    }

    #[test]
    fn rejects_branch_names() {
        assert!(!is_hex_commit_sha("main"));
        assert!(!is_hex_commit_sha("feature/foo"));
    }

    #[test]
    fn rejects_short_or_long() {
        assert!(!is_hex_commit_sha(
            "273f078bd20a09f1a524aae48fcb1771ceac9b5"
        ));
        assert!(!is_hex_commit_sha(
            "273f078bd20a09f1a524aae48fcb1771ceac9b5d0"
        ));
    }

    #[test]
    fn rejects_non_hex_chars() {
        assert!(!is_hex_commit_sha(
            "z73f078bd20a09f1a524aae48fcb1771ceac9b5d"
        ));
    }
}

#[cfg(test)]
mod patch_id_tests {
    use super::*;
    use crate::testing::TestRepo;
    use std::fmt::Write as _;

    /// Build the topology
    ///
    /// ```text
    /// base ─── feature  (file: A → B)
    ///   └────── squash ── pad1 ── … ── padN  = target  (each pad reuses parent's tree)
    /// ```
    ///
    /// via a single `git fast-import` stream — instant even at N = 2000, where
    /// running `git commit` once per pad would dominate the test.
    /// `merge_base(target, feature)` is `base`; `squash`'s combined patch
    /// equals `feature`'s; `rev-list --count base..target` is `1 + n_padding`.
    fn build(test: &TestRepo, n_padding: usize) {
        let mut s = String::new();
        s.push_str("blob\nmark :10\ndata 1\nA\n");
        s.push_str("blob\nmark :11\ndata 1\nB\n");
        writeln!(
            s,
            "commit refs/heads/base\nmark :1\ncommitter T <t@x> 1700000000 +0000\ndata 0\nM 100644 :10 file\n"
        )
        .unwrap();
        writeln!(
            s,
            "commit refs/heads/feature\nmark :2\ncommitter T <t@x> 1700000001 +0000\ndata 0\nfrom :1\nM 100644 :11 file\n"
        )
        .unwrap();
        writeln!(
            s,
            "commit refs/heads/target\nmark :3\ncommitter T <t@x> 1700000002 +0000\ndata 0\nfrom :1\nM 100644 :11 file\n"
        )
        .unwrap();
        for i in 0..n_padding {
            let mark = 4 + i;
            let parent = if i == 0 { 3 } else { mark - 1 };
            writeln!(
                s,
                "commit refs/heads/target\nmark :{mark}\ncommitter T <t@x> {} +0000\ndata 0\nfrom :{parent}\n",
                1_700_000_003 + i
            )
            .unwrap();
        }
        let output = test
            .git_command()
            .args(["fast-import", "--quiet"])
            .stdin_bytes(s.into_bytes())
            .run()
            .expect("fast-import spawn");
        assert!(
            output.status.success(),
            "fast-import failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn rev_parse(repo: &Repository, r: &str) -> String {
        repo.run_command(&["rev-parse", r])
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn detects_squash_when_range_is_under_cap() {
        let test = TestRepo::new();
        // base..target = 2 commits (squash + one pad). Well under the cap.
        build(&test, 1);
        let repo = Repository::at(test.root_path()).unwrap();
        let feature = rev_parse(&repo, "refs/heads/feature");
        let target = rev_parse(&repo, "refs/heads/target");

        assert!(
            repo.is_squash_merged_via_patch_id(&feature, &target)
                .unwrap(),
            "squash commit's patch-id matches feature's; should be detected within the cap"
        );
    }

    #[test]
    fn bails_when_range_exceeds_cap() {
        let test = TestRepo::new();
        // base..target = 1 (squash) + PATCH_ID_SCAN_MAX_COMMITS pads — one
        // over the cap. The squash is still in the range; we just refuse to
        // walk that far. Asserting `false` while the under-cap case asserts
        // `true` pins the difference to the cap, not the topology.
        build(&test, PATCH_ID_SCAN_MAX_COMMITS);
        let repo = Repository::at(test.root_path()).unwrap();
        let feature = rev_parse(&repo, "refs/heads/feature");
        let target = rev_parse(&repo, "refs/heads/target");

        assert!(
            !repo
                .is_squash_merged_via_patch_id(&feature, &target)
                .unwrap(),
            "should bail (return false) when base..target exceeds {PATCH_ID_SCAN_MAX_COMMITS}"
        );
    }

    /// Squash-merge detection must not depend on the user's `diff.*` git
    /// config. `git patch-id --verbatim` hashes context lines, so the two
    /// sides of the comparison must generate their diffs identically. Both
    /// now use `git diff-tree` (plumbing — ignores `diff.context`,
    /// `diff.algorithm`, …); the target side once used `git log -p`
    /// (porcelain — honors them), so a repo with a non-default `diff.context`
    /// made `log -p` emit a different patch-id for a byte-identical change,
    /// and a genuinely squash-merged branch was reported as not integrated.
    ///
    /// This fixture sets non-default `diff.*` config in the repo's *local*
    /// config (hermetic — bites regardless of the machine's global git
    /// config) and builds a squash merge followed by a commit re-touching
    /// the same line, so `git merge-tree` conflicts and the patch-id
    /// fallback actually runs.
    #[test]
    fn detects_squash_merge_under_nondefault_diff_config() {
        let test = TestRepo::new();
        let repo = &test.repo;

        // `diff.context` is the active discriminator below; `diff.algorithm`
        // is set too, to prove immunity to that latent second trigger.
        test.run_git(&["config", "diff.context", "25"]);
        test.run_git(&["config", "diff.algorithm", "histogram"]);

        // A 60-line file: a one-line change in the middle yields a hunk
        // whose context window differs between `diff.context = 3` and `= 25`,
        // so the two diff generators produce different `--verbatim`
        // patch-ids unless both ignore the config.
        let path = test.path().join("file");
        let base: String = (1..=60).map(|i| format!("l{i}\n")).collect();
        std::fs::write(&path, &base).unwrap();
        test.run_git(&["add", "file"]);
        test.run_git(&["commit", "-m", "base"]);

        let changed = base.replace("l30\n", "FEATURE\n");

        // feature branch: the change that should be detected as integrated.
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(&path, &changed).unwrap();
        test.run_git(&["commit", "-am", "feature change"]);

        // target (main): a squash-merge of feature, then a follow-up commit
        // re-touching the same line. The follow-up makes `merge-tree`
        // conflict — the only path that reaches the patch-id fallback.
        test.run_git(&["checkout", "main"]);
        std::fs::write(&path, &changed).unwrap();
        test.run_git(&["commit", "-am", "squash-merge feature"]);
        std::fs::write(&path, changed.replace("FEATURE\n", "PADDED\n")).unwrap();
        test.run_git(&["commit", "-am", "follow-up on same line"]);

        let snapshot = repo.capture_refs().unwrap();
        let signals = compute_integration_lazy(repo, &snapshot, "feature", "main").unwrap();

        assert_eq!(
            check_integration(&signals),
            Some(IntegrationReason::PatchIdMatch),
            "squash merge must be detected via patch-id regardless of diff.* config"
        );
    }
}
