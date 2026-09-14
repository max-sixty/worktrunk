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

/// Select the comparison-base name from resolved integration targets: the
/// primary target ([`IntegrationTargets::primary`]), falling back to the raw
/// default branch name when targets couldn't be resolved (snapshot capture
/// failed / no upstream relationship known).
///
/// The single home for the "primary-else-default" rule shared by the `wt list`
/// comparison columns (the list collector's `comparison_base`) and the
/// diff/summary preview panes ([`Repository::branch_diff_spec`]) — so the
/// dimmed column number and the pane can't drift onto different bases.
pub fn select_comparison_base<'a>(
    targets: Option<&'a IntegrationTargets>,
    default_branch: Option<&'a str>,
) -> Option<&'a str> {
    targets.map(|t| t.primary.as_str()).or(default_branch)
}

/// Git's well-known empty-tree object. Diffing a commit against it yields the
/// commit's full content as additions — the orphan-branch fallback for the
/// diff/summary preview panes, which can't three-dot-diff a branch that shares
/// no history with the comparison base.
pub(super) const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The upstream-aware base a branch's content is measured against in the
/// diff/summary preview panes — [`IntegrationTargets::primary`], resolved once
/// per repo. Carries both the display name (for "no file changes vs X"
/// messages) and its resolved SHA (for disk-cache keying).
#[derive(Debug, Clone)]
pub(in crate::git) struct ComparisonBase {
    /// Display name of the base (e.g. `main`, or `origin/main` when the local
    /// default lags its upstream).
    pub name: String,
    /// The base's resolved commit SHA. `resolve_comparison_base` returns `None`
    /// rather than a base whose SHA won't resolve, so this is always present.
    pub sha: String,
}

/// How a branch's content is diffed against the mainline for the diff and
/// summary preview panes.
///
/// The base is the **upstream-aware comparison base** — the same ref the
/// ahead/behind and branch-diff columns measure against — so a stale fork
/// default can't make the panes describe upstream commits as the
/// branch's own changes. Orphan branches (no merge base with the base) fall
/// back to a full diff against the empty tree, so a root-style branch shows its
/// content instead of "no file changes". Built by
/// [`Repository::branch_diff_spec`].
#[derive(Debug, Clone)]
pub struct BranchDiffSpec {
    /// The `git diff` revision arguments to splice in after `diff` and any
    /// options: a single three-dot range `["{base}...{head}"]` normally, or
    /// `["{empty-tree}", "{head}"]` for an orphan.
    pub revs: Vec<String>,
    /// Single tree-ish to compare with the worktree when rendering a unified
    /// committed + uncommitted diff: the merge-base SHA normally, or the empty
    /// tree for an orphan. `None` only when merge-base resolution failed; unlike
    /// the committed-only three-dot diff, a worktree diff has no correct range
    /// fallback in that case.
    pub working_base: Option<String>,
    /// Display name of the comparison base, for "no file changes vs X".
    pub base_name: String,
    /// Stable SHA identifying the base for disk-cache keying: the base's commit
    /// SHA, or the empty-tree SHA for an orphan.
    pub cache_sha: String,
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

/// How many candidate commits the patch-id squash-merge fallback diffs on the
/// target side — the oldest ones, nearest the merge-base.
///
/// [`Repository::is_squash_merged_via_patch_id`] runs
/// `git rev-list --reverse {merge-base}..{target} -- <branch paths> | git
/// diff-tree --stdin -p | git patch-id` — one patch per candidate. Candidates
/// are only the target commits that touch a path the branch changed: a commit
/// with the branch's patch-id changes exactly the branch's files, so no other
/// commit can match. The path-limited `rev-list` compares trees but generates
/// no patches, so it stays cheap even when an old branch sits tens of
/// thousands of commits behind a fast-moving tip; the patches are the cost
/// this cap bounds, since a single unbounded check would dominate `wt list`,
/// `wt remove`, and `wt step prune` (the latter goes visibly silent waiting
/// for it).
///
/// Past the cap, the oldest candidates are the ones worth diffing. A squash
/// merge lands on the target soon after the branch forks, and updating the
/// branch before merging (a rebase, a merge from the target) moves the
/// merge-base up to just before it; what piles up behind it is the target's
/// later edits to the same files.
///
/// `500` is conservative because count is only a rough proxy for cost — the
/// per-commit work scales with `changed_files × changed_lines`, so a few
/// hundred lockfile-bump or large-refactor squashes can be slower than a few
/// thousand tiny commits. Working back from "keep one check under a few
/// seconds" on a heavy monorepo (~50-100 KB patches), 500 holds. On a typical
/// repo (~5-20 KB patches), 500 is well under a second.
///
/// # Limitation
///
/// A branch whose squash merge landed after more than
/// `PATCH_ID_SCAN_MAX_COMMITS` other target commits touching its files —
/// counted from the merge-base — is reported as *not* integrated. This is the
/// safe direction — the branch is kept rather than wrongly deleted — and
/// `wt remove -D` still removes it. It takes a branch left un-updated while
/// the target edited its files hundreds of times, then squash-merged as is;
/// by then the target has usually changed the patch's context lines too, so
/// no patch-id would match anyway. Commits to unrelated files, however many,
/// don't count. There is no config knob; bump this constant if the trade-off
/// needs tuning.
const PATCH_ID_SCAN_MAX_COMMITS: usize = 500;

/// Outcome of `git merge-tree --write-tree`, classified by exit code.
///
/// Exit 0 → `Clean` carrying the resulting tree SHA; exit 1 → `Conflict`;
/// anything else is an error (the helper bails). Both merge-tree callers
/// share this dispatch; they differ only in what they do with a clean tree.
/// `Clone` + `Debug` so the outcome can be memoized in
/// [`RepoCache::merge_tree`](super::RepoCache::merge_tree).
#[derive(Debug, Clone)]
pub(in crate::git) enum MergeTreeOutcome {
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

