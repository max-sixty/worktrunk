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

impl Repository {
    /// Resolve a ref, preferring branches over tags when names collide.
    ///
    /// Uses git to check if `refs/heads/{ref}` exists. If so, returns the
    /// qualified form to ensure we reference the branch, not a same-named tag.
    /// Otherwise returns the original ref unchanged (for HEAD, SHAs, remote refs).
    /// Uncached: this is only consulted by the ref-taking convenience
    /// wrappers; hot paths go through a [`RefSnapshot`] instead.
    fn resolve_preferring_branch(&self, r: &str) -> String {
        let qualified = format!("refs/heads/{r}");
        if self
            .run_command(&["rev-parse", "--verify", "-q", &qualified])
            .is_ok()
        {
            qualified
        } else {
            r.to_string()
        }
    }

    /// Resolve a ref for integration helpers — passthrough for already-qualified
    /// refs, branch-preferring resolution for short names.
    ///
    /// Inputs starting with `refs/` (e.g., from `BranchRef::full_ref()`) are
    /// returned unchanged: they're already unambiguous, and re-qualifying would
    /// either (a) waste a `rev-parse` in the common case, or (b) pick the wrong
    /// ref when a branch literally named e.g. `refs/heads/foo` exists — the
    /// caller gave us `refs/heads/foo` meaning the branch `foo`, not the
    /// pathological branch at `refs/heads/refs/heads/foo`.
    ///
    /// Short names go through `resolve_preferring_branch` so its
    /// branch-over-tag disambiguation still applies to user/CLI input.
    fn resolve_ref(&self, r: &str) -> String {
        if r.starts_with("refs/") {
            r.to_string()
        } else {
            self.resolve_preferring_branch(r)
        }
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    ///
    /// See [`--is-ancestor`][1] for details.
    ///
    /// [1]: https://git-scm.com/docs/git-merge-base#Documentation/git-merge-base.txt---is-ancestor
    pub fn is_ancestor(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        let base = self.resolve_ref(base);
        let head = self.resolve_ref(head);

        let base_sha = self.rev_parse_commit(&base)?;
        let head_sha = self.rev_parse_commit(&head)?;

        self.is_ancestor_by_sha(&base_sha, &head_sha)
    }

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

    /// Check if two refs point to the same commit.
    pub fn same_commit(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        let ref1 = self.resolve_ref(ref1);
        let ref2 = self.resolve_ref(ref2);
        // Parse both refs in a single git command
        let output = self.run_command(&["rev-parse", &ref1, &ref2])?;
        let mut lines = output.lines();
        let sha1 = lines.next().context("rev-parse returned no output")?.trim();
        let sha2 = lines
            .next()
            .context("rev-parse returned only one line")?
            .trim();
        Ok(sha1 == sha2)
    }

    /// Check if a branch has file changes beyond the merge-base with target.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff to
    /// check for file changes. Returns false when the diff is empty (no added changes).
    ///
    /// For orphan branches (no common ancestor with target), returns true since all
    /// their changes are unique.
    pub fn has_added_changes(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        let branch = self.resolve_ref(branch);
        let target = self.resolve_ref(target);

        let branch_sha = self.rev_parse_commit(&branch)?;
        let target_sha = self.rev_parse_commit(&target)?;

        self.has_added_changes_by_sha(&branch_sha, &target_sha)
    }

    /// SHA-keyed variant of [`Self::has_added_changes`].
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

    /// Check if two refs have identical tree content (same files/directories).
    /// Returns true when content is identical even if commit history differs.
    ///
    /// Useful for detecting squash merges or rebases where the content has been
    /// integrated but commit ancestry doesn't show the relationship.
    pub fn trees_match(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        let ref1 = self.resolve_ref(ref1);
        let ref2 = self.resolve_ref(ref2);
        // Parse both tree refs in a single git command
        let output = self.run_command(&[
            "rev-parse",
            &format!("{ref1}^{{tree}}"),
            &format!("{ref2}^{{tree}}"),
        ])?;
        let mut lines = output.lines();
        let tree1 = lines.next().context("rev-parse returned no output")?.trim();
        let tree2 = lines
            .next()
            .context("rev-parse returned only one line")?
            .trim();
        Ok(tree1 == tree2)
    }

    /// SHA-keyed variant of [`Self::trees_match`].
    ///
    /// Resolves each commit SHA to its tree SHA in a single `git rev-parse`
    /// call and compares. The commit→tree resolution is itself uncached
    /// (trees are cheap and not stale-prone), but the inputs are SHAs so
    /// the call is immune to ref-name staleness.
    pub fn trees_match_by_sha(&self, commit_sha1: &str, commit_sha2: &str) -> anyhow::Result<bool> {
        let output = self.run_command(&[
            "rev-parse",
            &format!("{commit_sha1}^{{tree}}"),
            &format!("{commit_sha2}^{{tree}}"),
        ])?;
        let mut lines = output.lines();
        let tree1 = lines.next().context("rev-parse returned no output")?.trim();
        let tree2 = lines
            .next()
            .context("rev-parse returned only one line")?
            .trim();
        Ok(tree1 == tree2)
    }

    /// Check if HEAD's tree SHA matches a branch's tree SHA.
    /// Returns true when content is identical even if commit history differs.
    pub fn head_tree_matches_branch(&self, branch: &str) -> anyhow::Result<bool> {
        self.trees_match("HEAD", branch)
    }

    /// Check if merging head into base would result in conflicts.
    ///
    /// Uses `git merge-tree` to simulate a merge without touching the working tree.
    /// Returns true if conflicts would occur, false for a clean merge.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let has_conflicts = repo.has_merge_conflicts("main", "feature-branch")?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn has_merge_conflicts(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        let base = self.resolve_ref(base);
        let head = self.resolve_ref(head);

        let base_sha = self.rev_parse_commit(&base)?;
        let head_sha = self.rev_parse_commit(&head)?;

        self.has_merge_conflicts_by_sha(&base_sha, &head_sha)
    }

