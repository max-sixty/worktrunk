//! WorkingTree - a borrowed handle for worktree-specific git operations.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use dashmap::mapref::entry::Entry;
use path_slash::PathExt;

use crate::path::canonicalize_with_parents;
use crate::shell_exec::Cmd;
use dunce::canonicalize;

use super::{GitError, LineDiff, Repository};
use crate::git::CommandError;

const TEMP_INDEX_PREFIX: &str = "worktrunk-temp-index-";

#[derive(Debug)]
struct NumstatEntry {
    diff: LineDiff,
    paths: NumstatPaths,
}

#[derive(Debug)]
enum NumstatPaths {
    Single(Vec<u8>),
    Rename { old: Vec<u8>, new: Vec<u8> },
}

impl NumstatEntry {
    fn paths(&self) -> impl Iterator<Item = &[u8]> {
        let (first, second) = match &self.paths {
            NumstatPaths::Single(path) => (path.as_slice(), None),
            NumstatPaths::Rename { old, new } => (old.as_slice(), Some(new.as_slice())),
        };
        [Some(first), second].into_iter().flatten()
    }
}

fn take_numstat_field<'a>(input: &mut &'a [u8], separator: u8) -> anyhow::Result<&'a [u8]> {
    let Some(index) = input.iter().position(|&byte| byte == separator) else {
        bail!("Malformed git diff --numstat -z output")
    };
    let (field, remainder) = input.split_at(index);
    *input = &remainder[1..];
    Ok(field)
}

fn parse_numstat_count(field: &[u8]) -> anyhow::Result<usize> {
    if field == b"-" {
        return Ok(0);
    }
    std::str::from_utf8(field)
        .context("Malformed git diff --numstat count")?
        .parse()
        .context("Malformed git diff --numstat count")
}

fn parse_numstat_entries(mut output: &[u8]) -> anyhow::Result<Vec<NumstatEntry>> {
    let mut entries = Vec::new();
    while !output.is_empty() {
        let added = parse_numstat_count(take_numstat_field(&mut output, b'\t')?)?;
        let deleted = parse_numstat_count(take_numstat_field(&mut output, b'\t')?)?;
        let first_path = take_numstat_field(&mut output, 0)?;
        let paths = if first_path.is_empty() {
            NumstatPaths::Rename {
                old: take_numstat_field(&mut output, 0)?.to_vec(),
                new: take_numstat_field(&mut output, 0)?.to_vec(),
            }
        } else {
            NumstatPaths::Single(first_path.to_vec())
        };
        entries.push(NumstatEntry {
            diff: LineDiff { added, deleted },
            paths,
        });
    }
    Ok(entries)
}

/// Quote a path component for Git's `glob` pathspec magic. The caller adds
/// the one intentional wildcard after the escaped literal.
fn escape_pathspec_glob_literal(path: &str) -> String {
    path.chars().fold(String::new(), |mut escaped, ch| {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']') {
            escaped.push('\\');
        }
        escaped.push(ch);
        escaped
    })
}

/// Exclude one Worktrunk-owned temporary directory inside a worktree.
fn temporary_path_exclusion(worktree_root: &Path, temporary_path: &Path) -> Option<String> {
    let temporary_path = canonicalize_with_parents(temporary_path);
    let worktree_root = canonicalize_with_parents(worktree_root);
    temporary_path
        .strip_prefix(worktree_root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| format!(":(top,exclude,literal){}", relative.to_slash_lossy()))
}

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

/// A git operation a worktree is partway through, detected from the state files
/// git writes under its git dir (`MERGE_HEAD`, `rebase-merge/`, `BISECT_LOG`, …).
///
/// Produced by [`WorkingTree::operation_in_progress`]. Detection only: it
/// reports what git left on disk and deliberately maps nothing to a remedy.
/// `git status` already names the operation and the way out of it, in git's own
/// words and git's own translations, including states worktrunk has no variant
/// for, so a table here would only be a second copy to keep current with git.
///
/// Structured rather than a display string so [`Rebase`](Self::Rebase) can be
/// recognized without matching on user-visible text. The `strum` name is the
/// `wt list --format=json` value for the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum InProgressOperation {
    Merge,
    /// Rebase, from either backend. `git am` shares the am backend's
    /// `rebase-apply` directory and lands here too, which costs nothing: the
    /// only caller that cares is classifying a rebase worktrunk itself just
    /// started, and this gate stops that from happening during an am session.
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

/// The operation a queued sequencer belongs to, or `None` when nothing is
/// queued.
///
/// `git cherry-pick`/`git revert` over several commits keep the remaining
/// instructions in `sequencer/todo`, one `<command> <sha> <subject>` line each,
/// and delete the directory once the sequence finishes or is aborted. Reading
/// the first line mirrors git's own `sequencer_get_last_command`, which is how
/// `git status` reports a sequence whose stopped pick was committed by hand.
///
/// Only cherry-pick and revert write this file — a rebase has its own todo
/// under `rebase-merge/` — so an unrecognized first word means the file isn't a
/// sequence worktrunk should read, and it reports nothing rather than guessing.
fn sequencer_operation(git_dir: &Path) -> Option<InProgressOperation> {
    let todo = std::fs::read_to_string(git_dir.join("sequencer").join("todo")).ok()?;
    match todo.split_whitespace().next()? {
        "pick" => Some(InProgressOperation::CherryPick),
        "revert" => Some(InProgressOperation::Revert),
        _ => None,
    }
}

/// The working tree a `<common>/worktrees/<id>` registration records, read from
/// its `gitdir` file.
///
/// The file holds the path of that working tree's `.git`, absolute or relative to
/// the registration directory — git writes the relative form under
/// `worktree.useRelativePaths` and resolves either, so both are ordinary. Its
/// parent is the working tree, which is the half of git's `validate_worktree`
/// that reads from the registration side.
///
/// Resolved through [`canonicalize_with_parents`], which normalizes the `..`
/// chain a relative entry leaves behind even though the directory it names may
/// no longer exist — the case the caller is usually asking about, and the path it
/// then shows the user. Normalizing rewrites spellings only, so it cannot make
/// two directories compare equal.
fn registration_worktree_path(registration: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(registration.join("gitdir")).ok()?;
    let recorded = PathBuf::from(content.trim());
    let absolute = if recorded.is_relative() {
        registration.join(recorded)
    } else {
        recorded
    };
    absolute.parent().map(canonicalize_with_parents)
}

