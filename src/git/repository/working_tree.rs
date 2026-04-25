//! WorkingTree - a borrowed handle for worktree-specific git operations.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use dashmap::mapref::entry::Entry;

use crate::shell_exec::Cmd;
use dunce::canonicalize;

use super::{GitError, LineDiff, Repository};

/// Parse `git submodule status` output and detect whether any submodule is initialized.
///
/// Status lines start with a one-character state marker:
/// - `-` = not initialized
/// - ` ` / `+` / `U` = initialized variants
fn has_initialized_submodules_from_status(status: &str) -> bool {
    status.lines().any(|line| match line.chars().next() {
        Some('-') | None => false,
        Some(_) => true,
    })
}

/// Typed snapshot returned by [`WorkingTree::prewarm_info`].
///
/// Mirrors what the batched `git rev-parse` actually resolved so callers can
/// read the data directly instead of round-tripping through the per-field
/// cache accessors. `prewarm_info` still primes [`RepoCache`] so later calls
/// to [`WorkingTree::branch`], [`WorkingTree::root`], [`WorkingTree::git_dir`]
/// and [`WorkingTree::head_sha`] remain single cache hits.
///
/// When `is_inside` is false every other field is `None` — nothing else ran.
/// When `is_inside` is true, `root` lands unconditionally and `git_dir` lands
/// unless canonicalization failed. HEAD-derived fields (`current_branch`,
/// `head_sha`) are only populated when the whole batch succeeded; on unborn
/// HEAD they stay `None` and the per-accessor fallback paths handle the
/// symbolic-ref lookup.
///
/// [`RepoCache`]: super::RepoCache
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct WorkingTreeGitInfo {
    /// Whether this path sits inside a git work tree (false for bare repo roots).
    pub is_inside: bool,
    /// Canonicalized top-level directory. Always `Some` when `is_inside`.
    pub root: Option<PathBuf>,
    /// Canonicalized git directory (may differ from common dir in linked
    /// worktrees). `Some` when `is_inside` and canonicalization succeeded.
    pub git_dir: Option<PathBuf>,
    /// Current branch: outer `Some(Some(name))` on a branch, `Some(None)`
    /// detached. `None` when HEAD was unresolvable (unborn branch) or outside
    /// a work tree.
    pub current_branch: Option<Option<String>>,
    /// HEAD commit SHA. `None` on unborn branches or outside a work tree.
    pub head_sha: Option<String>,
}

/// Get a short display name for a path, used in logging context.
pub fn path_to_logging_context(path: &Path) -> String {
    if path.to_str() == Some(".") {
        ".".to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string()
    }
}

/// A borrowed handle for running git commands in a specific worktree.
///
/// This type borrows a [`Repository`] and holds a path to a specific worktree.
/// All worktree-specific operations (like `branch`, `is_dirty`) are on this type.
///
/// For an owned equivalent that can be cloned across threads, see [`super::super::BranchRef`].
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current()?;
/// let wt = repo.current_worktree();
///
/// // Worktree-specific operations
/// let _ = wt.is_dirty();
/// let _ = wt.branch();
///
/// // View at a different worktree
/// let _other = repo.worktree_at("/path/to/other/worktree");
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug)]
#[must_use]
pub struct WorkingTree<'a> {
    pub(super) repo: &'a Repository,
    pub(super) path: PathBuf,
}

impl<'a> WorkingTree<'a> {
    /// Get a reference to the repository this worktree belongs to.
    pub fn repo(&self) -> &Repository {
        self.repo
    }

