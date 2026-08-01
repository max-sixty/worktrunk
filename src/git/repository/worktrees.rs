//! Worktree management operations for Repository.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::Context as _;
use color_print::cformat;
use dunce::canonicalize;

use super::{GitError, Repository, ResolvedWorktree, WorktreeInfo, resolve_input_path};
use crate::path::{format_path_for_display, paths_match};
use crate::styling::{
    eprintln, format_with_gutter, hint_message, suggest_command, warning_message,
};

impl Repository {
    /// List all worktrees for this repository.
    ///
    /// Returns a list of worktrees with bare entries filtered out.
    ///
    /// **Ordering:** Git lists the main worktree first. For normal repos, `[0]` is
    /// the main worktree. For bare repos, the bare entry is filtered out, so `[0]`
    /// is the first linked worktree (no semantic "main" exists).
    ///
    /// Returns an empty slice for bare repos with no linked worktrees.
    ///
    /// Cached on `RepoCache` after the first successful call; subsequent calls
    /// return a reference into the cache. See the module-level `# Caching` docs
    /// for the "no post-mutation reads through the cache" invariant.
    pub fn list_worktrees(&self) -> anyhow::Result<&[WorktreeInfo]> {
        self.cache
            .worktrees
            .get_or_try_init(|| {
                let stdout = self.run_command(&["worktree", "list", "--porcelain"])?;
                let raw_worktrees = WorktreeInfo::parse_porcelain_list(&stdout)?;
                let mut worktrees: Vec<_> =
                    raw_worktrees.into_iter().filter(|wt| !wt.bare).collect();

                // Submodule path correction.
                //
                // Git's `get_main_worktree()` computes the main worktree path by stripping
                // a trailing `/.git` from the common dir. For submodules, the common dir is
                // `.git/modules/sub` (no trailing `/.git`), so git leaves it unchanged —
                // reporting the git data directory as the "main worktree" path. Git does not
                // consult `core.worktree` in this code path.
                //
                // We detect this by checking whether the first worktree's path equals
                // git_common_dir (which never holds for normal repos, where git_common_dir
                // is `.git` inside the worktree). When matched, we correct it using
                // repo_path(), which reads `core.worktree` from the bulk config map.
                //
                // We fix this here rather than at each call site because list_worktrees()
                // is the single point where worktree paths enter the system — all consumers
                // (worktree_for_branch, resolve_worktree, etc.) depend on paths being
                // working directories. If git fixes this upstream, the condition stops
                // triggering.
                if let Some(first) = worktrees.first_mut()
                    && canonicalize(&first.path).ok().as_deref() == Some(self.git_common_dir())
                {
                    first.path = self.repo_path()?.to_path_buf();
                }

                Ok(worktrees)
            })
            .map(Vec::as_slice)
    }

    /// Find the worktree path for a given branch, if one exists.
    ///
    /// A branch normally maps to at most one worktree, but `git worktree add
    /// --force <path> <branch>` bypasses git's "already used by worktree" guard
    /// and lets the same branch live in several at once. Worktrunk never creates
    /// that state; when it exists, this resolves to the first worktree git lists
    /// (roughly creation order) and warns once per branch so the otherwise-silent
    /// choice is visible. See `warn_duplicate_checkout`.
    pub fn worktree_for_branch(&self, branch: &str) -> anyhow::Result<Option<PathBuf>> {
        let worktrees = self.list_worktrees()?;
        let paths = worktree_paths_for_branch(worktrees, branch);
        if paths.len() > 1 {
            warn_duplicate_checkout(branch, &paths);
        }
        Ok(paths.into_iter().next())
    }

    /// The "home" worktree — main worktree for normal repos, default branch worktree for bare.
    ///
    /// Used as the default source for `copy-ignored` and the `{{ primary_worktree_path }}` template.
    /// Returns `None` for bare repos when no worktree has the default branch.
    pub fn primary_worktree(&self) -> anyhow::Result<Option<PathBuf>> {
        if self.is_bare()? {
            let Some(branch) = self.default_branch() else {
                return Ok(None);
            };
            self.worktree_for_branch(&branch)
        } else {
            Ok(Some(self.repo_path()?.to_path_buf()))
        }
    }

