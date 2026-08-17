//! Branch - a handle for branch-specific git operations.

use super::Repository;

/// Whether `refs/heads/<name>` is a well-formed ref, which is what a branch is.
///
/// A worktree selector is tried as a branch first and as a path second (see
/// [`Repository::resolve_worktree`](super::Repository::resolve_worktree)), so
/// when neither matches, the error has to pick which of the two the user meant.
/// This answers it: a name git could never put under `refs/heads/` — an
/// absolute path, `~/…`, `./…`, a Windows drive letter — was only ever a path,
/// and reporting it as a missing branch sends the user to `wt list --branches`
/// for something that would never appear there.
///
/// The rules are git's own, as `git check-ref-format refs/heads/<name>` applies
/// them; `branch_name_matches_git_check_ref_format` pins each one against that
/// command. In-process because it runs wherever a selector fails to resolve,
/// where a subprocess would buy nothing.
pub fn is_valid_branch_name(name: &str) -> bool {
    if name.is_empty() || name.ends_with('.') || name.contains("..") || name.contains("@{") {
        return false;
    }
    if name.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return false;
    }
    // An empty component covers a leading `/`, a trailing `/`, and `//`.
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// A handle for running git commands on a specific branch.
///
/// This type holds a reference to [`Repository`] and a branch name.
/// All branch-specific operations (like `exists`, `upstream`) are on this type.
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current()?;
/// let branch = repo.branch("feature");
///
/// // Branch-specific operations
/// let _ = branch.exists_locally();
/// let _ = branch.upstream();
/// let _ = branch.remotes();
///
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug)]
#[must_use]
pub struct Branch<'a> {
    pub(super) repo: &'a Repository,
    pub(super) name: String,
}

impl<'a> Branch<'a> {
    /// Get the branch name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if this branch exists locally.
    pub fn exists_locally(&self) -> anyhow::Result<bool> {
        Ok(self
            .repo
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", self.name),
            ])
            .is_ok())
    }

    /// Check if this branch exists (local or remote).
    ///
    /// Checks all remotes, matching git's default behavior for `git checkout`.
    pub fn exists(&self) -> anyhow::Result<bool> {
        // Try local branch first
        if self.exists_locally()? {
            return Ok(true);
        }

        // Check if any remote has this branch
        Ok(!self.remotes()?.is_empty())
    }

    /// Find which remotes have this branch.
    ///
    /// Returns a list of remote names that have this branch (e.g., `["origin"]`).
    /// Returns an empty list if no remotes have this branch.
    ///
    /// Filters the repository's remote-branch inventory (see
    /// [`Repository::remote_branches`]); the first call within a command
    /// triggers the `refs/remotes/` scan that populates the inventory.
    ///
    /// [`Repository::remote_branches`]: super::Repository::remote_branches
    pub fn remotes(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .repo
            .remote_branches()?
            .iter()
            .filter(|r| r.local_name == self.name)
            .map(|r| r.remote_name.clone())
            .collect())
    }

    /// Get the upstream tracking branch for this branch.
    ///
    /// Reads from the repository's local-branch inventory (see
    /// [`Repository::local_branches`]). The first call within a command
    /// triggers the `refs/heads/` scan that populates the inventory;
    /// subsequent lookups are O(1). Returns `None` when no upstream is
    /// configured, when no local branch by this name exists, or when the
    /// configured upstream is gone from its remote (git's `[gone]` track
    /// state).
    ///
    /// [`Repository::local_branches`]: super::Repository::local_branches
    pub fn upstream(&self) -> anyhow::Result<Option<String>> {
        Ok(self
            .repo
            .local_branch(&self.name)?
            .and_then(|b| b.upstream_short.clone()))
    }

    /// Unset the upstream tracking branch for this branch.
    ///
    /// This removes the tracking relationship, preventing accidental pushes
    /// to the wrong branch (e.g., when a feature branch was created from origin/main).
    pub fn unset_upstream(&self) -> anyhow::Result<()> {
        // `--` separates the option from the positional branch name so a
        // hyphen-prefixed branch cannot be misread as a flag.
        self.repo
            .run_command(&["branch", "--unset-upstream", "--", &self.name])?;
        Ok(())
    }

    /// Get the URL of the remote where this branch would be pushed.
    ///
    /// Uses `%(push:remotename)` which returns either a remote name or URL directly
    /// (`gh pr checkout` sets pushremote to a URL rather than a remote name).
    /// For remote names, uses `effective_remote_url` to apply `url.insteadOf` rewrites.
    /// Returns `None` if no push remote is configured or the remote has no URL.
    ///
    /// Result is cached on `RepoCache.push_remote_urls`. The CI-status detector
    /// calls this from both the PR-based path and the branch fallback, so
    /// without the cache the `for-each-ref` runs twice for the same branch
    /// on the no-PR path.
    pub fn push_remote_url(&self) -> Option<String> {
        self.repo
            .cache
            .push_remote_urls
            .entry(self.name.clone())
            .or_insert_with(|| {
                // %(push:remotename) returns either a remote name or URL
                // directly. Unlike @{push}, this doesn't fail when pushremote
                // is a URL.
                let push_remote = self
                    .repo
                    .run_command(&[
                        "for-each-ref",
                        "--format=%(push:remotename)",
                        &format!("refs/heads/{}", self.name),
                    ])
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())?;

                if push_remote.contains("://") || push_remote.starts_with("git@") {
                    Some(push_remote)
                } else {
                    self.repo.effective_remote_url(&push_remote)
                }
            })
            .clone()
    }
}