    /// SHA-keyed variant of [`Self::has_merge_conflicts`].
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
    /// Unlike [`Self::has_merge_conflicts`] which takes commit refs, this accepts a
    /// raw tree SHA (from `git write-tree`) and the branch HEAD commit SHA.
    /// On cache miss, creates an ephemeral commit via `git commit-tree` to
    /// feed `merge-tree` (which requires commit objects for merge-base
    /// resolution). On cache hit, no commit is created.
    ///
    /// The cache key is `(base_commit_sha, branch_head_sha+tree_sha)` — a
    /// composite that captures all three inputs to the three-way merge:
    /// the base tree, the merge-base (via branch HEAD ancestry), and the
    /// working tree content.
    pub fn has_merge_conflicts_by_tree(
        &self,
        base: &str,
        branch_head_sha: &str,
        tree_sha: &str,
    ) -> anyhow::Result<bool> {
        let base = self.resolve_ref(base);
        let base_sha = self.rev_parse_commit(&base)?;

        self.has_merge_conflicts_by_tree_with_base_sha(&base_sha, branch_head_sha, tree_sha)
    }

    /// SHA-keyed variant of [`Self::has_merge_conflicts_by_tree`] —
    /// `base_sha` is taken as a SHA (the other two arguments are already
    /// SHA-shaped). Same composite cache key as the ref-taking form.
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

        // Exit codes: 0 = clean merge, 1 = conflicts, 128+ = error (invalid ref, corrupt repo)
        let output =
            self.run_command_output(&["merge-tree", "--write-tree", base_sha, head_sha])?;

