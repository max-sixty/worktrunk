//! Shared test fixtures for worktrunk unit tests.
//!
//! Provides lightweight git repository fixtures for tests that need a real
//! `.git` directory (template expansion, config resolution, work item generation).
//!
//! This module is `#[doc(hidden)] pub` so both library (`src/`) and binary
//! (`src/commands/`) unit tests can use it — `#[cfg(test)]` modules are only
//! visible within their own crate.
//!
//! For integration tests, use `tests/common/mod.rs` (`TestRepo`) instead — it
//! provides full CLI isolation, snapshot filters, and mock commands.

use std::path::Path;

use crate::git::Repository;
use crate::shell_exec::Cmd;

/// Minimal test git repository backed by a temp directory.
///
/// The temp directory is cleaned up when this struct is dropped.
pub struct TestRepo {
    _dir: tempfile::TempDir,
    pub repo: Repository,
}

impl TestRepo {
    /// Create a new empty repo with `git init -b main` and test identity.
    ///
    /// Uses explicit `-b main` for determinism regardless of system git config.
    /// Identity is always configured so callers can commit without extra setup.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Cmd::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .run()
            .unwrap();
        let repo = Repository::at(dir.path()).unwrap();
        repo.run_command(&["config", "user.name", "Test"]).unwrap();
        repo.run_command(&["config", "user.email", "test@test.com"])
            .unwrap();
        Self { _dir: dir, repo }
    }

    /// Create a repo with one initial commit on `main`.
    ///
    /// Equivalent to `new()` followed by creating a file and committing it.
    /// Use this when tests need a non-empty repo (e.g. for branching or
    /// worktree operations that require at least one commit).
    pub fn with_initial_commit() -> Self {
        let test = Self::new();
        std::fs::write(test.path().join("file.txt"), "hello").unwrap();
        test.repo.run_command(&["add", "."]).unwrap();
        test.repo.run_command(&["commit", "-m", "init"]).unwrap();
        test
    }

    /// Path to the repository working directory.
    pub fn path(&self) -> &Path {
        self._dir.path()
    }
}

/// Set git user identity on a repository.
///
/// Use this for tests that manage their own repo creation (not via
/// [`TestRepo`]) and need identity configured for commits.
pub fn set_test_identity(repo: &Repository) {
    repo.run_command(&["config", "user.name", "Test"]).unwrap();
    repo.run_command(&["config", "user.email", "test@test.com"])
        .unwrap();
}