/// Typed snapshot returned by [`WorkingTree::prewarm_info`].
///
/// Mirrors what the batched `git rev-parse` actually resolved so callers can
/// read the data directly instead of round-tripping through the per-field
/// cache accessors. `prewarm_info` still primes the process-wide
/// `WORKTREE_ROOTS`, `GIT_DIRS`, and `CURRENT_BRANCHES` maps so later calls
/// to [`WorkingTree::branch`], [`WorkingTree::root`], and
/// [`WorkingTree::git_dir`] remain single cache hits. HEAD SHA is not cached:
/// `wt merge` and similar commands move HEAD mid-run, and a stale cached SHA
/// would surface in template variables (`{{ commit }}`) for hooks that fire
/// after the move. [`WorkingTree::head_sha`] always reads fresh.
///
/// When `is_inside` is false every other field is `None` — nothing else ran.
/// When `is_inside` is true, `root` lands unconditionally and `git_dir` lands
/// unless canonicalization failed. `current_branch` is only populated when
/// the whole batch succeeded; on unborn HEAD it stays `None` and the
/// per-accessor fallback paths handle the symbolic-ref lookup.
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
    /// Returns the canonicalized form when the input passed to `worktree_at()`
    /// (or the Repository's discovery path, for `current_worktree()`) exists
    /// on disk; otherwise returns the raw input. So on macOS, a temp path
    /// like `/tmp/foo` may surface here (and to hook template variables) as
    /// `/private/tmp/foo`.
    ///
    /// For the canonical git-determined root, use [`root()`](Self::root) instead.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a git command in this worktree and return stdout.
    pub fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = self.run_command_output(args)?;

        if !output.status.success() {
            return Err(CommandError::from_failed_output("git", args, &output).into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(stdout)
    }

    /// Run a git command in this worktree and return the raw Output.
    ///
    /// Use this when you need to check exit codes directly (e.g., for commands
    /// where non-zero exit is not an error condition).
    pub fn run_command_output(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        self.repo
            .with_object_store_env(
                Cmd::new("git")
                    .args(args.iter().copied())
                    .current_dir(&self.path)
                    .context(path_to_logging_context(&self.path)),
            )
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))
    }

    // =========================================================================
    // Worktree-specific methods
    // =========================================================================

    /// Pre-warm the worktree caches with a single batched `git rev-parse` and
    /// return a snapshot of what it resolved.
    ///
    /// Folds four rev-parse selectors that would otherwise fire as separate
    /// forks during alias/hook dispatch (`--is-inside-work-tree` from
    /// [`Repository::project_config_path`], plus [`Self::root`], [`Self::git_dir`],
    /// and [`Self::branch`]) into one. HEAD SHA is intentionally NOT batched
    /// here: it moves mid-command (`wt merge`'s rebase, `wt step commit`,
    /// hook-emitted commits), so caching invites stale reads in template
    /// expansion. [`Self::head_sha`] forks fresh on each call.
    ///
    /// `--show-toplevel` and `--git-dir` succeed even on unborn branches —
    /// rev-parse prints them before HEAD errors — so `root`/`git_dir` are
    /// cached whenever we're inside a work tree. `current_branch` is only
    /// populated when the whole batch succeeded, so [`Self::branch`]'s
    /// `symbolic-ref` fallback still handles genuine unborn HEADs. On unborn
    /// the symbolic-full-name line lands as a fallback literal "HEAD", which
    /// would be indistinguishable from detached HEAD without the exit status.
    ///
    /// Idempotent across the whole process (for paths inside a work tree):
    /// once `WORKTREE_ROOTS` is primed — by this method, by
    /// [`Repository::prewarm`], or by [`Self::root`] — subsequent calls (even
    /// from other `Repository` instances) reconstruct the snapshot from the
    /// process-wide maps without spawning a subprocess. Bare-repo roots and
    /// paths outside any work tree intentionally aren't memoized; repeat
    /// calls there re-run the batch, but such callers typically invoke
    /// `prewarm_info` only once.
    ///
    /// [`Repository::project_config_path`]: super::Repository::project_config_path
    /// [`Repository::prewarm`]: super::Repository::prewarm
    pub fn prewarm_info(&self) -> anyhow::Result<WorkingTreeGitInfo> {
        // Fast path: `WORKTREE_ROOTS` only lands on confirmed toplevels (both
        // `root()` and this method skip the cache on failure), so its presence
        // means we've already resolved this path as inside a work tree —
        // reconstruct the snapshot from the caches instead of spawning another
        // `git rev-parse`. Fields with no cache entry stay `None`, matching
        // the semantics of a freshly-run batch on unborn HEAD (where
        // `current_branch` never lands).
        if let Some(root) = super::WORKTREE_ROOTS.get(&self.path).map(|e| e.clone()) {
            return Ok(WorkingTreeGitInfo {
                is_inside: true,
                root: Some(root),
                git_dir: super::GIT_DIRS.get(&self.path).map(|e| e.clone()),
                current_branch: super::CURRENT_BRANCHES.get(&self.path).map(|e| e.clone()),
            });
        }

        let output = self.run_command_output(&[
            "rev-parse",
            "--is-inside-work-tree",
            "--show-toplevel",
            "--git-dir",
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
        super::WORKTREE_ROOTS
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
            super::GIT_DIRS
                .entry(self.path.clone())
                .or_insert_with(|| resolved.clone());
            Some(resolved)
        });

        // The `--symbolic-full-name HEAD` line is only trustworthy when the
        // batch succeeded. On unborn HEAD the line lands but is the literal
        // "HEAD" fallback — we can't tell that from detached HEAD without the
        // exit status.
        let current_branch = if output.status.success() {
            lines.next().map(|raw| {
                let branch = raw.trim().strip_prefix("refs/heads/").map(str::to_owned);
                super::CURRENT_BRANCHES
                    .entry(self.path.clone())
                    .or_insert_with(|| branch.clone());
                branch
            })
        } else {
            None
        };

        Ok(WorkingTreeGitInfo {
            is_inside: true,
            root,
            git_dir,
            current_branch,
        })
    }

    /// Get the branch checked out in this worktree, or None if in detached HEAD state.
    ///
    /// Result is cached process-wide in `CURRENT_BRANCHES` (keyed by worktree
    /// path). Errors (e.g., permission denied, corrupted `.git`) are
    /// propagated, not swallowed.
    pub fn branch(&self) -> anyhow::Result<Option<String>> {
        match super::CURRENT_BRANCHES.entry(self.path.clone()) {
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
    /// Always reads fresh via `git rev-parse HEAD` — HEAD moves mid-command in
    /// any flow that commits, rebases, or merges, and a cached SHA would
    /// surface stale in template variables (`{{ commit }}`) for hooks that
    /// fire after the move. The fork is cheap (~5 ms) and only fires on the
    /// few paths that need a HEAD SHA. Errors from `rev-parse` (unborn
    /// branches) are mapped to `None` so detached and unborn callers look the
    /// same.
    pub fn head_sha(&self) -> anyhow::Result<Option<String>> {
        Ok(self
            .run_command(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()))
    }

    /// Return cached `git status --porcelain` output for this worktree.
    ///
    /// Keyed by worktree path in the shared `RepoCache`, so parallel tasks that
    /// each want porcelain (e.g., working-tree diff + conflict detection during
    /// `wt list`) share a single subprocess. Uses `--no-optional-locks` to avoid
    /// index-lock contention with the `git write-tree` run by
    /// `WorkingTreeConflictsTask` in parallel. Explicitly requests normal
    /// untracked-file reporting so `status.showUntrackedFiles=no` cannot make
    /// an untracked-only worktree look clean to callers.
    pub fn status_porcelain_cached(&self) -> anyhow::Result<String> {
        match self.repo.cache.status_porcelain.entry(self.path.clone()) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                let exclusions = self.observation_path_exclusions()?;
                let mut args = vec![
                    "--no-optional-locks",
                    "status",
                    "--porcelain",
                    "--untracked-files=normal",
                ];
                args.extend(exclusions.iter().map(String::as_str));
                let stdout = self.run_command(&args)?;
                Ok(e.insert(stdout).clone())
            }
        }
    }

    fn observation_path_exclusions(&self) -> anyhow::Result<Vec<String>> {
        let worktree_root = self.root()?;
        Ok(self
            .repo
            .cache
            .observation_object_directories
            .iter()
            .filter_map(|entry| temporary_path_exclusion(&worktree_root, entry.key()))
            .collect())
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

    /// Return the raw `git status --porcelain` lines for a dirty working tree
    /// (one entry per line, trailing newline stripped, in git's order). Returns
    /// an empty vec when the tree is clean. Use this where the error message
    /// should tell the user *what* is dirty — e.g., when constructing
    /// [`GitError::UncommittedChanges`] in [`Self::ensure_clean`]. The same
    /// caveats as [`Self::is_dirty`] apply (skip-worktree files are invisible).
    pub fn dirty_files(&self) -> anyhow::Result<Vec<String>> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(stdout.lines().map(str::to_owned).collect())
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
    /// but not persisted. This keeps `WORKTREE_ROOTS.contains_key(path)` as a
    /// reliable "is inside a work tree" signal for [`Self::prewarm_info`]'s
    /// short-circuit.
    pub fn root(&self) -> anyhow::Result<PathBuf> {
        match super::WORKTREE_ROOTS.entry(self.path.clone()) {
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
    /// Result is cached process-wide in `GIT_DIRS` (keyed by worktree path).
    pub fn git_dir(&self) -> anyhow::Result<PathBuf> {
        match super::GIT_DIRS.entry(self.path.clone()) {
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

    /// The git operation this worktree is partway through, if any.
    ///
    /// Reads the state files git writes under the worktree's git dir, in the
    /// order [`git status`](https://git-scm.com/docs/git-status) consults them,
    /// so the answer tracks what git itself calls "in progress".
    pub fn operation_in_progress(&self) -> anyhow::Result<Option<InProgressOperation>> {
        let git_dir = self.git_dir()?;

        if git_dir.join("MERGE_HEAD").exists() {
            return Ok(Some(InProgressOperation::Merge));
        }

        // `rebase-merge` (interactive/merge backend) and `rebase-apply` (am
        // backend, also used by `git am`) are mutually exclusive; either one
        // means commits are mid-replay.
        if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
            return Ok(Some(InProgressOperation::Rebase));
        }

        if git_dir.join("CHERRY_PICK_HEAD").exists() {
            return Ok(Some(InProgressOperation::CherryPick));
        }

        if git_dir.join("REVERT_HEAD").exists() {
            return Ok(Some(InProgressOperation::Revert));
        }

        // The two `_HEAD` files above exist only while a single pick is
        // stopped: resolving one with `git commit` instead of `--continue`
        // removes it and leaves the rest of the sequence queued, which git
        // still reports as in progress.
        if let Some(operation) = sequencer_operation(&git_dir) {
            return Ok(Some(operation));
        }

        if git_dir.join("BISECT_LOG").exists() {
            return Ok(Some(InProgressOperation::Bisect));
        }

        Ok(None)
    }

    /// Paths the index still records as unmerged.
    ///
    /// A conflict git could not resolve leaves the path at stages 1–3 of the
    /// index, which is what makes `git commit` refuse. Reading the index
    /// rather than the state files answers a different question from
    /// [`operation_in_progress`](Self::operation_in_progress): a conflicted
    /// `git stash pop` leaves unmerged paths behind with no operation open at
    /// all.
    pub fn unmerged_paths(&self) -> anyhow::Result<Vec<String>> {
        let output = self
            .run_command(&["diff", "--name-only", "--diff-filter=U", "-z"])
            .context("Failed to list unmerged paths")?;
        Ok(output
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Fail when the index still holds unresolved conflicts.
    ///
    /// A precondition for the commands that stage on the user's behalf
    /// (`wt step commit`, `wt step squash`, `wt merge`,
    /// `wt step relocate --commit`). `git add -A` collapses
    /// an unmerged path's three stages into one entry, so it resolves the
    /// conflict as far as the index is concerned while the file on disk still
    /// holds `<<<<<<<` markers — and it takes git's own refusal to commit
    /// with it. Asking before staging is what keeps the markers out of a
    /// commit.
    ///
    /// `action` names the command the user typed. An entry point knows only
    /// that, and often not yet whether it will commit at all —
    /// `wt merge --no-commit` resolves its flags after this gate — so it
    /// refuses in the name of the operation it was asked for.
    ///
    /// Every one of those gates refuses *early* — ahead of hooks, approval
    /// prompts, and the LLM call, none of which is worth running for a commit
    /// that cannot happen. The staging itself is guarded by
    /// [`stage`](Self::stage), which runs the same check with nothing between
    /// it and the `git add`.
    pub fn ensure_no_unmerged_paths(&self, action: &str) -> anyhow::Result<()> {
        let files = self.unmerged_paths()?;
        if files.is_empty() {
            return Ok(());
        }
        Err(crate::git::GitError::UnmergedPaths {
            action: action.to_string(),
            files,
        }
        .into())
    }

    /// Stage the working tree on the user's behalf, refusing an unmerged index.
    ///
    /// **The only path to `git add` against a worktree's real index** — the
    /// other `git add` calls all write a throwaway index via
    /// [`temp_index`](Self::temp_index), which commits nothing. Staging
    /// for the user is what turns an unresolved conflict into a commit full of
    /// `<<<<<<<` markers (see
    /// [`ensure_no_unmerged_paths`](Self::ensure_no_unmerged_paths)), so the
    /// check and the `git add` live in one call rather than as a convention
    /// every caller re-applies. A command that stages through here cannot
    /// forget the gate, and cannot order it after the staging that destroys
    /// the evidence — `git add` collapses the index stages, so a check run
    /// afterwards passes vacuously.
    ///
    /// This is *in addition to* the callers' early refusals, not a replacement
    /// for them: the two guard different windows. `wt step commit` gates
    /// before its `pre-commit` hooks so a doomed commit runs no project
    /// commands — and those hooks are arbitrary project code, free to leave
    /// unmerged paths behind after that gate has passed.
    ///
    /// The refusal names the commit rather than taking an `action` from the
    /// caller, because the commit is the only thing this gate guards — staging
    /// on the user's behalf happens for no other reason. So
    /// `wt merge --no-squash` reports `Cannot commit` even though the user
    /// typed `merge`: that is the step actually blocked, and by then the flags
    /// have resolved and the commit is certain.
    ///
    /// [`StageMode::None`] stages nothing but is still gated: the caller is
    /// about to commit whatever the index already holds.
    ///
    /// [`StageMode::None`]: crate::config::StageMode::None
    pub fn stage(&self, mode: crate::config::StageMode) -> anyhow::Result<()> {
        self.ensure_no_unmerged_paths("commit")?;
        if let Some(args) = mode.add_args() {
            self.run_command(args).context("Failed to stage changes")?;
        }
        Ok(())
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

    /// Refuse when the directory at this worktree's path does not hold this
    /// worktree.
    ///
    /// Git makes this check itself before `git worktree remove` and refuses
    /// with `validation failed … does not point back to
    /// '.git/worktrees/<id>'`, `--force` included. Worktrunk's removal fast
    /// path renames the directory into trash rather than asking git to (see
    /// [`stage_worktree_removal`](crate::git::remove::stage_worktree_removal)),
    /// so git's validation never runs and the guarantee has to be made here.
    /// Removing the wrong directory is unrecoverable, whether it holds a full
    /// clone that came to sit at a stale registration's path (uncommitted work
    /// and, for a repo never pushed, the only copy of its objects) or a sibling
    /// worktree of this repository, moved onto the path after this one was
    /// deleted.
    ///
    /// So the test is git's own: the directory's `.git` must name *this
    /// registration*, and that registration's `gitdir` file must name this
    /// directory back (`registration_worktree_path`). Repository-level
    /// ownership is the weaker half: a sibling worktree's git dir sits under
    /// `<common>/worktrees/` too, so asking only which repository the occupant
    /// answers to accepts one moved onto this path. The main worktree is the
    /// same test where there is no registration to point back at: its git dir
    /// *is* the common dir, and that equality is the whole of it.
    ///
    /// Resolution reads the `.git` entry in this directory on every call
    /// (`Repository::git_dir_at`) instead of going through the `GIT_DIRS`-cached
    /// [`git_dir`](Self::git_dir), which would answer from whenever an earlier
    /// caller asked. That is what makes the second call worth making: removal
    /// gates at planning and again at the rename, with the approval prompt and
    /// the `pre-remove` hook running in between.
    ///
    /// Deliberately not a `prunable` check — git leaves the registration alone
    /// precisely because the occupant's own `.git` resolves, so
    /// [`Repository::usable_worktree_for_branch`] sees nothing wrong here. The
    /// two cover different halves of "the directory no longer holds this
    /// worktree": prunable is the half git notices, this is the half it does
    /// not.
    pub fn ensure_holds_this_worktree(&self) -> anyhow::Result<()> {
        let common_dir = self.repo.git_common_dir();
        // A git dir that can't be resolved at all is the strongest form of "not
        // this worktree": nothing there answers for it. Treating that as a
        // refusal keeps the failure closed.
        let git_dir = Repository::git_dir_at(&self.path);
        if git_dir.as_deref() == Some(common_dir) {
            return Ok(());
        }

        // Where the occupant's own registration says it lives. `None` when there
        // is no registration of ours to ask — the occupant answers to a
        // different repository, or its registration here has lost its `gitdir`
        // file.
        let registrations = common_dir.join("worktrees");
        let occupant_registered_at = git_dir
            .filter(|git_dir| git_dir.parent() == Some(registrations.as_path()))
            .as_deref()
            .and_then(registration_worktree_path);
        if occupant_registered_at
            .as_deref()
            .is_some_and(|recorded| crate::path::paths_match(recorded, &self.path))
        {
            return Ok(());
        }

        Err(GitError::WorktreePathNotOurs {
            path: self.path.clone(),
            occupant_registered_at,
        }
        .into())
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
        let dirty_files = self.dirty_files()?;
        if !dirty_files.is_empty() {
            return Err(GitError::UncommittedChanges {
                action: Some(action.into()),
                branch: branch.map(String::from),
                force_hint,
                dirty_files,
            }
            .into());
        }

        Ok(())
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    pub fn working_tree_diff_stats(&self) -> anyhow::Result<LineDiff> {
        let stdout = self.run_command(&["diff", "--shortstat", "--find-renames", "HEAD"])?;
        Ok(LineDiff::from_shortstat(&stdout))
    }

    /// Working-tree diff stats vs HEAD that also count untracked files.
    /// The scope matches `wt step diff`; explicit rename detection keeps
    /// `HEAD±` stable across user Git configuration.
    ///
    /// Untracked paths enter a temporary index as intent-to-add entries, which
    /// lets one diff pair them with tracked deletions as renames without writing
    /// their contents as blobs. The real-index diff fills any tracked entry the
    /// copied index misses because of racy stat data.
    pub fn working_tree_diff_stats_with_untracked(&self) -> anyhow::Result<LineDiff> {
        let exclusions = self.observation_path_exclusions()?;
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
        args.extend(exclusions.iter().map(String::as_str));
        let untracked_output = self.run_command_output(&args)?;
        if !untracked_output.status.success() {
            return Err(CommandError::from_failed_output("git", &args, &untracked_output).into());
        }
        if untracked_output.stdout.is_empty() {
            return self.working_tree_diff_stats();
        }

        let numstat_args = [
            "diff",
            "--numstat",
            "-z",
            "--find-renames",
            "--end-of-options",
            "HEAD",
        ];
        let tracked_output = self.run_command_output(&numstat_args)?;
        if !tracked_output.status.success() {
            return Err(
                CommandError::from_failed_output("git", &numstat_args, &tracked_output).into(),
            );
        }
        let tracked_entries = parse_numstat_entries(&tracked_output.stdout)?;

        let idx = self.temp_index()?;
        idx.register_untracked_paths(untracked_output.stdout)?;
        let combined_output = idx.run_command_output(numstat_args)?;
        let combined_entries = parse_numstat_entries(&combined_output.stdout)?;

        let mut stats = LineDiff::default();
        for entry in &combined_entries {
            stats.added += entry.diff.added;
            stats.deleted += entry.diff.deleted;
        }

        // A copied index can trust stale stat data that the real index treats
        // as racily clean. The combined diff owns rename pairing, while the
        // real-index diff supplies tracked records absent from it.
        let covered_paths: HashSet<&[u8]> = combined_entries
            .iter()
            .flat_map(NumstatEntry::paths)
            .collect();
        for entry in tracked_entries {
            if entry.paths().all(|path| !covered_paths.contains(path)) {
                stats.added += entry.diff.added;
                stats.deleted += entry.diff.deleted;
            }
        }
        Ok(stats)
    }

    /// Open a working copy of this worktree's index for staging
    /// operations that must not mutate the real index. See
    /// [`TempIndex`] for the rationale and call sites.
    pub fn temp_index(&self) -> anyhow::Result<TempIndex> {
        let git_dir = self.git_dir()?;
        let worktree_root = self.root()?;
        let real_index = git_dir.join("index");
        let log_ctx = path_to_logging_context(self.path());

        // Keep the exclusively-created file open while copying a real index;
        // removing it first would expose its random name to a symlink race in
        // a shared system temp directory. A missing `<gitdir>/index` is
        // semantically an empty index (nothing staged), so only that case
        // closes and removes the 0-byte file before Git creates a valid index.
        let mut builder = tempfile::Builder::new();
        builder.prefix(TEMP_INDEX_PREFIX);
        let mut temp_file = match builder.tempfile() {
            Ok(file) => file,
            Err(temp_error) => builder.tempfile_in(&git_dir).context(format!(
                "Failed to create temporary index in the system temp directory or Git directory; system temp error: {temp_error}"
            ))?,
        };
        let mut real_index_file = match std::fs::File::open(&real_index) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("Failed to open index file"),
        };
        if let Some(real_index_file) = &mut real_index_file {
            std::io::copy(real_index_file, temp_file.as_file_mut())
                .context("Failed to copy index file")?;
        }
        let temp = temp_file.into_temp_path();
        if real_index_file.is_none() {
            // `into_temp_path` closes the handle first, which matters on
            // Windows: deleting a still-open file leaves the name pending.
            std::fs::remove_file(&temp).context("Failed to clear temporary index")?;
        }

        Ok(TempIndex {
            temp,
            worktree_root,
            log_ctx,
            object_store_environment: self.repo.object_store_environment().map(
                |(directory, alternates)| (directory.to_path_buf(), alternates.to_os_string()),
            ),
            observation_path_exclusions: self.observation_path_exclusions()?,
        })
    }

    /// Write a tree containing the index plus tracked working-tree changes.
    ///
    /// A temporary index keeps the user's staging state unchanged. The conflict
    /// probe is advisory and leaves untracked paths out of its synthetic tree.
    pub fn write_tracked_worktree_tree(&self) -> anyhow::Result<String> {
        let index = self.temp_index()?;
        // With no pathspec, `add -u` also accepts an empty index (a staged
        // deletion of every tracked file, or a missing real index). The command
        // still covers the whole worktree because TempIndex runs at its root.
        index.run_command(["add".to_string(), "-u".to_string(), "--sparse".to_string()])?;
        index.write_tree()
    }

    /// Write a tree containing the current index state without changing the
    /// real index. Copying the index keeps a missing real index absent while
    /// avoiding a working-tree scan for staged-only changes.
    pub fn write_index_tree(&self) -> anyhow::Result<String> {
        self.temp_index()?.write_tree()
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

        // Slashes are valid in ref names, so use the branch as-is —
        // flattening `/` to `-` would collide e.g. `a/b` with `a-b`.
        // --create-reflog creates the reflog without adding to the stash list.
        let ref_name = format!("refs/wt-backup/{}", branch);
        self.run_command(&[
            "update-ref",
            "--create-reflog",
            "-m",
            message,
            &ref_name,
            &backup_sha,
        ])
        .context("Failed to create backup ref")?;

        self.repo().short_sha(&backup_sha)
    }
}

/// A temporary copy of a worktree's index, plus the bits needed to run
/// `git` commands against it.
///
/// Built by [`WorkingTree::temp_index`]. Drop deletes the temp file.
///
/// Some operations need to "stage something" — register untracked files,
/// or `git add -A` everything — to feed a follow-up `git diff` /
/// `git write-tree` / `git diff --shortstat`. Doing that against the real
/// index would mutate the user's staging state. The trick is to copy the
/// real index, point `GIT_INDEX_FILE` at the copy, and run those
/// operations there. Today the callers are
/// [`WorkingTree::working_tree_diff_stats_with_untracked`] (HEAD± with
/// untracked, used by list and statusline),
/// `WorkingTreeConflictsTask` (write-tree of tracked changes, for
/// merge-conflict probing), `wt step diff` (diff vs target merge-base with
/// untracked), `wt step commit --dry-run` (mirror its `--stage` mode without
/// changing the user's index), and the `wt switch` unified/working preview tabs.
pub struct TempIndex {
    temp: tempfile::TempPath,
    worktree_root: PathBuf,
    log_ctx: String,
    /// Copied from the [`Repository`] so a redirected `wt list` writes the temp
    /// index's `write-tree` objects into the temporary store. `None` on the
    /// normal persistent path. See [`Repository::redirect_objects_for_observation`].
    object_store_environment: Option<(PathBuf, OsString)>,
    /// Exact Worktrunk-owned temporary directories to hide from untracked
    /// previews when the configured temp directory is inside this worktree.
    observation_path_exclusions: Vec<String>,
}

impl TempIndex {
    /// Path to the temporary index file.
    pub fn path(&self) -> &Path {
        &self.temp
    }

    /// Stage the paths selected by `mode` into this temporary index.
    ///
    /// Uses the same `git add` mode as [`WorkingTree::stage`], minus its
    /// unmerged-paths gate. All-files mode also scopes the add to the worktree
    /// root (`-- .`) and, when the system temp directory sits inside the
    /// worktree, excludes Worktrunk's own temporary indexes.
    ///
    /// [`StageMode`]: crate::config::StageMode
    pub fn stage(&self, mode: crate::config::StageMode) -> anyhow::Result<()> {
        let Some(add_args) = mode.add_args() else {
            return Ok(());
        };
        let mut args: Vec<String> = add_args.iter().map(|arg| (*arg).to_string()).collect();
        if mode == crate::config::StageMode::All {
            args.extend(["--".to_string(), ".".to_string()]);
            self.append_temp_index_exclusion(&mut args)?;
        }
        self.run_command(args)?;
        Ok(())
    }

    /// Write the temporary index as a tree and return its object id.
    fn write_tree(&self) -> anyhow::Result<String> {
        self.run_command(["write-tree"])
            .map(|output| output.trim().to_string())
    }

    /// Register untracked files in the temporary index without adding their
    /// contents. A following `git diff <base>` can then include those files as
    /// new while preserving the user's real index and staging state. If the
    /// system temp directory is inside the worktree, the pathspec excludes all
    /// files in that directory with Worktrunk's dedicated prefix so concurrent
    /// temporary indexes cannot enter each other's diffs. Keeping the random
    /// files directly in the OS temp directory avoids a predictable shared
    /// parent on multi-user systems.
    pub(super) fn register_untracked(&self) -> anyhow::Result<()> {
        let mut args = vec![
            "add".to_string(),
            "--intent-to-add".to_string(),
            // An untracked file may sit outside a sparse-checkout definition.
            // Without --sparse git refuses the entire add, hiding both that
            // file and otherwise-valid worktree changes from the preview.
            "--sparse".to_string(),
            "--".to_string(),
            ".".to_string(),
        ];
        self.append_temp_index_exclusion(&mut args)?;
        self.run_command(args)?;
        Ok(())
    }

    /// Register an exact NUL-separated set of untracked paths as intent-to-add.
    fn register_untracked_paths(&self, paths: Vec<u8>) -> anyhow::Result<()> {
        self.run_command_output_with_input(
            [
                "add",
                "--intent-to-add",
                "--sparse",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            paths,
        )?;
        Ok(())
    }

    fn append_temp_index_exclusion(&self, args: &mut Vec<String>) -> anyhow::Result<()> {
        let temp_dir = canonicalize_with_parents(
            self.temp
                .parent()
                .context("Temporary index has no parent directory")?,
        );
        let worktree_root = canonicalize_with_parents(&self.worktree_root);
        if let Ok(relative) = temp_dir.strip_prefix(&worktree_root) {
            let relative = escape_pathspec_glob_literal(&relative.to_slash_lossy());
            let separator = if relative.is_empty() { "" } else { "/" };
            args.push(format!(
                ":(top,exclude,glob){relative}{separator}{TEMP_INDEX_PREFIX}*"
            ));
        }
        args.extend(self.observation_path_exclusions.iter().cloned());
        Ok(())
    }

    fn run_command(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> anyhow::Result<String> {
        let output = self.run_command_output(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_command_output(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> anyhow::Result<std::process::Output> {
        self.run_command_output_with_input(args, Vec::new())
    }

    fn run_command_output_with_input(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        stdin: Vec<u8>,
    ) -> anyhow::Result<std::process::Output> {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        let mut command = self.command(args.iter().cloned());
        if !stdin.is_empty() {
            command = command.stdin_bytes(stdin);
        }
        let output = command
            .run()
            .with_context(|| format!("Failed to execute: git {}", args.join(" ")))?;
        if !output.status.success() {
            return Err(CommandError::from_failed_output("git", &args, &output).into());
        }
        Ok(output)
    }

    /// Build a `git` command pointed at this temp index.
    ///
    /// Wires `current_dir` to the worktree root, the worktree's logging
    /// context, and `GIT_INDEX_FILE`. The caller adds the subcommand and
    /// chooses `.run()` / `.stream()`.
    pub(super) fn command<I, S>(&self, args: I) -> Cmd
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let command = Cmd::new("git")
            .args(args)
            .current_dir(&self.worktree_root)
            .context(self.log_ctx.clone())
            .env("GIT_INDEX_FILE", self.path());
        match &self.object_store_environment {
            Some((directory, alternates)) => command
                .env("GIT_OBJECT_DIRECTORY", directory)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", alternates),
            None => command,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::has_initialized_submodules_from_status;
    use crate::git::{LineDiff, Repository};
    use crate::shell_exec::Cmd;
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

        assert!(info.is_inside);
        assert_eq!(info.root.as_deref(), Some(wt.root().unwrap().as_path()));
        assert_eq!(
            info.git_dir.as_deref(),
            Some(wt.git_dir().unwrap().as_path())
        );
        assert_eq!(info.current_branch, Some(Some("main".to_string())));
    }

    #[test]
    fn head_sha_tracks_head_movement() {
        // `head_sha()` always reads fresh — a commit between calls must be
        // visible without invalidation. Guards against re-introducing a cache.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());

        let before = wt.head_sha().unwrap().expect("HEAD resolved after init");

        std::fs::write(test.root_path().join("file.txt"), "second").unwrap();
        test.run_git(&["add", "."]);
        test.run_git(&["commit", "-m", "second"]);

        let after = wt
            .head_sha()
            .unwrap()
            .expect("HEAD still resolved after second commit");
        assert_ne!(before, after, "head_sha must reflect the new commit");
    }

    #[test]
    fn cached_porcelain_reports_untracked_files_hidden_by_user_config() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(test.root_path().join("hidden-by-config.txt"), "loose\n").unwrap();

        let repo = Repository::at(test.root_path()).unwrap();
        let status = repo.current_worktree().status_porcelain_cached().unwrap();

        assert!(
            status.contains("?? hidden-by-config.txt"),
            "the shared status snapshot must override status.showUntrackedFiles=no: {status:?}"
        );
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
        super::super::WORKTREE_ROOTS.insert(wt.path().to_path_buf(), sentinel_root.clone());

        let second = wt.prewarm_info().unwrap();
        assert_eq!(second.root.as_deref(), Some(sentinel_root.as_path()));
        assert_eq!(second.git_dir, first.git_dir);
        assert_eq!(second.current_branch, first.current_branch);
    }

    #[test]
    fn root_fallback_outside_work_tree_does_not_pollute_cache() {
        // Invariant: `WORKTREE_ROOTS.contains_key(path)` ⇔ `path` is inside a
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
            !super::super::WORKTREE_ROOTS.contains_key(wt.path()),
            "fallback must not populate the cache"
        );

        let info = wt.prewarm_info().unwrap();
        assert!(!info.is_inside);
        assert!(info.root.is_none());
    }

    #[test]
    fn create_safety_backup_distinguishes_slash_and_dash_branches() {
        // Branches `a/b` and `a-b` must back up to distinct refs;
        // flattening `/` to `-` would let one backup clobber the other.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());

        for branch in ["a/b", "a-b"] {
            test.run_git(&["switch", "-c", branch]);
            // Modify the tracked file so `git stash create` picks up changes.
            std::fs::write(test.root_path().join("file.txt"), branch).unwrap();
            wt.create_safety_backup(&format!("{branch} (squash)"))
                .unwrap();
            test.run_git(&["checkout", "--", "file.txt"]);
            test.run_git(&["switch", "main"]);
        }

        let refs = test.git_output(&["for-each-ref", "--format=%(refname)", "refs/wt-backup/"]);
        let mut listed: Vec<&str> = refs.lines().collect();
        listed.sort();
        assert_eq!(
            listed,
            vec!["refs/wt-backup/a-b", "refs/wt-backup/a/b"],
            "expected distinct backup refs for `a/b` and `a-b`, got: {refs}"
        );
    }

    #[test]
    fn prewarm_at_populates_global_caches_for_a_fresh_repository() {
        // `Repository::prewarm_at` is the eager merge: one rev-parse fills
        // `GIT_COMMON_DIR_CACHE` (so the next `Repository::at` is free) and
        // the process-wide worktree maps (`WORKTREE_ROOTS`, `GIT_DIRS`,
        // `CURRENT_BRANCHES`) so a `prewarm_info` call from a fresh
        // Repository hits memory.
        let test = TestRepo::with_initial_commit();
        Repository::prewarm_at(test.root_path());

        // Sentinel-based check: a freshly-built `Repository` has nothing of
        // its own to short-circuit `prewarm_info`. Mutating the process-wide
        // entry — which `prewarm_at` just populated — and observing that
        // `prewarm_info` returns the sentinel proves the fast path consults
        // the global map (no refork).
        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());
        let sentinel = std::path::PathBuf::from("/nonexistent/prewarm-at-sentinel");
        super::super::WORKTREE_ROOTS.insert(wt.path().to_path_buf(), sentinel.clone());

        let info = wt.prewarm_info().unwrap();
        assert_eq!(info.root.as_deref(), Some(sentinel.as_path()));
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

        // `branch()` fallback still resolves the unborn branch name through
        // `symbolic-ref --short HEAD`, independently of the batch.
        assert_eq!(wt.branch().unwrap().as_deref(), Some("main"));
        // `head_sha()` returns None on unborn HEAD — `rev-parse HEAD` errors,
        // mapped to None so detached and unborn callers look the same.
        assert!(wt.head_sha().unwrap().is_none());
    }

    #[test]
    fn working_tree_diff_stats_with_untracked_counts_untracked_and_preserves_real_index() {
        // Two-line untracked file plus a one-line tracked modification:
        // the with-untracked variant must sum both, while the real index
        // must remain byte-identical (the temp-index trick is the
        // mechanism — this test guards that contract).
        let test = TestRepo::with_initial_commit();
        std::fs::write(test.root_path().join("tracked.txt"), "old\n").unwrap();
        test.run_git(&["add", "tracked.txt"]);
        test.run_git(&["commit", "-m", "add tracked"]);

        std::fs::write(test.root_path().join("tracked.txt"), "new\n").unwrap();
        std::fs::write(test.root_path().join("untracked.txt"), "a\nb\n").unwrap();

        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());
        let real_index = wt.git_dir().unwrap().join("index");
        let index_before = std::fs::read(&real_index).unwrap();

        // Tracked-only path skips the untracked file entirely.
        let tracked_only = wt.working_tree_diff_stats().unwrap();
        assert_eq!(tracked_only.added, 1);
        assert_eq!(tracked_only.deleted, 1);

        let with_untracked = wt.working_tree_diff_stats_with_untracked().unwrap();
        assert_eq!(with_untracked.added, 3, "1 modified line + 2 untracked");
        assert_eq!(with_untracked.deleted, 1);

        let index_after = std::fs::read(&real_index).unwrap();
        assert_eq!(
            index_before, index_after,
            "real index must not be mutated by the temp-index path"
        );
    }

    #[test]
    fn working_tree_diff_stats_without_untracked_files_still_finds_renames() {
        let test = TestRepo::with_initial_commit();
        let source = (1..=100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(test.root_path().join("source.txt"), format!("{source}\n")).unwrap();
        test.run_git(&["add", "source.txt"]);
        test.run_git(&["commit", "-m", "add source"]);
        test.run_git(&["config", "diff.renames", "false"]);

        std::fs::create_dir(test.root_path().join("moved")).unwrap();
        test.run_git(&["mv", "source.txt", "moved/source.txt"]);

        let repo = Repository::at(test.root_path()).unwrap();
        assert_eq!(
            repo.current_worktree()
                .working_tree_diff_stats_with_untracked()
                .unwrap(),
            LineDiff::default(),
            "HEAD± uses one rename policy whether or not untracked files exist"
        );
    }

    #[test]
    fn working_tree_diff_stats_with_untracked_reports_only_a_moves_line_delta() {
        let test = TestRepo::with_initial_commit();
        let source = (1..=100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(test.root_path().join("source.txt"), format!("{source}\n")).unwrap();
        test.run_git(&["add", "source.txt"]);
        test.run_git(&["commit", "-m", "add source"]);

        std::fs::create_dir(test.root_path().join("moved")).unwrap();
        std::fs::rename(
            test.root_path().join("source.txt"),
            test.root_path().join("moved/source.txt"),
        )
        .unwrap();

        let repo = Repository::at(test.root_path())
            .unwrap()
            .redirect_objects_for_observation()
            .unwrap();
        let wt = repo.current_worktree();
        let real_index = wt.git_dir().unwrap().join("index");
        let index_before = std::fs::read(&real_index).unwrap();
        let objects_before = test.git_output(&["count-objects", "-v"]);

        assert_eq!(
            wt.working_tree_diff_stats_with_untracked().unwrap(),
            LineDiff::default(),
            "an exact move changes paths but no lines"
        );

        let mut moved = source.lines().map(str::to_string).collect::<Vec<_>>();
        moved[49] = "changed line".to_string();
        moved.push("added line".to_string());
        std::fs::write(
            test.root_path().join("moved/source.txt"),
            format!("{}\n", moved.join("\n")),
        )
        .unwrap();
        assert_eq!(
            wt.working_tree_diff_stats_with_untracked().unwrap(),
            LineDiff {
                added: 2,
                deleted: 1,
            },
            "an edited move reports only the edits"
        );

        assert_eq!(std::fs::read(&real_index).unwrap(), index_before);
        assert_eq!(test.git_output(&["count-objects", "-v"]), objects_before);
    }

    #[cfg(unix)]
    #[test]
    fn working_tree_diff_stats_with_untracked_preserves_rename_path_bytes() {
        let test = TestRepo::with_initial_commit();
        let source = "source\tname\n.txt";
        let destination = "moved\tname\n.txt";
        std::fs::write(test.root_path().join(source), "one\ntwo\nthree\n").unwrap();
        test.run_git(&["add", source]);
        test.run_git(&["commit", "-m", "add unusual path"]);
        std::fs::rename(
            test.root_path().join(source),
            test.root_path().join(destination),
        )
        .unwrap();

        let repo = Repository::at(test.root_path())
            .unwrap()
            .redirect_objects_for_observation()
            .unwrap();
        assert_eq!(
            repo.current_worktree()
                .working_tree_diff_stats_with_untracked()
                .unwrap(),
            LineDiff::default()
        );
    }

    #[test]
    fn observation_object_directory_is_excluded_from_status_and_diff() {
        let test = TestRepo::with_initial_commit();
        let observation_directory = test
            .root_path()
            .join("local-tmp/worktrunk-list-objects-test");
        std::fs::create_dir_all(observation_directory.join("info")).unwrap();
        std::fs::write(observation_directory.join("info/alternates"), "temporary\n").unwrap();
        std::fs::write(test.root_path().join("untracked.txt"), "user\n").unwrap();

        let repo = Repository::at(test.root_path()).unwrap();
        repo.cache
            .observation_object_directories
            .insert(observation_directory.clone(), ());
        let wt = repo.current_worktree();

        let status = wt.status_porcelain_cached().unwrap();
        assert!(status.contains("?? untracked.txt"));
        assert!(!status.contains("worktrunk-list-objects-test"));

        let stats = wt.working_tree_diff_stats_with_untracked().unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(stats.deleted, 0);
    }

    fn sparse_checkout_with_untracked_file() -> TestRepo {
        let test = TestRepo::with_initial_commit();
        std::fs::create_dir_all(test.root_path().join("visible")).unwrap();
        std::fs::create_dir_all(test.root_path().join("hidden")).unwrap();
        std::fs::write(test.root_path().join("visible/tracked.txt"), "base\n").unwrap();
        std::fs::write(test.root_path().join("hidden/tracked.txt"), "base\n").unwrap();
        test.run_git(&["add", "visible/tracked.txt", "hidden/tracked.txt"]);
        test.run_git(&["commit", "-m", "add sparse fixture"]);
        test.run_git(&["sparse-checkout", "init", "--cone"]);
        test.run_git(&["sparse-checkout", "set", "visible"]);

        std::fs::create_dir_all(test.root_path().join("hidden")).unwrap();
        std::fs::write(test.root_path().join("hidden/loose.txt"), "loose\n").unwrap();

        test
    }

    #[test]
    fn working_tree_diff_stats_with_untracked_crosses_sparse_checkout_boundary() {
        let test = sparse_checkout_with_untracked_file();

        let repo = Repository::at(test.root_path()).unwrap();
        let stats = repo
            .current_worktree()
            .working_tree_diff_stats_with_untracked()
            .unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(stats.deleted, 0);
    }

    #[test]
    fn temp_index_stage_all_preserves_sparse_checkout_boundary() {
        let test = sparse_checkout_with_untracked_file();

        let repo = Repository::at(test.root_path()).unwrap();
        let idx = repo.current_worktree().temp_index().unwrap();
        let err = idx
            .stage(crate::config::StageMode::All)
            .expect_err("temporary staging must mirror real git add -A");
        let cmd_err = crate::git::CommandError::find_in(&err)
            .expect("sparse-boundary refusal should remain a CommandError");
        assert_eq!(cmd_err.command_string(), "git add -A -- .");
    }

    #[test]
    fn tracked_worktree_tree_crosses_sparse_checkout_boundary() {
        let test = sparse_checkout_with_untracked_file();
        std::fs::write(test.root_path().join("hidden/tracked.txt"), "changed\n").unwrap();

        let repo = Repository::at(test.root_path()).unwrap();
        let tree = repo
            .current_worktree()
            .write_tracked_worktree_tree()
            .unwrap();
        let files = test.git_output(&["ls-tree", "-r", "--name-only", &tree]);
        assert_eq!(
            files.lines().collect::<Vec<_>>(),
            ["file.txt", "hidden/tracked.txt", "visible/tracked.txt"]
        );
    }

    #[test]
    fn working_tree_diff_stats_with_untracked_unborn_head_is_command_error() {
        // With an unborn HEAD the tracked diff cannot resolve its comparison
        // revision. The failure must surface as a typed `CommandError`.
        let test = TestRepo::new();
        std::fs::write(test.root_path().join("new.txt"), "hello\n").unwrap();
        let repo = Repository::at(test.root_path()).unwrap();

        let err = repo
            .current_worktree()
            .working_tree_diff_stats_with_untracked()
            .unwrap_err();
        let cmd_err =
            crate::git::CommandError::find_in(&err).expect("error should carry a CommandError");
        assert!(
            cmd_err
                .command_string()
                .starts_with("git diff --numstat -z --find-renames --end-of-options HEAD")
        );
    }

    #[test]
    fn temp_index_tolerates_missing_real_index() {
        // A worktree whose `<gitdir>/index` file is absent must not error
        // when callers ask for a temp index — git itself treats a missing
        // index as empty, and the WorkingTreeConflictsTask used to surface
        // this as a misleading `working-tree conflict check (Failed to copy
        // index file)` footer.
        let test = TestRepo::with_initial_commit();
        std::fs::write(test.root_path().join("tracked.txt"), "hello\n").unwrap();
        std::fs::write(test.root_path().join("untracked.txt"), "world\n").unwrap();

        let repo = Repository::at(test.root_path()).unwrap();
        let wt = repo.worktree_at(test.root_path());
        let real_index = wt.git_dir().unwrap().join("index");
        std::fs::remove_file(&real_index).unwrap();
        assert!(!real_index.exists(), "precondition: real index removed");

        // (a) temp_index() succeeds without <gitdir>/index.
        let idx = wt
            .temp_index()
            .expect("temp_index tolerates missing real index");

        // (b) git add -A against the resulting temp index produces a tree
        // containing the working-tree files.
        idx.stage(crate::config::StageMode::All).unwrap();
        let tree_sha = idx.write_tree().unwrap();
        let ls_tree = Cmd::new("git")
            .args(["ls-tree", "-r", "--name-only", &tree_sha])
            .current_dir(test.root_path())
            .run()
            .unwrap();
        let mut names: Vec<&str> = std::str::from_utf8(&ls_tree.stdout)
            .unwrap()
            .lines()
            .collect();
        names.sort();
        assert_eq!(names, vec!["file.txt", "tracked.txt", "untracked.txt"]);

        // (c) the real index is still absent afterward.
        assert!(
            !real_index.exists(),
            "temp_index must not resurrect the real index"
        );
    }
}