    /// Run `git merge-tree --write-tree <a> <b>`, memoizing the outcome in the
    /// in-memory [`RepoCache::merge_tree`](super::RepoCache::merge_tree) cache.
    ///
    /// `git merge-tree` is the costliest operation in `wt list`, and the two
    /// integration probes ask it the same question per row:
    /// [`Self::run_merge_tree`] wants the conflict bit (for the `WouldConflict`
    /// column) and [`Self::would_merge_add_to_target`] wants the resulting tree
    /// (for the integration column) — both run `merge-tree --write-tree
    /// <target> <branch>`. Their persistent caches (`merge-tree-conflicts` vs
    /// `merge-add-probe`) are keyed separately, so on a cold run neither sees
    /// the other and the subprocess fires twice. The shared entry lock
    /// collapses both — and any cross-row repeat of the same pair — to a single
    /// spawn, mirroring [`Self::merge_base_by_sha`].
    fn merge_tree_outcome(&self, a: &str, b: &str) -> anyhow::Result<MergeTreeOutcome> {
        use dashmap::mapref::entry::Entry;

        // Keyed in call order, not normalized: both dedup call sites pass
        // `(target, branch)`, so they already share a key, and the cached
        // outcome (a conflict flag, or the order-independent clean-merge tree)
        // doesn't depend on argument order — a symmetric key would buy nothing.
        match self.cache.merge_tree.entry((a.to_string(), b.to_string())) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let outcome = self.compute_merge_tree_outcome(a, b)?;
                Ok(e.insert(outcome).clone())
            }
        }
    }

    /// Compute a fresh `git merge-tree --write-tree <a> <b>` outcome, classified
    /// by exit code. Exit 1 → [`MergeTreeOutcome::Conflict`]; anything other
    /// than success is an error; exit 0 → [`MergeTreeOutcome::Clean`] with the
    /// resulting tree SHA (first line of stdout).
    fn compute_merge_tree_outcome(&self, a: &str, b: &str) -> anyhow::Result<MergeTreeOutcome> {
        // Exit codes: 0 = clean merge, 1 = conflicts (git also uses 1 for
        // unresolvable args — callers pass pre-resolved commit SHAs, see
        // `run_merge_tree`), anything else = error (corrupt repo, bad usage)
        let args = ["merge-tree", "--write-tree", a, b];
        let output = self.run_command_output(&args)?;

        if output.status.code() == Some(1) {
            return Ok(MergeTreeOutcome::Conflict);
        }
        if !output.status.success() {
            return Err(crate::git::CommandError::from_failed_output("git", &args, &output).into());
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
    /// since `MergeAddsNothing` handles the non-conflict case. Only target commits
    /// that touch a path the branch changed can match, so only those are diffed,
    /// oldest first and at most [`PATCH_ID_SCAN_MAX_COMMITS`] of them.
    ///
    /// Returns `Ok(true)` if a matching squash-merge commit is found on the target,
    /// `Ok(false)` otherwise (including when the squash merge sits past the oldest
    /// [`PATCH_ID_SCAN_MAX_COMMITS`] candidates, or when patch-id computation
    /// fails — both conservative).
    fn is_squash_merged_via_patch_id(&self, branch: &str, target: &str) -> anyhow::Result<bool> {
        let Some(merge_base) = self.merge_base(target, branch)? else {
            return Ok(false);
        };

        // The paths the branch changed. `diff-tree` without `-M`, like the
        // branch-side patch below, so a rename lists both its old and its new
        // path — exactly the paths that patch covers. NUL-separated and kept
        // as raw bytes so any filename survives: `run_command`'s lossy UTF-8
        // decode would turn a non-UTF-8 name into U+FFFD, a pathspec that
        // matches nothing.
        let args = ["diff-tree", "-r", "-z", "--name-only", &merge_base, branch];
        let output = self.run_command_output(&args)?;
        if !output.status.success() {
            return Err(crate::git::CommandError::from_failed_output("git", &args, &output).into());
        }
        let branch_paths: Vec<&[u8]> = output
            .stdout
            .split(|&b| b == b'\0')
            .filter(|p| !p.is_empty())
            .collect();
        if branch_paths.is_empty() {
            return Ok(false);
        }

        // Candidates: target commits since the merge-base that touch one of
        // those paths. A commit with the branch's patch-id changes exactly the
        // branch's files, so a commit this filter drops can never match — on a
        // fast-moving target that is nearly all of them. `rev-list --stdin`
        // reads the range, then the paths after a `--` line, which keeps a
        // branch that touched thousands of files off the argv.
        // `--literal-pathspecs` stops `*`, `?`, `[` or a leading `:` in a
        // filename from acting as pathspec magic; `--full-history` stops
        // history simplification from dropping a commit that reached the
        // target through the side of a merge, and `--no-merges` then drops
        // the merges themselves: `diff-tree` emits no patch for a merge and a
        // squash merge has one parent, so a merge could only take a cap slot.
        // `--reverse` lists them oldest first, for the cap below. A path
        // containing a line break can't be written one per line, so that
        // branch falls back to every commit in the range.
        let mut rev_list_input = format!("{merge_base}..{target}\n").into_bytes();
        if !branch_paths
            .iter()
            .any(|p| p.iter().any(|b| matches!(b, b'\n' | b'\r')))
        {
            rev_list_input.extend_from_slice(b"--\n");
            for path in &branch_paths {
                rev_list_input.extend_from_slice(path);
                rev_list_input.push(b'\n');
            }
        }
        let candidates = self.run_command_with_stdin(
            &[
                "--literal-pathspecs",
                "rev-list",
                "--full-history",
                "--no-merges",
                "--reverse",
                "--stdin",
            ],
            rev_list_input,
        )?;

        // Bound the patch-id work to the oldest candidates. Every candidate
        // becomes one `diff-tree -p` patch, so a hot file re-touched by
        // thousands of target commits would turn one integration check into
        // seconds of work — visible as `wt step prune` / `wt list` going
        // silent. The path-limited walk above compares trees but builds no
        // patches, so it stays cheap however far behind the branch is. The
        // cut happens here, not with `--max-count`, which git applies before
        // `--reverse` and so would keep the newest. Why the oldest: see
        // [`PATCH_ID_SCAN_MAX_COMMITS`].
        let target_commits: String = candidates
            .split_inclusive('\n')
            .take(PATCH_ID_SCAN_MAX_COMMITS)
            .collect();
        let candidate_count = candidates.lines().count();
        if candidate_count > PATCH_ID_SCAN_MAX_COMMITS {
            tracing::debug!(
                candidate_count,
                merge_base = %merge_base,
                target = %target,
                "patch-id squash-merge check: {candidate_count} commits in {merge_base}..{target} touch the branch's paths; diffing the oldest {PATCH_ID_SCAN_MAX_COMMITS}"
            );
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
        // the commit list on stdin and emits one diff per commit. No pathspec
        // here: each candidate's patch must cover its whole change, or a
        // commit that also touched other files would falsely match.
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

    /// The upstream-aware comparison base for the diff/summary preview panes,
    /// resolved once and memoized (the base is repo-wide, like the default
    /// branch). Returns `None` when no default branch can be determined.
    ///
    /// Selects the base via the shared [`select_comparison_base`] rule
    /// ([`IntegrationTargets::primary`] else the raw local default), so the
    /// preview panes and the `wt list` columns can't pick different bases.
    /// Captures a [`RefSnapshot`] on first call unless already primed via
    /// [`Self::prime_comparison_base`]; safe in the read-only preview contexts
    /// that use it (wt doesn't move refs there). Fully local — no network.
    fn comparison_base(&self) -> Option<&ComparisonBase> {
        self.cache
            .comparison_base
            .get_or_init(|| self.resolve_comparison_base())
            .as_ref()
    }

    /// Seed the memoized comparison base from an already-captured snapshot, so
    /// the diff/summary preview panes reuse the `wt list` collector's ref scan
    /// instead of capturing their own (both share one `Arc<RepoCache>`).
    /// Idempotent — a no-op once the base is resolved; the lazy
    /// `comparison_base` path remains the fallback for callers that preview
    /// without collecting first.
    pub fn prime_comparison_base(&self, snapshot: &RefSnapshot) {
        self.cache
            .comparison_base
            .get_or_init(|| self.comparison_base_from(snapshot));
    }

    fn resolve_comparison_base(&self) -> Option<ComparisonBase> {
        // No default branch → no comparison base; skip the ref scan entirely.
        self.default_branch()?;
        let snapshot = self.capture_refs().ok()?;
        self.comparison_base_from(&snapshot)
    }

    /// Resolve the comparison base from a captured snapshot. The shared core of
    /// the lazy [`Self::resolve_comparison_base`] (captures its own snapshot)
    /// and [`Self::prime_comparison_base`] (reuses the collector's), so both
    /// select the base by the same [`select_comparison_base`] rule.
    fn comparison_base_from(&self, snapshot: &RefSnapshot) -> Option<ComparisonBase> {
        let targets = self.integration_targets(snapshot);
        let default_branch = self.default_branch();
        let name = select_comparison_base(targets.as_ref(), default_branch.as_deref())?.to_string();
        // No usable base if the name won't resolve (a stale
        // `worktrunk.default-branch` pointing at a deleted branch) — return
        // `None` so the panes skip the branch diff rather than render a
        // guaranteed-empty range against a missing ref.
        let sha = snapshot_resolve(self, snapshot, &name).ok()?;
        Some(ComparisonBase { name, sha })
    }

    /// Resolve how to diff a branch's content against the mainline for the
    /// diff/summary preview panes. `head` is a resolved commit SHA. See
    /// [`BranchDiffSpec`]. Returns `None` when there's no comparison base (no
    /// default branch, or its ref won't resolve).
    pub fn branch_diff_spec(&self, head: &str) -> Option<BranchDiffSpec> {
        let base = self.comparison_base()?;
        let base_sha = base.sha.as_str();

        // Orphan (no merge base with the base): a three-dot range is
        // ill-defined, so diff the full content against the empty tree. A
        // merge-base *error* (invalid/unreachable object) is NOT an orphan —
        // fall through to the normal three-dot range and let the diff surface
        // the failure rather than dumping the whole tree as "the branch".
        let (revs, working_base, cache_sha) = match self.merge_base_by_sha(base_sha, head) {
            Ok(None) => (
                vec![EMPTY_TREE_SHA.to_string(), head.to_string()],
                Some(EMPTY_TREE_SHA.to_string()),
                EMPTY_TREE_SHA.to_string(),
            ),
            Ok(Some(merge_base)) => (
                vec![format!("{base_sha}...{head}")],
                Some(merge_base),
                base_sha.to_string(),
            ),
            Err(_) => (
                vec![format!("{base_sha}...{head}")],
                None,
                base_sha.to_string(),
            ),
        };

        Some(BranchDiffSpec {
            revs,
            working_base,
            base_name: base.name.clone(),
            cache_sha,
        })
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
    /// On the `wt list` path this cache is **primed in bulk** by
    /// [`Self::commit_details_many`] (the pre-skeleton `git log` batch reads
    /// `%T` for every item head + the default-branch tip), so the per-row
    /// lookups here are memory hits and fire no subprocess at all. This method
    /// remains the fallback for any SHA the batch didn't cover.
    ///
    /// In-memory rather than the persistent `sha_cache`: on the hot path the
    /// batch above already resolves it fork-free, so disk persistence would
    /// save only the rare off-batch miss. The `Entry` match holds the shard
    /// lock across check-and-insert so the first miss computes once and every
    /// concurrent row reads it — no cold-start race.
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

    /// The target's upstream, when the current branch's history extends past
    /// the local `target_branch` ref into it — the #3519 stale-local-target
    /// topology.
    ///
    /// The rewrite steps (`wt step squash`, `wt step rebase`, and `wt merge`
    /// through them) measure "the branch's own commits" as
    /// `merge-base(target, HEAD)..HEAD`. When the local target ref lags its
    /// upstream (e.g. the primary checkout's `main` left behind `origin/main`)
    /// and the branch was built on the newer upstream tip, that span sweeps in
    /// commits the upstream already contains — folding (squash) or replaying
    /// (rebase) them would duplicate published history under new SHAs. Callers
    /// measure against the returned upstream instead, which keeps the span to
    /// the branch's own commits; `wt merge`'s final fast-forward then carries
    /// the local target through the upstream commits by their real SHAs.
    ///
    /// Detected locally, with no fetch (worktrunk is local-first): returns
    /// `Some(upstream)` (short name, e.g. `origin/main`) when the target branch
    /// has a configured upstream and `merge-base(HEAD, upstream)` is not an
    /// ancestor of the local `target` ref — i.e. the local-target span would
    /// reach commits the upstream already has. `None` — measure against the
    /// local target as usual — for a legitimately-diverged local target whose
    /// span holds only the branch's own commits (the fork point is still an
    /// ancestor of the local target), an orphan branch with no shared history,
    /// a non-branch target (an explicit `origin/main` has no upstream of its
    /// own), or a target with no upstream at all.
    pub fn span_upstream(&self, target_branch: &str) -> anyhow::Result<Option<String>> {
        // `upstream()` already excludes a `[gone]` upstream, so a `Some` value
        // is a live remote-tracking ref that `merge_base` resolves locally — no
        // fetch, and no separate existence check.
        let Some(upstream) = self.branch(target_branch).upstream()? else {
            return Ok(None);
        };
        let target_sha = self.resolve_to_commit_sha(target_branch)?;
        match self.merge_base("HEAD", &upstream)? {
            Some(fork_point) if !self.is_ancestor_by_sha(&fork_point, &target_sha)? => {
                Ok(Some(upstream))
            }
            _ => Ok(None),
        }
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

    /// What a run of padding commits on the target changes.
    #[derive(Clone, Copy)]
    enum Padding {
        /// Nothing — each pad reuses its parent's tree.
        Empty,
        /// `other`, a file the feature never touches.
        OtherFile,
        /// The feature's own file.
        SameFile,
    }

    /// `n` padding commits of one kind.
    #[derive(Clone, Copy)]
    struct Pads(usize, Padding);

    const NO_PADS: Pads = Pads(0, Padding::Empty);

    /// Build the topology
    ///
    /// ```text
    /// base ─── feature  (path: A → B)
    ///   └────── before… ── squash ── after…  = target
    /// ```
    ///
    /// via a single `git fast-import` stream — instant even at N = 2000, where
    /// running `git commit` once per pad would dominate the test.
    /// `merge_base(target, feature)` is `base`; `squash`'s combined patch
    /// equals `feature`'s. A touching pad alternates its file between `C` and
    /// `D`, so every pad is a real change to it. `SameFile` pads before the
    /// squash are followed by one more commit restoring `A`, so the squash is
    /// still `A → B`.
    ///
    /// `path` is written into the stream as is, so a name fast-import can't
    /// take bare — a line break, non-UTF-8 bytes — goes in C-quoted
    /// (`"\351t\351.txt"`).
    fn build(test: &TestRepo, path: &str, before: Pads, after: Pads) {
        let mut s = String::new();
        s.push_str("blob\nmark :10\ndata 1\nA\n");
        s.push_str("blob\nmark :11\ndata 1\nB\n");
        s.push_str("blob\nmark :12\ndata 1\nC\n");
        s.push_str("blob\nmark :13\ndata 1\nD\n");
        writeln!(
            s,
            "commit refs/heads/base\nmark :1\ncommitter T <t@x> 1700000000 +0000\ndata 0\nM 100644 :10 {path}\n"
        )
        .unwrap();
        writeln!(
            s,
            "commit refs/heads/feature\nmark :2\ncommitter T <t@x> 1700000001 +0000\ndata 0\nfrom :1\nM 100644 :11 {path}\n"
        )
        .unwrap();

        let pads = move |Pads(n, padding): Pads| {
            (0..n).map(move |i| {
                let blob = 12 + i % 2;
                match padding {
                    Padding::Empty => String::new(),
                    Padding::OtherFile => format!("M 100644 :{blob} other\n"),
                    Padding::SameFile => format!("M 100644 :{blob} {path}\n"),
                }
            })
        };
        let mut target_changes: Vec<String> = pads(before).collect();
        if matches!(before, Pads(1.., Padding::SameFile)) {
            target_changes.push(format!("M 100644 :10 {path}\n"));
        }
        target_changes.push(format!("M 100644 :11 {path}\n")); // the squash
        target_changes.extend(pads(after));

        // Target marks start at :100, clear of the blob marks :10–:13.
        for (i, change) in target_changes.iter().enumerate() {
            let mark = 100 + i;
            let parent = if i == 0 { 1 } else { mark - 1 };
            writeln!(
                s,
                "commit refs/heads/target\nmark :{mark}\ncommitter T <t@x> {} +0000\ndata 0\nfrom :{parent}\n{change}",
                1_700_000_002 + i
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

    fn is_squash_merged(test: &TestRepo) -> bool {
        let repo = Repository::at(test.root_path()).unwrap();
        let feature = rev_parse(&repo, "refs/heads/feature");
        let target = rev_parse(&repo, "refs/heads/target");
        repo.is_squash_merged_via_patch_id(&feature, &target)
            .unwrap()
    }

    #[test]
    fn detects_squash_when_range_is_under_cap() {
        let test = TestRepo::new();
        // base..target = 2 commits (squash + one pad). Well under the cap.
        build(&test, "file", NO_PADS, Pads(1, Padding::Empty));

        assert!(
            is_squash_merged(&test),
            "squash commit's patch-id matches feature's; should be detected within the cap"
        );
    }

    #[test]
    fn detects_squash_among_oldest_candidates_past_cap() {
        let test = TestRepo::new();
        // The squash plus PATCH_ID_SCAN_MAX_COMMITS pads, every one touching
        // the feature's file — one candidate over the cap. The squash is the
        // oldest, so it is among the candidates diffed.
        build(
            &test,
            "file",
            NO_PADS,
            Pads(PATCH_ID_SCAN_MAX_COMMITS, Padding::SameFile),
        );

        assert!(
            is_squash_merged(&test),
            "later commits to the same file must not hide the squash past the cap"
        );
    }

    #[test]
    fn misses_squash_past_oldest_candidates() {
        let test = TestRepo::new();
        // PATCH_ID_SCAN_MAX_COMMITS pads touching the feature's file (plus
        // the one restoring `A`) land before the squash, which pushes it past
        // the oldest candidates we diff. The squash is still on the target;
        // we just don't look that far. Asserting `false` while
        // `detects_squash_among_oldest_candidates_past_cap` asserts `true`
        // pins the difference to where the squash sits.
        build(
            &test,
            "file",
            Pads(PATCH_ID_SCAN_MAX_COMMITS, Padding::SameFile),
            NO_PADS,
        );

        assert!(
            !is_squash_merged(&test),
            "should not diff past the oldest {PATCH_ID_SCAN_MAX_COMMITS} commits touching the branch's paths"
        );
    }

    #[test]
    fn detects_squash_behind_commits_to_other_paths() {
        let test = TestRepo::new();
        // PATCH_ID_SCAN_MAX_COMMITS commits to `other` land between the fork
        // and the squash — a busy default branch whose traffic is elsewhere.
        // Only the squash touches the feature's file, so it is the only
        // candidate. Unfiltered, it would sit past the oldest
        // PATCH_ID_SCAN_MAX_COMMITS and go undetected.
        build(
            &test,
            "file",
            Pads(PATCH_ID_SCAN_MAX_COMMITS, Padding::OtherFile),
            NO_PADS,
        );

        assert!(
            is_squash_merged(&test),
            "commits that don't touch the branch's paths must not count toward the cap"
        );
    }

    #[test]
    fn branch_paths_are_literal_not_pathspec_magic() {
        let test = TestRepo::new();
        // As a pathspec, `[o]the[r]` matches `other` too, which would make
        // every pad a candidate and push the squash past the oldest
        // PATCH_ID_SCAN_MAX_COMMITS. Taken literally it matches only the
        // file named `[o]the[r]`. Not `*`, which pins the same thing but which
        // git on Windows rejects outright: `fatal: invalid path '*'`.
        build(
            &test,
            "[o]the[r]",
            Pads(PATCH_ID_SCAN_MAX_COMMITS, Padding::OtherFile),
            NO_PADS,
        );

        assert!(
            is_squash_merged(&test),
            "a filename must be matched literally, not as a glob"
        );
    }

    // Git for Windows refuses control characters in paths, and a Windows
    // checkout can't hold a non-UTF-8 name, so these two are Unix-only.

    #[test]
    #[cfg(unix)]
    fn non_utf8_branch_paths_still_filter() {
        let test = TestRepo::new();
        // `été.txt` in Latin-1 — the literal below is `\351t\351.txt`, and
        // a lone `\351` byte is not valid UTF-8. A lossy decode would pass
        // U+FFFD to `rev-list`, filter the squash out, and miss it; with the
        // pads on `other`, falling back to the whole range would push it past
        // the cap too. Only the raw bytes find it.
        build(
            &test,
            r#""\351t\351.txt""#,
            Pads(PATCH_ID_SCAN_MAX_COMMITS, Padding::OtherFile),
            NO_PADS,
        );

        assert!(
            is_squash_merged(&test),
            "a non-UTF-8 filename must reach the path filter byte for byte"
        );
    }

    #[test]
    #[cfg(unix)]
    fn branch_path_with_line_break_scans_whole_range() {
        let test = TestRepo::new();
        // Written one per line, `new\nline` would become the two pathspecs
        // `new` and `line`, which match nothing — so the branch falls back
        // to every commit in the range, which finds the squash.
        build(&test, r#""new\nline""#, NO_PADS, NO_PADS);

        assert!(
            is_squash_merged(&test),
            "a path with a line break must fall back to the whole range, not a split pathspec"
        );
    }

    #[test]
    fn branch_that_changes_nothing_is_not_squash_merged() {
        let test = TestRepo::new();
        std::fs::write(test.path().join("file"), "A\n").unwrap();
        test.run_git(&["add", "file"]);
        test.run_git(&["commit", "-m", "base"]);
        test.run_git(&["checkout", "-b", "feature"]);
        test.run_git(&["commit", "--allow-empty", "-m", "no change"]);
        test.run_git(&["checkout", "-b", "target", "main"]);
        test.run_git(&["commit", "--allow-empty", "-m", "also no change"]);

        assert!(
            !is_squash_merged(&test),
            "a branch with no changes has no patch to match"
        );
    }

    /// A target commit that makes the branch's change *and* an unrelated one
    /// isn't a squash merge of that branch. The path filter picks such a
    /// commit as a candidate, so its patch must still cover its whole change
    /// — a diff narrowed to the branch's paths would match.
    #[test]
    fn rejects_target_commit_with_extra_changes() {
        let test = TestRepo::new();
        let path = test.path().join("file");
        std::fs::write(&path, "A\n").unwrap();
        test.run_git(&["add", "file"]);
        test.run_git(&["commit", "-m", "base"]);

        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(&path, "B\n").unwrap();
        test.run_git(&["commit", "-am", "feature change"]);

        test.run_git(&["checkout", "-b", "target", "main"]);
        std::fs::write(&path, "B\n").unwrap();
        std::fs::write(test.path().join("other"), "unrelated\n").unwrap();
        test.run_git(&["add", "file", "other"]);
        test.run_git(&["commit", "-m", "feature change plus something else"]);

        assert!(
            !is_squash_merged(&test),
            "a target commit with changes beyond the branch's must not match"
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

#[cfg(test)]
mod merge_tree_error_tests {
    use super::*;
    use crate::testing::TestRepo;

    /// A hard `git merge-tree` failure (here: a fatal config error, exit
    /// 128) must surface as a typed `CommandError`, distinct from exit 1,
    /// which means "merged with conflicts".
    #[test]
    fn hard_failure_is_command_error() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        std::fs::write(test.root_path().join(".git/config"), "[bad\n").unwrap();

        let err = repo.compute_merge_tree_outcome("HEAD", "HEAD").unwrap_err();
        let cmd_err =
            crate::git::CommandError::find_in(&err).expect("error should carry a CommandError");
        assert_eq!(
            cmd_err.command_string(),
            "git merge-tree --write-tree HEAD HEAD"
        );
    }
}

#[cfg(test)]
mod merge_tree_cache_tests {
    use super::*;
    use crate::testing::TestRepo;

    /// The conflict probe ([`Repository::has_merge_conflicts_by_sha`], driving
    /// the `WouldConflict` column) and the integration probe
    /// ([`Repository::merge_integration_probe_by_sha`], driving the
    /// integration column) both run `git merge-tree --write-tree <target>
    /// <branch>` for the same row but extract different answers — the conflict
    /// bit versus the resulting tree. They must key the in-memory cache
    /// identically, in `(target, branch)` order, so the subprocess fires once
    /// per row rather than once per probe. An argument-order regression on
    /// either call site would split this into two entries (two spawns); this
    /// pins it at one.
    #[test]
    fn conflict_and_integration_probes_share_one_merge_tree_spawn() {
        let test = TestRepo::with_initial_commit();

        // feature: one commit ahead of main, clean-mergeable, not integrated.
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("new.txt"), "content\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "feature"]);
        test.run_git(&["checkout", "main"]);

        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);

        let repo = Repository::at(test.root_path()).unwrap();

        // Conflict probe: a clean merge has no conflicts.
        assert!(
            !repo
                .has_merge_conflicts_by_sha(&main_sha, &feature_sha)
                .unwrap()
        );
        // Integration probe: merging adds new.txt, so it would change main —
        // not yet integrated, and no patch-id fallback (merge was clean).
        let probe = repo
            .merge_integration_probe_by_sha(&feature_sha, &main_sha)
            .unwrap();
        assert!(probe.would_merge_add);
        assert!(!probe.is_patch_id_match);

        // Both probes resolved through a single shared merge-tree entry, keyed
        // (target, branch) = (main, feature).
        assert_eq!(
            repo.cache.merge_tree.len(),
            1,
            "conflict + integration probes must share one merge-tree cache entry"
        );
        assert!(
            repo.cache.merge_tree.contains_key(&(main_sha, feature_sha)),
            "the shared entry must be keyed (target, branch)"
        );
    }
}

#[cfg(test)]
mod read_only_object_store_tests {
    use super::*;
    use crate::testing::TestRepo;

    /// A cleanly-mergeable but diverged topology: `main` and `feature` touch
    /// different files off a shared base, so `merge-tree --write-tree` must
    /// write a genuinely new tree (not a fast-forward that reuses an existing
    /// one). Returns `(main_sha, feature_sha)`.
    fn diverged_repo() -> (TestRepo, String, String) {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["checkout", "-b", "feature"]);
        std::fs::write(test.root_path().join("feature.txt"), "feature\n").unwrap();
        test.run_git(&["add", "feature.txt"]);
        test.run_git(&["commit", "-m", "feature"]);
        test.run_git(&["checkout", "main"]);
        std::fs::write(test.root_path().join("main.txt"), "main\n").unwrap();
        test.run_git(&["add", "main.txt"]);
        test.run_git(&["commit", "-m", "main"]);
        let main_sha = test.git_output(&["rev-parse", "main"]);
        let feature_sha = test.git_output(&["rev-parse", "feature"]);
        (test, main_sha, feature_sha)
    }

    #[cfg(unix)]
    #[test]
    fn redirected_objects_resolve_a_repo_path_containing_a_colon() {
        let parent = crate::testing::test_tempdir();
        let test = TestRepo::at(&parent.path().join("repo:with-colon"));
        test.commit("Initial commit");

        let redirected = test.repo.redirect_objects_for_observation().unwrap();
        assert!(
            redirected
                .run_command_check(&["cat-file", "-e", "HEAD"])
                .unwrap(),
            "the real object database must remain readable through the redirect"
        );
    }

    #[cfg(unix)]
    #[test]
    fn redirected_objects_resolve_a_repo_path_containing_a_newline() {
        let parent = crate::testing::test_tempdir();
        let test = TestRepo::at(&parent.path().join("repo\nwith-newline"));
        test.commit("Initial commit");

        let redirected = test.repo.redirect_objects_for_observation().unwrap();
        assert!(
            redirected
                .run_command_check(&["cat-file", "-e", "HEAD"])
                .unwrap(),
            "the real object database must remain readable through the redirect"
        );
    }

    #[test]
    fn redirected_object_directory_unregisters_on_drop() {
        let test = TestRepo::with_initial_commit();
        let redirected = test.repo.redirect_objects_for_observation().unwrap();
        let directory = redirected
            .object_store_environment()
            .unwrap()
            .0
            .to_path_buf();
        assert!(
            test.repo
                .cache
                .observation_object_directories
                .contains_key(&directory)
        );

        drop(redirected);

        assert!(
            !test
                .repo
                .cache
                .observation_object_directories
                .contains_key(&directory)
        );
        assert!(!directory.exists());
    }

    /// The safety property behind scoping the redirect to observational
    /// commands: a redirected merge tree is computed correctly but written only
    /// to the throwaway store, so it is *not* in the real object database. A
    /// mutating command that redirected would therefore lose its commit — which
    /// is why mutating commands keep the persistent path and fail loudly on a
    /// read-only store instead.
    #[test]
    fn redirected_object_writes_stay_out_of_the_real_database() {
        let (test, main_sha, feature_sha) = diverged_repo();
        let merge_tree = |repo: &Repository| {
            repo.run_command(&["merge-tree", "--write-tree", &main_sha, &feature_sha])
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .trim()
                .to_string()
        };

        // Redirected clone: the merge tree lands in the temporary store only.
        let redirected = Repository::at(test.root_path())
            .unwrap()
            .redirect_objects_for_observation()
            .unwrap();
        let ephemeral_tree = merge_tree(&redirected);

        // A separate, non-redirected repository reads the real database
        // directly; the redirected tree is absent there.
        let real = Repository::at(test.root_path()).unwrap();
        assert!(
            !real
                .run_command_check(&["cat-file", "-e", &ephemeral_tree])
                .unwrap(),
            "a redirected merge tree must not be written to the real object database"
        );

        // The persistent path yields the same content-addressed tree, this time
        // materialized in the real database.
        let persistent_tree = merge_tree(&real);
        assert_eq!(persistent_tree, ephemeral_tree);
        assert!(
            real.run_command_check(&["cat-file", "-e", &persistent_tree])
                .unwrap(),
            "the persistent path must materialize its merge tree in the real database"
        );
    }
}