    /// Find the worktree at a given path, returning its branch if known.
    ///
    /// Returns `Some((path, branch))` if a worktree exists at the path,
    /// where `branch` is `None` for detached HEAD worktrees.
    ///
    /// Uses symlink-aware comparison so a path that resolves to a worktree
    /// through one or more symlinks still matches the worktree's recorded path.
    pub fn worktree_at_path(
        &self,
        path: &Path,
    ) -> anyhow::Result<Option<(PathBuf, Option<String>)>> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .iter()
            .find(|wt| paths_match(&wt.path, path))
            .map(|wt| (wt.path.clone(), wt.branch.clone())))
    }

    /// Unregister the one worktree at `path`, whose directory is already gone.
    ///
    /// Git tracks worktrees in `.git/worktrees/<id>/`. When a worktree's
    /// directory disappears — deleted externally, or renamed into the trash by
    /// [`stage_worktree_removal`](crate::git::remove::stage_worktree_removal) —
    /// that admin dir is stale and has to go. `git worktree remove` skips its
    /// clean check when the directory is missing and deletes just that entry,
    /// so it is the scoped spelling of the cleanup.
    ///
    /// # Why not `git worktree prune`
    ///
    /// `git worktree prune` takes no path filter: it walks *every* entry and
    /// unregisters each one whose directory it cannot find at that instant. A
    /// worktree that is merely absent right now — an unmounted volume, a
    /// dropped network mount, a half-finished `mv` — is indistinguishable from
    /// a deleted one, so removing worktree A would also unregister bystander
    /// B. B's committed work survives (refs and objects are shared) and its
    /// files stay on disk, but its admin dir does not: the index, `ORIG_HEAD`,
    /// the per-worktree reflog, `refs/worktree/*` and `refs/bisect/*`, and any
    /// in-progress rebase or merge all go with it. `git worktree repair`
    /// cannot rebuild them — it relinks an admin dir, it does not recreate
    /// one. Naming the target keeps a removal's blast radius equal to its
    /// intent.
    ///
    /// # Locked worktrees
    ///
    /// A repo-wide prune silently skips a locked entry; `git worktree remove`
    /// fails on one instead. Every call site has already established the target
    /// is unlocked, by a different route each: `prepare_worktree_removal`
    /// returns `WorktreeLocked` at the top of its path/current arm and rejects
    /// a locked worktree before staging one for removal, and prune's
    /// `gather_check_items` never selects one as a candidate.
    ///
    /// # Concurrent calls
    ///
    /// `wt step prune` removes several entries at once, but **serializes the
    /// teardowns** — this and every other `git worktree remove` — behind
    /// `RemovalContext::registry_lock` (see the `prune` module). It has to:
    /// naming one entry bounds what a call *deletes*, not what it *reads*.
    /// `git worktree remove` enumerates *every* sibling under `.git/worktrees/`
    /// and reads each one's `commondir` while resolving its target, so a
    /// teardown overlapping another worker's teardown — or a branch delete's
    /// `list_worktrees` probe — can read an entry mid-deletion and fail
    /// (`failed to read …/commondir` / `Invalid path …/.git/worktrees/<id>`).
    /// That is git's own TOCTOU between the enumerator's `readdir` and its
    /// `open`; it holds however wt schedules its removals, so wt closes the
    /// window by not letting two registry mutations overlap (issue #3661).
    ///
    /// Git also `rmdir`s the containing `.git/worktrees` once the last entry
    /// goes, but that only succeeds on an already-empty directory, and every
    /// per-worktree command in the removal chain (`git status`, the fsmonitor
    /// stop) runs while that worktree is still registered, so an emptied
    /// directory has no in-flight reader left to strand. A `git worktree list`
    /// in an *unrelated* process — outside wt's lock — remains exposed to the
    /// same `Invalid path` race; wt's serialization only covers its own
    /// removals.
    pub fn prune_worktree_entry(&self, path: &Path) -> anyhow::Result<()> {
        // Every caller's path came from `list_worktrees`, which parses git's
        // porcelain as UTF-8, so this only fires if that edge ever stops
        // guaranteeing it — a bare `?` rather than a rendered path.
        let path_str = path.to_str().context("worktree path is not valid UTF-8")?;
        self.run_command(&["worktree", "remove", path_str])?;
        Ok(())
    }

    /// Remove a worktree at the specified path.
    ///
    /// When `force` is true, passes `--force` to `git worktree remove`,
    /// allowing removal even when the worktree contains untracked files
    /// (like build artifacts such as `.vite/` or `node_modules/`).
    ///
    /// When the worktree contains initialized submodules, git refuses removal
    /// even for clean worktrees. This method detects that case up front and
    /// adds `--force`, which is safe because the caller has already validated
    /// worktree cleanliness via `ensure_clean()`.
    ///
    /// # Why git requires `--force` for submodules
    ///
    /// Git's `--force` flag on `worktree remove` bypasses two unrelated
    /// protections under one flag: dirty working tree checks AND the
    /// submodule structural check. We separate these concerns — our
    /// `ensure_clean()` handles dirty state, and `--force`
    /// handles the submodule restriction.
    ///
    /// # TOCTOU note
    ///
    /// Git checks for submodules *before* checking for dirty files, so when
    /// we synthesize `--force` for a submodule worktree, git's own dirty
    /// check is bypassed. To keep failing closed on a file modified after the
    /// caller's `ensure_clean()`, this method re-runs `ensure_clean()` itself
    /// immediately before the synthesized-force removal — restoring the
    /// backstop git would otherwise have provided. (For an explicit
    /// user-requested `force`, the user opted into destructive removal, so no
    /// re-check.)
    pub fn remove_worktree(&self, path: &std::path::Path, force: bool) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::Error::from(GitError::Other {
                message: format!(
                    "Worktree path contains invalid UTF-8: {}",
                    format_path_for_display(path)
                ),
            })
        })?;
        let use_force = if force {
            true
        } else {
            self.worktree_at(path).has_initialized_submodules()?
        };
        if use_force && !force {
            // Synthesized force (submodule worktree, not user-requested).
            // `--force` will suppress git's dirty check, so re-validate
            // cleanliness ourselves right before the destructive command —
            // a file modified since the caller's check must still fail
            // closed rather than be silently destroyed.
            self.worktree_at(path).ensure_clean(
                "remove worktree with submodules",
                None,
                /* force_hint */ true,
            )?;
            tracing::debug!("Using --force for worktree removal due to initialized submodules");
        }
        let mut args = vec!["worktree", "remove"];
        if use_force {
            args.push("--force");
        }
        args.push(path_str);

        self.run_command(&args)?;
        Ok(())
    }

    /// Resolve a worktree name, expanding "@" to current, "-" to previous, and "^" to the default branch.
    ///
    /// # Arguments
    /// * `name` - The worktree name to resolve:
    ///   - "@" for current HEAD
    ///   - "-" for previous branch (via worktrunk.history)
    ///   - "^" for default branch
    ///   - any other string is returned as-is
    ///
    /// # Returns
    /// - `Ok(name)` if not a special symbol
    /// - `Ok(current_branch)` if "@" and on a branch
    /// - `Ok(previous_branch)` if "-" and worktrunk.history has a previous branch
    /// - `Ok(default_branch)` if "^"
    /// - `Err(DetachedHead)` if "@" and in detached HEAD state
    /// - `Err` if "-" but no previous branch in history
    pub fn resolve_worktree_name(&self, name: &str) -> anyhow::Result<String> {
        match name {
            "@" => self.current_worktree().branch()?.ok_or_else(|| {
                GitError::DetachedHead {
                    action: Some("resolve @ to current branch".into()),
                }
                .into()
            }),
            "-" => {
                // Read from worktrunk.history (recorded by wt switch operations)
                self.switch_previous().ok_or_else(|| {
                    GitError::Other {
                        message: cformat!(
                            "No previous branch found in history. Run <underline>wt list</> to see available worktrees."
                        ),
                    }
                    .into()
                })
            }
            "^" => self.default_branch().ok_or_else(|| {
                GitError::Other {
                    message: cformat!(
                        "Cannot determine default branch. Specify target explicitly or run <underline>wt config state default-branch set <bold>BRANCH</></>"
                    ),
                }
                .into()
            }),
            _ => Ok(name.to_string()),
        }
    }

    /// Resolve a worktree selector — the one place a token the user typed
    /// becomes a worktree.
    ///
    /// Every argument that names a worktree routes through here, so they all
    /// accept the same vocabulary: the shortcuts, a branch name, and the path
    /// of the worktree itself. wt addresses worktrees by branch (see the
    /// "Worktree Model" section of `CLAUDE.md`), so the branch is tried first
    /// and a path only answers what a branch name cannot — a detached worktree,
    /// or one of several checkouts of the same branch.
    ///
    /// Resolution order:
    /// 1. `@` — the current worktree, matched by path so detached HEAD resolves
    /// 2. `-` / `^` — the previous / default branch, then as a branch below
    /// 3. a branch with a worktree
    /// 4. a path naming a registered worktree — absolute, `~`-relative, or
    ///    relative to `-C` (see [`resolve_input_path`])
    /// 5. otherwise the branch alone, which may or may not exist
    ///
    /// # Returns
    /// - `Worktree { path, branch }` — `branch` is `None` for a detached worktree
    /// - `BranchOnly { branch }` when nothing is checked out under that name
    pub fn resolve_worktree(&self, name: &str) -> anyhow::Result<ResolvedWorktree> {
        match name {
            "@" => {
                // Current worktree by path - works even in detached HEAD
                // If worktree_root fails (e.g., in bare repo directory), give a clear error
                let path = self
                    .current_worktree()
                    .root()
                    .map_err(|_| GitError::NotInWorktree {
                        action: Some("resolve @".into()),
                    })?;
                // root() returns canonicalized path, so canonicalize worktree paths
                // for comparison to handle symlinks (e.g., macOS /var -> /private/var)
                let worktrees = self.list_worktrees()?;
                let branch = worktrees
                    .iter()
                    .find(|wt| canonicalize(&wt.path).map(|p| p == path).unwrap_or(false))
                    .and_then(|wt| wt.branch.clone());
                Ok(ResolvedWorktree::Worktree { path, branch })
            }
            _ => {
                let branch = self.resolve_worktree_name(name)?;
                if let Some(path) = self.worktree_for_branch(&branch)? {
                    return Ok(ResolvedWorktree::Worktree {
                        path,
                        branch: Some(branch),
                    });
                }

                // A shortcut named a branch, not a directory: `resolve_worktree_name`
                // returns a non-shortcut token unchanged, so an unequal result is
                // exactly the case where the literal token would be a nonsense path.
                if branch == name
                    && let Some((path, wt_branch)) = self.worktree_at_input_path(name)?
                {
                    return Ok(ResolvedWorktree::Worktree {
                        path,
                        branch: wt_branch,
                    });
                }

                Ok(ResolvedWorktree::BranchOnly { branch })
            }
        }
    }

    /// The branch a selector names, erroring only when it names a detached
    /// worktree.
    ///
    /// [`resolve_worktree`](Self::resolve_worktree) for the arguments that want
    /// a branch rather than a worktree — `wt step promote`, and the `--branch`
    /// of the `wt config state` commands, which key state by branch name. A
    /// branch with no worktree is a fine answer to those; a worktree named by
    /// path that has no branch is not.
    pub fn require_selected_branch(&self, name: &str, action: &str) -> anyhow::Result<String> {
        match self.resolve_worktree(name)? {
            ResolvedWorktree::Worktree {
                branch: Some(branch),
                ..
            }
            | ResolvedWorktree::BranchOnly { branch } => Ok(branch),
            ResolvedWorktree::Worktree { path, branch: None } => Err(GitError::DetachedHead {
                action: Some(cformat!(
                    "{action} — <bold>{}</> is detached",
                    format_path_for_display(&path)
                )),
            }
            .into()),
        }
    }

    /// The path of the worktree a selector names, erroring when it names none.
    ///
    /// [`resolve_worktree`](Self::resolve_worktree) for the commands that need a
    /// worktree to operate in rather than a branch to reason about — `wt step
    /// diff --branch`, `copy-ignored --from`/`--to`, `promote`. A branch with no
    /// checkout and a name matching nothing at all are the same answer to them.
    /// A selector matching nothing at all is reported as such, rather than as a
    /// branch without a worktree: `wt switch <the selector>` creates a worktree
    /// only when the branch exists, so offering it for a mistyped path would
    /// just fail again.
    pub fn require_worktree(&self, name: &str) -> anyhow::Result<PathBuf> {
        match self.resolve_worktree(name)? {
            ResolvedWorktree::Worktree { path, .. } => Ok(path),
            ResolvedWorktree::BranchOnly { branch } => Err(self.no_worktree_error(branch)),
        }
    }

    /// The error for a selector that resolved to a branch with no checkout.
    fn no_worktree_error(&self, branch: String) -> anyhow::Error {
        match self.branch(&branch).exists_locally() {
            Ok(true) => GitError::WorktreeNotFound { branch }.into(),
            // A ref lookup that fails says nothing about the branch, so fall
            // back to the message that claims less.
            _ => GitError::WorktreeSelectorNotFound { selector: branch }.into(),
        }
    }

    /// The worktree a user-supplied token names by path, if it names one.
    ///
    /// The path half of [`resolve_worktree`](Self::resolve_worktree), split out
    /// for arguments that want a branch rather than a worktree — a merge target,
    /// a state key. Resolving the token is the whole point: it goes through
    /// [`resolve_input_path`] here so no call site
    /// has to remember that a relative path answers to `-C` and a leading `~` to
    /// the home directory.
    ///
    /// Branch-first is the rule everywhere, so callers reach for this only after
    /// a branch or ref lookup has already come up empty. `branch` is `None` for
    /// a detached worktree.
    pub fn worktree_at_input_path(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<(PathBuf, Option<String>)>> {
        self.worktree_at_path(&resolve_input_path(name))
    }

    /// Find the "home" path - where to cd when leaving a worktree.
    ///
    /// Returns the primary worktree if it exists, otherwise the repo root.
    /// - Normal repos: the main worktree (repo root)
    /// - Bare repos: the default branch's worktree, or the bare repo directory
    pub fn home_path(&self) -> anyhow::Result<PathBuf> {
        self.primary_worktree()?
            .map_or_else(|| self.repo_path().map(|p| p.to_path_buf()), Ok)
    }
}