        if output.status.code() == Some(1) {
            super::sha_cache::put_merge_conflicts(self, cache_base, cache_head, true);
            return Ok(true);
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "git merge-tree failed for {base_sha} {head_sha}: {}",
                stderr.trim()
            );
        }
        super::sha_cache::put_merge_conflicts(self, cache_base, cache_head, false);
        Ok(false)
    }

    /// Check if merging a branch into target would add anything (not already integrated).
    ///
    /// Caller must pass resolved refs (via `resolve_ref`).
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
        // Exit codes: 0 = clean merge, 1 = conflicts, 128+ = error (invalid ref, corrupt repo)
        let output = self.run_command_output(&["merge-tree", "--write-tree", target, branch])?;

        if output.status.code() == Some(1) {
            // Conflicts — caller should try patch-id fallback
            return Ok(None);
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "git merge-tree failed for {target} {branch}: {}",
                stderr.trim()
            );
        }

        // Clean merge — first line of stdout is the resulting tree SHA
        let merge_tree = String::from_utf8_lossy(&output.stdout);
        let merge_tree = merge_tree.lines().next().unwrap_or("").trim();

        // Get target's tree for comparison
        let target_tree = self.rev_parse_tree(&format!("{target}^{{tree}}"))?;

        // If merge result differs from target's tree, merging would add something
        Ok(Some(merge_tree != target_tree))
    }

    /// Detect squash merges via patch-id matching.
    ///
    /// Computes the combined diff of the entire branch (`diff-tree -p merge-base branch`)
    /// and checks if any single commit on the target has the same patch-id. A match means
    /// the target has a commit containing the exact same file changes as the whole branch
    /// — i.e., a squash merge.
    ///
    /// Only runs when `merge-tree` conflicts (both sides modified the same files),
    /// since `MergeAddsNothing` handles the non-conflict case. Cost scales with the
    /// number of commits on target since the merge-base (`git log -p`).
    ///
    /// Returns `Ok(true)` if a matching squash-merge commit is found on the target,
    /// `Ok(false)` otherwise (including when patch-id computation fails — conservative).
    fn is_squash_merged_via_patch_id(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        let Some(merge_base) = self.merge_base(target, branch)? else {
            return Ok(false);
        };

        // Compute the squashed patch-id (combined diff of all branch changes).
        let branch_pids = self.patch_ids_from(&["diff-tree", "-p", &merge_base, branch])?;
        let Some(branch_pid) = branch_pids.split_whitespace().next() else {
            return Ok(false);
        };

        // Get all target commits' patch-ids in one pass.
        let target_pids =
            self.patch_ids_from(&["log", "-p", "--reverse", &format!("{merge_base}..{target}")])?;

        Ok(target_pids
            .lines()
            .any(|line| line.split_whitespace().next() == Some(branch_pid)))
    }

    /// Pipe the output of `git <args>` directly into `git patch-id --verbatim`
    /// and return the patch-id output.
    ///
    /// The intermediate diff never passes through this process — it flows from
    /// one git child to the other via an OS pipe. Keeps raw diffs out of our
    /// `-vv` debug stream (where `log_output` would otherwise dump every line
    /// of `git diff-tree -p` / `git log -p`).
    ///
    /// Uses `--verbatim` (not `--stable`) to avoid false positives from
    /// whitespace normalization — `--stable` strips whitespace, so
    /// tabs-vs-spaces would produce matching patch-ids even though the content
    /// differs.
    fn patch_ids_from(&self, args: &[&str]) -> anyhow::Result<String> {
        let source = Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.discovery_path)
            .context(self.logging_context());
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
    pub fn merge_integration_probe(
        &self,
        branch: &str,
        target: &str,
    ) -> anyhow::Result<MergeProbeResult> {
        let branch = self.resolve_ref(branch);
        let target = self.resolve_ref(target);

        // Resolve refs to commit SHAs for the persistent cache key.
        // The probe result depends only on the two committed trees (the
        // patch-id fallback reads commits in merge_base..target, also a
        // pure function of the two SHAs). Asymmetric key: branch first,
        // then target, because the merge-tree result is compared against
        // target's tree.
        let branch_sha = self.rev_parse_commit(&branch)?;
        let target_sha = self.rev_parse_commit(&target)?;

        self.merge_integration_probe_by_sha(&branch_sha, &target_sha)
    }

    /// SHA-keyed variant of [`Self::merge_integration_probe`].
    ///
    /// Asymmetric: branch first, then target — the merge-tree result is
    /// compared against target's tree.
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
    /// against a moving ref name.
    pub(super) fn rev_parse_tree(&self, spec: &str) -> anyhow::Result<String> {
        Ok(self.run_command(&["rev-parse", spec])?.trim().to_string())
    }

    /// Resolve a ref to its commit SHA. Uncached — callers that want
    /// SHA-stable lookups for an entire command should capture a
    /// [`RefSnapshot`] up front and resolve through it.
    pub(super) fn rev_parse_commit(&self, r: &str) -> anyhow::Result<String> {
        Ok(self.run_command(&["rev-parse", r])?.trim().to_string())
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
    Ok(repo.run_command(&["rev-parse", name])?.trim().to_string())
}

/// Returns true when `s` is a 40-character hex string — the canonical form
/// of a git commit SHA-1.
fn is_hex_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
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