    /// Get the path this WorkingTree was created with.
    ///
    /// Returns the canonicalized form when the input passed to `worktree_at()` /
    /// `base_path()` for `current_worktree()` exists on disk; otherwise returns
    /// the raw input. So on macOS, a temp path like `/tmp/foo` may surface here
    /// (and to hook template variables) as `/private/tmp/foo`.
    ///
    /// For the canonical git-determined root, use [`root()`](Self::root) instead.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a git command in this worktree and return stdout.
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = self.run_command_output(args)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.replace('\r', "\n");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let error_msg = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("{}", error_msg);
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(stdout)
    }

    /// Run a git command in this worktree and return the raw Output.
    ///
    /// Use this when you need to check exit codes directly (e.g., for commands
    /// where non-zero exit is not an error condition).
    pub fn run_command_output(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        Cmd::new("git")
            .args(args.iter().copied())
            .current_dir(&self.path)
            .context(path_to_logging_context(&self.path))
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))
    }

    // =========================================================================
    // Worktree-specific methods
    // =========================================================================

    /// Pre-warm the worktree caches with a single batched `git rev-parse` and
    /// return a snapshot of what it resolved.
    ///
    /// Folds five rev-parse selectors that would otherwise fire as separate
    /// forks during alias/hook dispatch (`--is-inside-work-tree` from
    /// [`Repository::project_config_path`], plus [`Self::root`], [`Self::git_dir`],
    /// [`Self::head_sha`], and [`Self::branch`]) into one. Bare `HEAD` sits
    /// before `--symbolic-full-name HEAD` because `--symbolic-full-name` is a
    /// sticky mode flag — emitting the unqualified `HEAD` first gives us a
    /// commit SHA, then the mode switch makes the second `HEAD` resolve as a
    /// ref name.
    ///
    /// `--show-toplevel` and `--git-dir` succeed even on unborn branches —
    /// rev-parse prints them before HEAD errors — so `root`/`git_dir` are
    /// cached whenever we're inside a work tree. `head_sha` and
    /// `current_branch` are only populated when the whole batch succeeded,
    /// so [`Self::branch`]'s `symbolic-ref` fallback still handles genuine
    /// unborn HEADs. On unborn the symbolic-full-name line lands as a
    /// fallback literal "HEAD", which would be indistinguishable from
    /// detached HEAD without the exit status.
    ///
    /// Idempotent within a single command (for paths inside a work tree):
    /// once `worktree_roots` is primed — by this method or by [`Self::root`]
    /// — subsequent calls reconstruct the snapshot from the primed caches
    /// without spawning a subprocess. Bare-repo roots and paths outside any
    /// work tree intentionally aren't memoized; repeat calls there re-run the
    /// batch, but such callers typically invoke `prewarm_info` only once.
    ///
    /// [`Repository::project_config_path`]: super::Repository::project_config_path
    pub fn prewarm_info(&self) -> anyhow::Result<WorkingTreeGitInfo> {
        // Fast path: `worktree_roots` only lands on confirmed toplevels (both
        // `root()` and this method skip the cache on failure), so its presence
        // means we've already resolved this path as inside a work tree —
        // reconstruct the snapshot from the caches instead of spawning another
        // `git rev-parse`. Fields with no cache entry stay `None`, matching
        // the semantics of a freshly-run batch on unborn HEAD (where
        // HEAD-derived entries never land).
        if let Some(root) = self
            .repo
            .cache
            .worktree_roots
            .get(&self.path)
            .map(|e| e.clone())
        {
            return Ok(WorkingTreeGitInfo {
                is_inside: true,
                root: Some(root),
                git_dir: self.repo.cache.git_dirs.get(&self.path).map(|e| e.clone()),
                current_branch: self
                    .repo
                    .cache
                    .current_branches
                    .get(&self.path)
                    .map(|e| e.clone()),
                head_sha: self
                    .repo
                    .cache
                    .head_shas
                    .get(&self.path)
                    .and_then(|e| e.clone()),
            });
        }

        let output = self.run_command_output(&[
            "rev-parse",
            "--is-inside-work-tree",
            "--show-toplevel",
            "--git-dir",
            "HEAD",
            "--symbolic-full-name",
            "HEAD",
        ])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();

        let is_inside = lines.next().is_some_and(|s| s.trim() == "true");
        if !is_inside {
            return Ok(WorkingTreeGitInfo::default());
        }

        // `root` and `git_dir` are safe to cache whenever their lines landed,
        // because any failure in the batch is from HEAD — which comes after.
        // `--show-toplevel` always emits a line when `is_inside=true`; if
        // canonicalize of that line fails (e.g., pathological filesystem
        // state), fall back to `self.path` which is already canonicalized by
        // `worktree_at` and guaranteed inside the work tree.
        let raw_toplevel = lines.next().unwrap_or("").trim();
        let canonical = canonicalize(PathBuf::from(raw_toplevel)).unwrap_or(self.path.clone());
        self.repo
            .cache
            .worktree_roots
            .entry(self.path.clone())
            .or_insert_with(|| canonical.clone());
        let root = Some(canonical);

        let git_dir = lines.next().and_then(|raw| {
            let path = PathBuf::from(raw.trim());
            let absolute = if path.is_relative() {
                self.path.join(&path)
            } else {
                path
            };
            let resolved = canonicalize(&absolute).ok()?;
            self.repo
                .cache
                .git_dirs
                .entry(self.path.clone())
                .or_insert_with(|| resolved.clone());
            Some(resolved)
        });

        // HEAD-derived lines (SHA, symbolic-full-name) are only trustworthy
        // when the batch succeeded. On unborn HEAD the SHA line is absent
        // (rev-parse errored on it) and the symbolic-full-name line still
        // lands but is the literal "HEAD" fallback — we can't tell that from
        // detached HEAD without the exit status.
        let (head_sha, current_branch) = if output.status.success() {
            let sha = lines.next().and_then(|raw| {
                let sha = (!raw.trim().is_empty()).then(|| raw.trim().to_owned());
                self.repo
                    .cache
                    .head_shas
                    .entry(self.path.clone())
                    .or_insert_with(|| sha.clone());
                sha
            });
            let branch = lines.next().map(|raw| {
                let branch = raw.trim().strip_prefix("refs/heads/").map(str::to_owned);
                self.repo
                    .cache
                    .current_branches
                    .entry(self.path.clone())
                    .or_insert_with(|| branch.clone());
                branch
            });
            (sha, branch)
        } else {
            (None, None)
        };

        Ok(WorkingTreeGitInfo {
            is_inside: true,
            root,
            git_dir,
            current_branch,
            head_sha,
        })
    }

    /// Get the branch checked out in this worktree, or None if in detached HEAD state.
    ///
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    /// Errors (e.g., permission denied, corrupted `.git`) are propagated, not swallowed.
    pub fn branch(&self) -> anyhow::Result<Option<String>> {
        match self.repo.cache.current_branches.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                // rev-parse --symbolic-full-name returns "refs/heads/<branch>" on a branch,
                // or "HEAD" when detached. Fails on unborn branches (no commits yet),
                // so fall back to symbolic-ref which works in all cases except detached HEAD.
                let result = match self.run_command(&["rev-parse", "--symbolic-full-name", "HEAD"])
                {
                    Ok(stdout) => stdout.trim().strip_prefix("refs/heads/").map(str::to_owned),
                    Err(_) => self
                        .run_command(&["symbolic-ref", "--short", "HEAD"])
                        .ok()
                        .map(|s| s.trim().to_owned()),
                };

                Ok(e.insert(result).clone())
            }
        }
    }

    /// Get the HEAD commit SHA for this worktree, or `None` on an unborn branch.
    ///
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    /// Populated in bulk by [`Self::prewarm_info`]; resolved lazily here on miss
    /// via `git rev-parse HEAD`, which errors on unborn branches — treated as
    /// `None` rather than an error so detached and unborn callers look the same.
    pub fn head_sha(&self) -> anyhow::Result<Option<String>> {
        match self.repo.cache.head_shas.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let sha = self
                    .run_command(&["rev-parse", "HEAD"])
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
                Ok(e.insert(sha).clone())
            }
        }
    }

    /// Return cached `git status --porcelain` output for this worktree.
    ///
    /// Keyed by worktree path in the shared `RepoCache`, so parallel tasks that
    /// each want porcelain (e.g., working-tree diff + conflict detection during
    /// `wt list`) share a single subprocess. Uses `--no-optional-locks` to avoid
    /// index-lock contention with the `git write-tree` run by
    /// `WorkingTreeConflictsTask` in parallel.
    pub fn status_porcelain_cached(&self) -> anyhow::Result<String> {
        match self.repo.cache.status_porcelain.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let stdout = self.run_command(&["--no-optional-locks", "status", "--porcelain"])?;
                Ok(e.insert(stdout).clone())
            }
        }
    }

    /// Check if the working tree has uncommitted changes.
    ///
    /// Note: This does NOT detect files hidden via `git update-index --assume-unchanged`
    /// or `--skip-worktree`. We intentionally skip that check because:
    /// 1. Detecting hidden files requires `git ls-files -v` which lists ALL tracked files
    /// 2. On large repos (70k+ files), this adds noticeable latency to every clean check
    /// 3. Users who use skip-worktree are power users who understand the implications
    /// 4. A warning wouldn't prevent data loss anyway — it's informational only
    pub fn is_dirty(&self) -> anyhow::Result<bool> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Get the root directory of this worktree (top-level of the working tree).
    ///
    /// Returns the canonicalized absolute path to the top-level directory.
    /// This could be the main worktree or a linked worktree. When the path is
    /// outside any work tree (bare repo root, non-repo directory, deleted
    /// CWD), falls back to `self.path` so callers (alias template expansion,
    /// hook context building) can degrade gracefully rather than aborting.
    ///
    /// Only confirmed toplevels are cached — the fallback path is returned
    /// but not persisted. This keeps `worktree_roots.contains_key(path)` as a
    /// reliable "is inside a work tree" signal for [`Self::prewarm_info`]'s
    /// short-circuit.
    pub fn root(&self) -> anyhow::Result<PathBuf> {
        match self.repo.cache.worktree_roots.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => match self
                .run_command(&["rev-parse", "--show-toplevel"])
                .ok()
                .map(|s| PathBuf::from(s.trim()))
                .and_then(|p| canonicalize(&p).ok())
            {
                Some(root) => Ok(e.insert(root).clone()),
                None => Ok(self.path.clone()),
            },
        }
    }

    /// Get the git directory (may be different from common-dir in worktrees).
    ///
    /// Always returns a canonicalized absolute path, resolving symlinks.
    /// This ensures consistent comparison with `git_common_dir()`.
    /// Result is cached in the repository's shared cache (keyed by worktree path).
    pub fn git_dir(&self) -> anyhow::Result<PathBuf> {
        match self.repo.cache.git_dirs.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
                let path = PathBuf::from(stdout.trim());

                // Always canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
                let absolute_path = if path.is_relative() {
                    self.path.join(&path)
                } else {
                    path
                };
                let resolved =
                    canonicalize(&absolute_path).context("Failed to resolve git directory")?;

                Ok(e.insert(resolved).clone())
            }
        }
    }

    /// Check if a rebase is in progress.
    pub fn is_rebasing(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        Ok(git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists())
    }

    /// Check if a merge is in progress.
    pub fn is_merging(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        Ok(git_dir.join("MERGE_HEAD").exists())
    }

    /// Check if this is a linked worktree (vs the main worktree).
    ///
    /// Returns `true` for linked worktrees (created via `git worktree add`),
    /// `false` for the main worktree (original clone location).
    ///
    /// Implementation: compares `git_dir` vs `common_dir`. In linked worktrees,
    /// the `.git` file points to `.git/worktrees/NAME`, so they differ. In the
    /// main worktree, both point to the same `.git` directory.
    ///
    /// For bare repos, all worktrees are "linked" (returns `true`).
    pub fn is_linked(&self) -> anyhow::Result<bool> {
        let git_dir = self.git_dir()?;
        let common_dir = self.repo.git_common_dir();
        Ok(git_dir != common_dir)
    }

    /// Ensure this worktree is clean (no uncommitted changes).
    ///
    /// Returns an error if there are uncommitted changes.
    /// - `action` describes what was blocked (e.g., "remove worktree").
    /// - `branch` identifies which branch for multi-worktree operations.
    /// - `force_hint` when true, the error hint mentions `--force` as an alternative.
    pub fn ensure_clean(
        &self,
        action: &str,
        branch: Option<&str>,
        force_hint: bool,
    ) -> anyhow::Result<()> {
        if self.is_dirty()? {
            return Err(GitError::UncommittedChanges {
                action: Some(action.into()),
                branch: branch.map(String::from),
                force_hint,
            }
            .into());
        }

        Ok(())
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    pub fn working_tree_diff_stats(&self) -> anyhow::Result<LineDiff> {
        let stdout = self.run_command(&["diff", "--shortstat", "HEAD"])?;
        Ok(LineDiff::from_shortstat(&stdout))
    }

    /// Determine whether there are staged changes in the index.
    ///
    /// Returns `Ok(true)` when staged changes are present, `Ok(false)` otherwise.
    ///
    /// Note: The index is per-worktree in git, so this checks this specific
    /// worktree's staging area.
    pub fn has_staged_changes(&self) -> anyhow::Result<bool> {
        // Exit code 0 = no diff (no staged changes), exit code 1 = diff exists (has staged changes)
        // run_command returns Ok on exit 0, Err on non-zero
        // So: Err means has changes
        Ok(self
            .run_command(&["diff", "--cached", "--quiet", "--exit-code"])
            .is_err())
    }

    /// Check whether this worktree has initialized submodules.
    ///
    /// Uses `git submodule status --recursive` and parses its stable single-character
    /// status prefix instead of relying on human-readable git error messages.
    pub fn has_initialized_submodules(&self) -> anyhow::Result<bool> {
        let output = self.run_command(&["submodule", "status", "--recursive"])?;
        Ok(has_initialized_submodules_from_status(&output))
    }

    /// Create a safety backup of current working tree state without affecting the working tree.
    ///
    /// This creates a backup commit containing all changes (staged, unstaged, and untracked files)
    /// and stores it in a custom ref (`refs/wt-backup/<branch>`). This creates a reflog entry
    /// for recovery without polluting the stash list. The working tree remains unchanged.
    ///
    /// Users can find safety backups with: `git reflog show refs/wt-backup/<branch>`
    ///
    /// Returns the short SHA of the backup commit.
    ///
    /// # Example
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current()?;
    /// let wt = repo.current_worktree();
    /// let sha = wt.create_safety_backup("feature → main (squash)")?;
    /// println!("Backup created: {}", sha);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn create_safety_backup(&self, message: &str) -> anyhow::Result<String> {
        // Create a backup commit using git stash create (without storing it in the stash list)
        let backup_sha = self
            .run_command(&["stash", "create", "--include-untracked"])?
            .trim()
            .to_string();

        // Validate that we got a SHA back
        if backup_sha.is_empty() {
            return Err(GitError::Other {
                message: "git stash create returned empty SHA - no changes to backup".into(),
            }
            .into());
        }

        // Get current branch name to use in the ref name
        let stdout = self.run_command(&["rev-parse", "--symbolic-full-name", "HEAD"])?;
        let branch = stdout
            .trim()
            .strip_prefix("refs/heads/")
            .unwrap_or("HEAD")
            .to_string();

        // Sanitize branch name for use in ref path (replace / with -)
        let safe_branch = branch.replace('/', "-");

        // Update a custom ref to point to this commit
        // --create-reflog ensures the reflog is created for this custom ref
        // This creates a reflog entry but doesn't add to the stash list
        let ref_name = format!("refs/wt-backup/{}", safe_branch);
        self.run_command(&[
            "update-ref",
            "--create-reflog",
            "-m",
            message,
            &ref_name,
            &backup_sha,
        ])
        .context("Failed to create backup ref")?;

        Ok(backup_sha[..7].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::has_initialized_submodules_from_status;
    use crate::git::Repository;
    use crate::testing::TestRepo;

    #[test]
    fn submodule_status_empty_is_not_initialized() {
        assert!(!has_initialized_submodules_from_status(""));
    }

    #[test]
    fn submodule_status_dash_is_not_initialized() {
        assert!(!has_initialized_submodules_from_status(
            "-9c8b8ff2fe89b8f1c5b8e17cb60f0d0df47f71e0 submod"
        ));
    }

    #[test]
    fn submodule_status_space_is_initialized() {
        assert!(has_initialized_submodules_from_status(
            " 9c8b8ff2fe89b8f1c5b8e17cb60f0d0df47f71e0 submod (heads/main)"
        ));
    }

    #[test]
    fn submodule_status_plus_is_initialized() {
        assert!(has_initialized_submodules_from_status(
            "+9c8b8ff2fe89b8f1c5b8e17cb60f0d0df47f71e0 submod (heads/main)"
        ));
    }

    #[test]
    fn prewarm_info_populates_every_field_on_a_branch() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());

        let info = wt.prewarm_info().unwrap();
        let head_sha = wt.head_sha().unwrap().expect("HEAD resolved after commit");

        assert!(info.is_inside);
        assert_eq!(info.root.as_deref(), Some(wt.root().unwrap().as_path()));
        assert_eq!(
            info.git_dir.as_deref(),
            Some(wt.git_dir().unwrap().as_path())
        );
        assert_eq!(info.current_branch, Some(Some("main".to_string())));
        assert_eq!(info.head_sha.as_deref(), Some(head_sha.as_str()));
        // Second call hits the shared cache (same values, no fresh subprocess semantics).
        assert_eq!(wt.head_sha().unwrap().as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn prewarm_info_second_call_returns_cached_snapshot() {
        // Once `worktree_roots` is primed, subsequent `prewarm_info` calls
        // must reconstruct from the caches rather than spawning a second
        // `git rev-parse`. We verify by mutating the cache after the first
        // call — a subprocess run would overwrite via `or_insert_with`
        // (no-op on occupied), but the short-circuit just reads the cache,
        // so our sentinel value survives.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());

        let first = wt.prewarm_info().unwrap();
        let sentinel_root = std::path::PathBuf::from("/nonexistent/sentinel");
        repo.cache
            .worktree_roots
            .insert(wt.path().to_path_buf(), sentinel_root.clone());

        let second = wt.prewarm_info().unwrap();
        assert_eq!(second.root.as_deref(), Some(sentinel_root.as_path()));
        assert_eq!(second.git_dir, first.git_dir);
        assert_eq!(second.current_branch, first.current_branch);
        assert_eq!(second.head_sha, first.head_sha);
    }

    #[test]
    fn root_fallback_outside_work_tree_does_not_pollute_cache() {
        // Invariant: `worktree_roots.contains_key(path)` ⇔ `path` is inside a
        // work tree. `root()` still returns `self.path` as a fallback for
        // graceful degradation (bare-repo aliases, deleted-CWD recovery), but
        // that fallback must never be cached — otherwise `prewarm_info`'s
        // short-circuit would misreport `is_inside: true` on the next call.
        let tmp = tempfile::tempdir().unwrap();
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(tmp.path());

        let fallback = wt.root().expect("root() returns fallback, never errors");
        assert_eq!(fallback, wt.path());
        assert!(
            !repo.cache.worktree_roots.contains_key(wt.path()),
            "fallback must not populate the cache"
        );

        let info = wt.prewarm_info().unwrap();
        assert!(!info.is_inside);
        assert!(info.root.is_none());
    }

    #[test]
    fn prewarm_info_leaves_head_fields_unresolved_on_unborn_branch() {
        // TestRepo::new() runs `git init -b main` but makes no commits, so HEAD is unborn.
        let test = TestRepo::new();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());

        let info = wt.prewarm_info().unwrap();

        assert!(info.is_inside);
        assert!(info.root.is_some(), "toplevel lands even on unborn HEAD");
        assert!(info.git_dir.is_some(), "git-dir lands even on unborn HEAD");
        assert!(
            info.current_branch.is_none(),
            "batch failed, branch cache left to `symbolic-ref` fallback"
        );
        assert!(info.head_sha.is_none(), "no commits yet => no SHA");

        // `branch()` fallback still resolves the unborn branch name through
        // `symbolic-ref --short HEAD`, independently of the batch.
        assert_eq!(wt.branch().unwrap().as_deref(), Some("main"));
        // `head_sha()` fallback returns None — `rev-parse HEAD` errors on unborn.
        assert!(wt.head_sha().unwrap().is_none());
    }
}
