//! Branch - a borrowed handle for branch-specific git operations.

use super::Repository;

/// A borrowed handle for running git commands on a specific branch.
///
/// This type borrows a [`Repository`] and holds a branch name.
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
        let remotes: Vec<String> = output
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                // Strip the branch suffix to get the remote name
                line.strip_suffix(&format!("/{}", self.name))
                    .map(String::from)
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
}
