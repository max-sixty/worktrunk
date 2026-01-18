//! Integration detection operations for Repository.
//!
//! Methods for determining if a branch has been integrated into the target
//! (same commit, ancestor, trees match, etc.).

use super::Repository;
use crate::git::{IntegrationReason, check_integration, compute_integration_lazy};

impl Repository {
    /// Resolve a ref, preferring branches over tags when names collide.
    ///
    /// Uses git to check if `refs/heads/{ref}` exists. If so, returns the
    /// qualified form to ensure we reference the branch, not a same-named tag.
    /// Otherwise returns the original ref unchanged (for HEAD, SHAs, remote refs).
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

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    ///
    /// See [`--is-ancestor`][1] for details.
    ///
    /// [1]: https://git-scm.com/docs/git-merge-base#Documentation/git-merge-base.txt---is-ancestor
    pub fn is_ancestor(&self, base: &str, head: &str) -> anyhow::Result<bool> {
        let base = self.resolve_preferring_branch(base);
        let head = self.resolve_preferring_branch(head);
        self.run_command_check(&["merge-base", "--is-ancestor", &base, &head])
    }

    /// Check if two refs point to the same commit.
    pub fn same_commit(&self, ref1: &str, ref2: &str) -> anyhow::Result<bool> {
        let ref1 = self.resolve_preferring_branch(ref1);
        let ref2 = self.resolve_preferring_branch(ref2);
        // Parse both refs in a single git command
        let output = self.run_command(&["rev-parse", &ref1, &ref2])?;
        let mut lines = output.lines();
        let sha1 = lines.next().unwrap_or_default().trim();
        let sha2 = lines.next().unwrap_or_default().trim();
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
        // Try to get merge-base (cached). Orphan branches return None.
        let Some(merge_base) = self.merge_base(&target, &branch)? else {
            // Orphan branches have no common ancestor, so all their changes are unique
            return Ok(true);
        };

        // git diff --name-only merge_base..branch shows files changed from merge-base to branch
        let range = format!("{merge_base}..{branch}");
        let output = self.run_command(&["diff", "--name-only", &range])?;
        Ok(!output.trim().is_empty())
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
        let tree1 = lines.next().unwrap_or_default().trim();
        let tree2 = lines.next().unwrap_or_default().trim();
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
        // Use modern merge-tree --write-tree mode which exits with 1 when conflicts exist
        // (the old 3-argument deprecated mode always exits with 0)
        // run_command_check returns true for exit 0, false otherwise
        let clean_merge = self.run_command_check(&["merge-tree", "--write-tree", &base, &head])?;
        Ok(!clean_merge)
    }

    /// Check if merging a branch into target would add anything (not already integrated).
    ///
    /// Uses `git merge-tree` to simulate merging the branch into the target. If the
    /// resulting tree matches the target's tree, then merging would add nothing,
    /// meaning the branch's content is already integrated.
    ///
    /// This handles cases that simple tree comparison misses:
    /// - Squash-merged branches where main has advanced with additional commits
    /// - Rebased branches where the base has moved forward
    ///
    /// Returns:
    /// - `Ok(true)` if merging would change the target (branch has unintegrated changes)
    /// - `Ok(false)` if merging would NOT change target (branch is already integrated)
    /// - `Ok(true)` if merge would have conflicts (conservative: treat as not integrated)
    /// - `Err` if git commands fail
    pub fn would_merge_add_to_target(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        let branch = self.resolve_preferring_branch(branch);
        let target = self.resolve_preferring_branch(target);
        // Simulate merging branch into target
        // On conflict, merge-tree exits non-zero and we can't get a clean tree
        let merge_result = self.run_command(&["merge-tree", "--write-tree", &target, &branch]);

        let Ok(merge_tree) = merge_result else {
            // merge-tree failed (likely conflicts) - conservatively treat as having changes
            return Ok(true);
        };

        let merge_tree = merge_tree.trim();
        if merge_tree.is_empty() {
            // Empty output is unexpected - treat as having changes
            return Ok(true);
        }

        // Get target's tree for comparison
        let target_tree = self.rev_parse_tree(&format!("{target}^{{tree}}"))?;

        // If merge result differs from target's tree, merging would add something
        Ok(merge_tree != target_tree)
    }

    /// Determine the effective target for integration checks.
    ///
    /// If the upstream of the local target (e.g., `origin/main`) is strictly ahead of
    /// the local target (i.e., local is an ancestor of upstream but not the same commit),
    /// uses the upstream. This handles the common case where a branch was merged remotely
    /// but the user hasn't pulled yet.
    ///
    /// When local and upstream are the same commit, prefers local for clearer messaging.
    ///
    /// Returns the effective target ref to check against.
    ///
    /// Used by both `wt list` and `wt remove` to ensure consistent integration detection.
    ///
    /// TODO(future): When local and remote have diverged (neither is ancestor),
    /// check integration against both and delete only if integrated into both.
    /// Current behavior: uses only local in diverged state, may miss remote-merged branches.
    pub fn effective_integration_target(&self, local_target: &str) -> String {
        // Get the upstream ref for the local target (e.g., origin/main for main)
        let upstream = match self.branch(local_target).upstream() {
            Ok(Some(upstream)) => upstream,
            _ => return local_target.to_string(),
        };

        // If local and upstream are the same commit, prefer local for clearer messaging
        if self.same_commit(local_target, &upstream).unwrap_or(false) {
            return local_target.to_string();
        }

        // Check if local is strictly behind upstream (local is ancestor of upstream)
        // This means upstream has commits that local doesn't have
        // On error, fall back to local target (defensive: don't fail due to git errors)
        if self.is_ancestor(local_target, &upstream).unwrap_or(false) {
            return upstream;
        }

        local_target.to_string()
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

    /// Parse a tree ref to get its SHA.
    pub(super) fn rev_parse_tree(&self, spec: &str) -> anyhow::Result<String> {
        self.run_command(&["rev-parse", spec])
            .map(|output| output.trim().to_string())
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
        let effective_target = self.effective_integration_target(target);
        let signals = compute_integration_lazy(self, branch, &effective_target)?;
        Ok((effective_target, check_integration(&signals)))
    }
}