/// Paths of every worktree checked out on `branch`, in git's listing order.
///
/// At most one under normal use; more than one only when the user ran
/// `git worktree add --force <path> <branch>`, which bypasses git's
/// "already used by worktree" guard. Worktrunk never creates that state.
pub(crate) fn worktree_paths_for_branch(worktrees: &[WorktreeInfo], branch: &str) -> Vec<PathBuf> {
    worktrees
        .iter()
        .filter(|wt| wt.branch.as_deref() == Some(branch))
        .map(|wt| wt.path.clone())
        .collect()
}

/// Every branch checked out in more than one worktree.
///
/// The all-at-once counterpart to `worktree_paths_for_branch`, for callers
/// classifying the whole list rather than resolving one branch — `wt list`
/// flags each affected row with `⚑` so the ambiguity is visible before a
/// command resolves the branch and warns.
pub fn duplicated_branches(worktrees: &[WorktreeInfo]) -> HashSet<&str> {
    let mut seen = HashSet::new();
    let mut duplicated = HashSet::new();
    for branch in worktrees.iter().filter_map(|wt| wt.branch.as_deref()) {
        if !seen.insert(branch) {
            duplicated.insert(branch);
        }
    }
    duplicated
}

/// Warn once per process that `branch` resolves ambiguously across worktrees.
///
/// Worktrunk addresses worktrees by branch name and resolves an ambiguous
/// branch to the first worktree git lists, leaving the others reachable only by
/// path. The warning surfaces that otherwise-silent choice — naming every
/// path — without changing which worktree is used. Deduplicated per branch so
/// a command that resolves the same branch repeatedly (the picker, `wt list`)
/// warns only once.
///
/// Called only from `worktree_for_branch` with `paths.len() > 1`, so `paths[1..]`
/// names at least one shadowed worktree.
fn warn_duplicate_checkout(branch: &str, paths: &[PathBuf]) {
    static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    // A poisoned set of already-warned branches never justifies aborting; recover it.
    let is_new = WARNED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(branch.to_string());
    if is_new {
        let listing = paths
            .iter()
            .map(|p| format_path_for_display(p))
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Branch <bold>{branch}</> is checked out in {} worktrees; wt uses the first:",
                paths.len()
            ))
        );
        eprintln!("{}", format_with_gutter(&listing, None));
        // Name every shadowed worktree so removing them all is actionable; with a
        // single extra (the common case) this is one hint. `wt remove <path>`
        // removes exactly the worktree named and retains the branch the others
        // still hold, so it's safe to suggest for a duplicate.
        for extra in &paths[1..] {
            let cmd = suggest_command("remove", &[&format_path_for_display(extra)], &[]);
            eprintln!(
                "{}",
                hint_message(cformat!("To drop a duplicate, run <underline>{cmd}</>"))
            );
        }
    }
}
