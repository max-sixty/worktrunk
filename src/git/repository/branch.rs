//! Branch - a handle for branch-specific git operations.

use super::Repository;

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
    pub fn exists(&self) -> anyhow::Result<bool> {
        // Try local branch first
        if self.exists_locally()? {
            return Ok(true);
        }

        // Try remote branch (if remotes exist)
        let Ok(remote) = self.repo.primary_remote() else {
            return Ok(false);
        };
        Ok(self
            .repo
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{}/{}", remote, self.name),
            ])
            .is_ok())
    }

    /// Find which remotes have this branch.
    ///
    /// Returns a list of remote names that have this branch (e.g., `["origin"]`).
    /// Returns an empty list if no remotes have this branch.
    pub fn remotes(&self) -> anyhow::Result<Vec<String>> {
        // Get all remote tracking branches matching this name
        // Format: refs/remotes/<remote>/<branch>
        let output = self.repo.run_command(&[
            "for-each-ref",
            "--format=%(refname:strip=2)",
            &format!("refs/remotes/*/{}", self.name),
        ])?;

        // Parse output: each line is "<remote>/<branch>"
        // Extract the remote name (everything before the last /<branch>)
        let suffix = format!("/{}", self.name);
        let remotes: Vec<String> = output
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                // Strip the branch suffix to get the remote name
                line.strip_suffix(&suffix).map(String::from)
            })
            .collect();

        Ok(remotes)
    }

    /// Get the upstream tracking branch for this branch.
    ///
    /// Uses [`@{upstream}` syntax][1] to resolve the tracking branch.
    ///
    /// [1]: https://git-scm.com/docs/gitrevisions#Documentation/gitrevisions.txt-emltaboranchgtemuaboranchgtupaboranchgtupstream
    pub fn upstream(&self) -> anyhow::Result<Option<String>> {
        let result =
            self.repo
                .run_command(&["rev-parse", "--abbrev-ref", &format!("{}@{{u}}", self.name)]);

        match result {
            Ok(upstream) => {
                let trimmed = upstream.trim();
                Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
            }
            Err(_) => Ok(None), // No upstream configured
        }
    }

    /// Get the remote where this branch would be pushed.
    ///
    /// Uses [`@{push}` syntax][1] which resolves through:
    /// 1. `branch.<name>.pushRemote` (branch-specific push remote)
    /// 2. `remote.pushDefault` (default push remote for all branches)
    /// 3. `branch.<name>.remote` (tracking remote)
    ///
    /// Returns `None` if no push destination is configured.
    ///
    /// [1]: https://git-scm.com/docs/gitrevisions#Documentation/gitrevisions.txt-emltbraboranchgtpaboranchgtpush
    pub fn push_remote(&self) -> Option<String> {
        let push_ref = self
            .repo
            .run_command(&[
                "rev-parse",
                "--abbrev-ref",
                &format!("{}@{{push}}", self.name),
            ])
            .ok()?;

        // Returns "origin/branch", extract remote name
        let remote = push_ref.trim().split('/').next()?;
        (!remote.is_empty()).then(|| remote.to_string())
    }
}
