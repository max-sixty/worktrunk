//! Integration detection operations for Repository.
//!
//! Methods for determining if a branch has been integrated into the target
//! (same commit, ancestor, trees match, etc.).

use anyhow::Context;
use dashmap::mapref::entry::Entry;
use serde::{Deserialize, Serialize};

use super::Repository;
use crate::git::{IntegrationReason, check_integration, compute_integration_lazy};
use crate::shell_exec::Cmd;

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
    fn resolve_preferring_branch(&self, r: &str) -> String {
        self.cache
            .resolved_refs
            .entry(r.to_string())
            .or_insert_with(|| {
                let qualified = format!("refs/heads/{r}");
                if self
                    .run_command(&["rev-parse", "--verify", "-q", &qualified])
                    .is_ok()
                {
                    qualified
                } else {
                    r.to_string()
                }
            })
            .clone()
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    ///
    /// See [`--is-ancestor`][1] for details.
    ///
    /// [1]: https://git-scm.com/docs/git-merge-base#Documentation/git-merge-base.txt---is-ancestor
    pub fn is_ancestor(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        let base = self.resolve_preferring_branch(base);
        let head = self.resolve_preferring_branch(head);

        let base_sha = self.rev_parse_commit(&base)?;
        let head_sha = self.rev_parse_commit(&head)?;

        if let Some(cached) = super::sha_cache::is_ancestor(self, &base_sha, &head_sha) {
            return Ok(cached);
        }

        let result =
            self.run_command_check(&["merge-base", "--is-ancestor", &base_sha, &head_sha])?;
        super::sha_cache::put_is_ancestor(self, &base_sha, &head_sha, result);
        Ok(result)
    }

    /// Check if two refs point to the same commit.
    pub fn same_commit(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        let ref1 = self.resolve_preferring_branch(ref1);
        let ref2 = self.resolve_preferring_branch(ref2);
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
        let branch = self.resolve_preferring_branch(branch);
        let target = self.resolve_preferring_branch(target);

        let branch_sha = self.rev_parse_commit(&branch)?;
        let target_sha = self.rev_parse_commit(&target)?;

        if let Some(cached) = super::sha_cache::has_added_changes(self, &branch_sha, &target_sha) {
            return Ok(cached);
        }

        // Orphan branches have no common ancestor, so all their changes are unique
        let Some(merge_base) = self.merge_base(&target_sha, &branch_sha)? else {
            super::sha_cache::put_has_added_changes(self, &branch_sha, &target_sha, true);
            return Ok(true);
        };

        let range = format!("{merge_base}..{branch_sha}");
        let output = self.run_command(&["diff", "--name-only", &range])?;
        let result = !output.trim().is_empty();
        super::sha_cache::put_has_added_changes(self, &branch_sha, &target_sha, result);
        Ok(result)
    }

    /// Check if two refs have identical tree content (same files/directories).
    /// Returns true when content is identical even if commit history differs.
    ///
    /// Useful for detecting squash merges or rebases where the content has been
    /// integrated but commit ancestry doesn't show the relationship.
    pub fn trees_match(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        let ref1 = self.resolve_preferring_branch(ref1);
        let ref2 = self.resolve_preferring_branch(ref2);
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
        let base = self.resolve_preferring_branch(base);
        let head = self.resolve_preferring_branch(head);

        let base_sha = self.rev_parse_commit(&base)?;
        let head_sha = self.rev_parse_commit(&head)?;

        if let Some(cached) = super::sha_cache::merge_conflicts(self, &base_sha, &head_sha) {
            return Ok(cached);
        }

        self.run_merge_tree(&base_sha, &head_sha, &base_sha, &head_sha)
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
        let base = self.resolve_preferring_branch(base);
        let base_sha = self.rev_parse_commit(&base)?;

        let cache_head = format!("{branch_head_sha}+{tree_sha}");
        if let Some(cached) = super::sha_cache::merge_conflicts(self, &base_sha, &cache_head) {
            return Ok(cached);
        }

        // Cache miss — create an ephemeral commit so merge-tree can resolve
        // the merge-base. The commit is unreferenced and will be GC'd.
        let head_commit =
            self.run_command(&["commit-tree", tree_sha, "-p", branch_head_sha, "-m", ""])?;
        let head_commit = head_commit.trim();

        self.run_merge_tree(&base_sha, head_commit, &base_sha, &cache_head)
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
    /// Caller must pass resolved refs (via `resolve_preferring_branch`).
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
        let branch_diff = self.run_command(&["diff-tree", "-p", &merge_base, branch])?;
        let branch_output = self.compute_patch_ids(&branch_diff)?;
        let Some(branch_pid) = branch_output.split_whitespace().next() else {
            return Ok(false);
        };

        // Get all target commits' patch-ids in one pass.
        // `git log -p` pipes all patches through `git patch-id --verbatim`.
        let target_log =
            self.run_command(&["log", "-p", "--reverse", &format!("{merge_base}..{target}")])?;

        let target_pids = self.compute_patch_ids(&target_log)?;

        Ok(target_pids
            .lines()
            .any(|line| line.split_whitespace().next() == Some(branch_pid)))
    }

    /// Pipe diff content through `git patch-id --verbatim` and return the output.
    ///
    /// Uses `--verbatim` (not `--stable`) to avoid false positives from whitespace
    /// normalization — `--stable` strips whitespace, so tabs-vs-spaces would produce
    /// matching patch-ids even though the content differs.
    fn compute_patch_ids(&self, diff: &str) -> anyhow::Result<String> {
        let output = Cmd::new("git")
            .args(["patch-id", "--verbatim"])
            .current_dir(&self.discovery_path)
            .context(self.logging_context())
            .stdin_bytes(diff.to_owned())
            .run()
            .context("Failed to compute patch-id")?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
        let branch = self.resolve_preferring_branch(branch);
        let target = self.resolve_preferring_branch(target);

        // Resolve refs to commit SHAs for the persistent cache key.
        // The probe result depends only on the two committed trees (the
        // patch-id fallback reads commits in merge_base..target, also a
        // pure function of the two SHAs). Asymmetric key: branch first,
        // then target, because the merge-tree result is compared against
        // target's tree.
        let branch_sha = self.rev_parse_commit(&branch)?;
        let target_sha = self.rev_parse_commit(&target)?;

        if let Some(cached) = super::sha_cache::merge_add_probe(self, &branch_sha, &target_sha) {
            return Ok(cached);
        }

        // Orphan branches (no common ancestor) can't be merge-tree simulated
        // (git exits 128 with "refusing to merge unrelated histories") and have
        // no merge-base for patch-id either. Short-circuit: they always have changes.
        if self.merge_base(&target_sha, &branch_sha)?.is_none() {
            let result = MergeProbeResult {
                would_merge_add: true,
                is_patch_id_match: false,
            };
            super::sha_cache::put_merge_add_probe(self, &branch_sha, &target_sha, result);
            return Ok(result);
        }

        let merge_result = self.would_merge_add_to_target(&branch_sha, &target_sha)?;
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
                    .is_squash_merged_via_patch_id(&branch_sha, &target_sha)
                    .unwrap_or(false);
                MergeProbeResult {
                    would_merge_add: true,
                    is_patch_id_match: matched,
                }
            }
        };
        super::sha_cache::put_merge_add_probe(self, &branch_sha, &target_sha, result);
        Ok(result)
    }

    /// Determine the effective target for integration checks.
    ///
    /// If the upstream of the local target (e.g., `origin/main`) contains commits that
    /// the local target does not, uses the upstream. This handles both the common "local
    /// branch is behind upstream" case and the diverged case where local has extra commits
    /// but upstream contains a remote merge that local hasn't integrated yet.
    ///
    /// When local and upstream are the same commit, prefers local for clearer messaging.
    ///
    /// Returns the effective target ref to check against.
    ///
    /// Used by both `wt list` and `wt remove` to ensure consistent integration detection.
    ///
    pub fn effective_integration_target(&self, local_target: &str) -> String {
        self.cache
            .effective_integration_targets
            .entry(local_target.to_string())
            .or_insert_with(|| {
                // Get the upstream ref for the local target (e.g., origin/main for main)
                let upstream = match self.branch(local_target).upstream() {
                    Ok(Some(upstream)) => upstream,
                    _ => return local_target.to_string(),
                };

                // If local and upstream are the same commit, prefer local for clearer messaging
                if self.same_commit(local_target, &upstream).unwrap_or(false) {
                    return local_target.to_string();
                }

                // If upstream contains commits not present in local, prefer upstream so
                // remotely merged branches still count as integrated after a fetch.
                if self.is_ancestor(local_target, &upstream).unwrap_or(false) {
                    return upstream;
                }

                // If upstream is strictly behind local, local is more complete.
                if self.is_ancestor(&upstream, local_target).unwrap_or(false) {
                    return local_target.to_string();
                }

                // Local and upstream have diverged (neither is ancestor of the other).
                // Prefer upstream so remote merges are still visible to integration
                // checks even while local has extra commits.
                upstream
            })
            .clone()
    }

    /// Get the cached integration target for this repository.
    ///
    /// This is the effective target for integration checks (status symbols, safe deletion).
    /// May be upstream (e.g., "origin/main") if it's ahead of local, catching remotely-merged branches.
    ///
    /// Returns None if the default branch cannot be determined.
    ///
    /// Result is cached in the shared repo cache (shared across all worktrees).
    pub fn integration_target(&self) -> Option<String> {
        self.cache
            .integration_target
            .get_or_init(|| {
                let default_branch = self.default_branch()?;
                Some(self.effective_integration_target(&default_branch))
            })
            .clone()
    }

    /// Parse a tree ref to get its SHA (cached).
    pub(super) fn rev_parse_tree(&self, spec: &str) -> anyhow::Result<String> {
        match self.cache.tree_shas.entry(spec.to_string()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let sha = self
                    .run_command(&["rev-parse", spec])
                    .map(|output| output.trim().to_string())?;
                Ok(e.insert(sha).clone())
            }
        }
    }

    /// Resolve a ref to its commit SHA (cached).
    ///
    /// Unlike [`Self::rev_parse_tree`], this returns the commit SHA rather than the
    /// tree SHA. Used by the persistent `sha_cache` to convert ref names into
    /// stable SHA-based cache keys before looking up cached merge-tree results.
    pub(super) fn rev_parse_commit(&self, r: &str) -> anyhow::Result<String> {
        match self.cache.commit_shas.entry(r.to_string()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let sha = self
                    .run_command(&["rev-parse", r])
                    .map(|output| output.trim().to_string())?;
                Ok(e.insert(sha).clone())
            }
        }
    }

    /// Check if a branch is integrated into a target.
    ///
    /// This is a convenience method that combines [`compute_integration_lazy()`] and
    /// [`check_integration()`]. The `target` is transformed via [`Self::effective_integration_target()`]
    /// before checking, which may use an upstream ref if it's ahead of the local target.
    ///
    /// Uses lazy evaluation with short-circuit: stops as soon as any check confirms
    /// integration, avoiding expensive operations like merge simulation when cheaper
    /// checks succeed.
    ///
    /// Returns `(effective_target, reason)` where:
    /// - `effective_target` is the ref actually checked (may be upstream like "origin/main")
    /// - `reason` is `Some(reason)` if integrated, `None` if not
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let (effective_target, reason) = repo.integration_reason("feature", "main")?;
    /// if let Some(r) = reason {
    ///     println!("Branch integrated into {}: {}", effective_target, r.description());
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn integration_reason(
        &self,
        branch: &str,
        target: &str,
    ) -> anyhow::Result<(String, Option<IntegrationReason>)> {
        use dashmap::mapref::entry::Entry;
        match self
            .cache
            .integration_reasons
            .entry((branch.to_string(), target.to_string()))
        {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let effective_target = self.effective_integration_target(target);
                let signals = compute_integration_lazy(self, branch, &effective_target)?;
                let result = (effective_target, check_integration(&signals));
                Ok(e.insert(result).clone())
            }
        }
    }
}
