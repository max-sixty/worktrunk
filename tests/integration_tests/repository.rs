//! Tests for git repository methods to improve code coverage.

use std::fs;

use worktrunk::git::Repository;

use crate::common::{BareRepoTest, TestRepo};

// =============================================================================
// is_bare() tests
// =============================================================================

/// When `core.bare` is unset (e.g., repos cloned by Eclipse/EGit), `is_bare()`
/// must return `false`. Before the fix for #1939, `git rev-parse
/// --is-bare-repository` was used, which infers `true` from inside `.git/`
/// when `core.bare` is absent.
#[test]
fn test_is_bare_returns_false_when_core_bare_unset() {
    let repo = TestRepo::new();

    // Simulate a repo where core.bare was never written (e.g., Eclipse/EGit)
    repo.run_git(&["config", "--unset", "core.bare"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert!(
        !repository.is_bare().unwrap(),
        "repo with unset core.bare should not be detected as bare"
    );
}

#[test]
fn test_is_bare_returns_false_for_normal_repo() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert!(!repository.is_bare().unwrap());
}

#[test]
fn test_is_bare_returns_true_for_bare_repo() {
    let test = BareRepoTest::new();
    let repository = Repository::at(test.bare_repo_path().to_path_buf()).unwrap();
    assert!(repository.is_bare().unwrap());
}

// =============================================================================
// worktree_state() tests - simulate various git operation states
// =============================================================================

#[test]
fn test_worktree_state_normal() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Normal state - no special files
    let state = repository.worktree_state().unwrap();
    assert!(state.is_none());
}

#[test]
fn test_worktree_state_merging() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate merge state by creating MERGE_HEAD
    let git_dir = repo.root_path().join(".git");
    fs::write(git_dir.join("MERGE_HEAD"), "abc123\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("MERGING".to_string()));
}

#[test]
fn test_worktree_state_rebasing_with_progress() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate rebase state with progress
    let git_dir = repo.root_path().join(".git");
    let rebase_dir = git_dir.join("rebase-merge");
    fs::create_dir_all(&rebase_dir).unwrap();
    fs::write(rebase_dir.join("msgnum"), "2\n").unwrap();
    fs::write(rebase_dir.join("end"), "5\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("REBASING 2/5".to_string()));
}

#[test]
fn test_worktree_state_rebasing_apply() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate rebase-apply state (git am or git rebase without -m)
    let git_dir = repo.root_path().join(".git");
    let rebase_dir = git_dir.join("rebase-apply");
    fs::create_dir_all(&rebase_dir).unwrap();
    fs::write(rebase_dir.join("msgnum"), "3\n").unwrap();
    fs::write(rebase_dir.join("end"), "7\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("REBASING 3/7".to_string()));
}

#[test]
fn test_worktree_state_rebasing_no_progress() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate rebase state without progress files
    let git_dir = repo.root_path().join(".git");
    let rebase_dir = git_dir.join("rebase-merge");
    fs::create_dir_all(&rebase_dir).unwrap();
    // No msgnum/end files - just the directory

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("REBASING".to_string()));
}

#[test]
fn test_worktree_state_cherry_picking() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate cherry-pick state
    let git_dir = repo.root_path().join(".git");
    fs::write(git_dir.join("CHERRY_PICK_HEAD"), "def456\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("CHERRY-PICKING".to_string()));
}

#[test]
fn test_worktree_state_reverting() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate revert state
    let git_dir = repo.root_path().join(".git");
    fs::write(git_dir.join("REVERT_HEAD"), "789abc\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("REVERTING".to_string()));
}

#[test]
fn test_worktree_state_bisecting() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Simulate bisect state
    let git_dir = repo.root_path().join(".git");
    fs::write(git_dir.join("BISECT_LOG"), "# bisect log\n").unwrap();

    let state = repository.worktree_state().unwrap();
    assert_eq!(state, Some("BISECTING".to_string()));
}

// =============================================================================
// available_branches() tests
// =============================================================================

#[test]
fn test_available_branches_all_have_worktrees() {
    let mut repo = TestRepo::new();
    // main branch already has a worktree (the main repo)
    // Create feature branch with worktree
    repo.add_worktree("feature");

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let available = repository.available_branches().unwrap();

    // Both main and feature have worktrees, so nothing should be available
    assert!(available.is_empty());
}

#[test]
fn test_available_branches_some_without_worktrees() {
    let repo = TestRepo::with_initial_commit();
    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let available = repository.available_branches().unwrap();

    // orphan-branch has no worktree, so it should be available
    assert!(available.contains(&"orphan-branch".to_string()));
    // main has a worktree, so it should not be available
    assert!(!available.contains(&"main".to_string()));
}

// =============================================================================
// all_branches() tests
// =============================================================================

#[test]
fn test_all_branches() {
    let repo = TestRepo::with_initial_commit();
    // Create some branches
    repo.git_command().args(["branch", "alpha"]).run().unwrap();
    repo.git_command().args(["branch", "beta"]).run().unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let branches = repository.all_branches().unwrap();

    assert!(branches.contains(&"main".to_string()));
    assert!(branches.contains(&"alpha".to_string()));
    assert!(branches.contains(&"beta".to_string()));
}

// =============================================================================
// project_identifier() URL parsing tests
// =============================================================================

#[test]
fn test_project_identifier_https() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");
    // Override the remote URL to https format
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "https://github.com/user/repo.git",
        ])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_http() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");
    // Override the remote URL to http format (no SSL)
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "http://gitlab.example.com/team/project.git",
        ])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "gitlab.example.com/team/project");
}

#[test]
fn test_project_identifier_ssh_colon() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");
    // Override the remote URL to SSH format with colon
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "git@github.com:user/repo.git",
        ])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_ssh_protocol() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");
    // Override the remote URL to ssh:// format
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "ssh://git@github.com/user/repo.git",
        ])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // ssh://git@github.com/user/repo.git -> github.com/user/repo
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_ssh_protocol_with_port() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");
    // Override the remote URL to ssh:// format with port
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "ssh://git@gitlab.example.com:2222/team/project.git",
        ])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // Port is stripped — irrelevant to project identity
    assert_eq!(id, "gitlab.example.com/team/project");
}

#[test]
fn test_project_identifier_no_remote_fallback() {
    let repo = TestRepo::with_initial_commit();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // Should be the full canonical path (security: avoids collisions across unrelated repos)
    let expected = dunce::canonicalize(repo.root_path()).unwrap();
    assert_eq!(id, expected.to_str().unwrap());
}

// =============================================================================
// config_value/set_config tests
// =============================================================================

#[test]
fn test_get_config_exists() {
    let repo = TestRepo::new();
    repo.git_command()
        .args(["config", "test.key", "test-value"])
        .run()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let value = repository.config_value("test.key").unwrap();
    assert_eq!(value, Some("test-value".to_string()));
}

#[test]
fn test_get_config_not_exists() {
    let repo = TestRepo::new();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let value = repository.config_value("nonexistent.key").unwrap();
    assert!(value.is_none());
}

#[test]
fn test_set_config() {
    let repo = TestRepo::new();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    repository.set_config("test.setting", "new-value").unwrap();

    // Verify it was set
    let value = repository.config_value("test.setting").unwrap();
    assert_eq!(value, Some("new-value".to_string()));
}

// =============================================================================
// config_value() error handling: corrupt config propagation
// =============================================================================

#[test]
fn test_config_value_propagates_error_on_corrupt_config() {
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();

    // Create repository before corrupting config
    let repository = Repository::at(root.clone()).unwrap();

    // Corrupt the git config file after repository creation
    let config_path = root.join(".git/config");
    fs::write(&config_path, "[invalid section\n").unwrap();

    let result = repository.config_value("test.key");

    // Should propagate the error, not silently return None
    assert!(
        result.is_err(),
        "config_value() should propagate errors from corrupt config, not return Ok(None)"
    );
}

#[test]
fn test_clear_hint_propagates_error_on_corrupt_config() {
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();

    // Create repository and set a hint before corrupting config
    let repository = Repository::at(root.clone()).unwrap();
    repository.mark_hint_shown("test-hint").unwrap();

    // Corrupt the git config file
    let config_path = root.join(".git/config");
    fs::write(&config_path, "[invalid section\n").unwrap();

    let result = repository.clear_hint("test-hint");

    // Should propagate the error, not silently return Ok(false)
    assert!(
        result.is_err(),
        "clear_hint() should propagate errors from corrupt config, not return Ok(false)"
    );
}

// =============================================================================
// Bulk config cache coverage
// =============================================================================

/// `mark_hint_shown` → `has_shown_hint` → `list_shown_hints` → `clear_hint`
/// exercises the full write-then-read round trip through the bulk config
/// cache, including coherent in-memory updates.
#[test]
fn test_hint_roundtrip_through_bulk_cache() {
    let repo = TestRepo::new();
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Populate bulk cache before the write so set/unset hit the in-memory
    // update paths.
    assert!(!r.is_bare().unwrap());

    r.mark_hint_shown("zebra").unwrap();
    r.mark_hint_shown("alpha").unwrap();
    assert!(r.has_shown_hint("zebra"));
    assert!(r.has_shown_hint("alpha"));
    assert!(!r.has_shown_hint("unknown"));

    // Deterministic alphabetical ordering (bulk cache is a HashMap — order
    // must be explicitly sorted for display).
    let hints = r.list_shown_hints();
    assert_eq!(hints, vec!["alpha".to_string(), "zebra".to_string()]);

    // Clear one → coherent in-memory removal.
    assert!(r.clear_hint("alpha").unwrap());
    assert!(!r.has_shown_hint("alpha"));
    assert!(r.has_shown_hint("zebra"));
    assert_eq!(r.list_shown_hints(), vec!["zebra".to_string()]);

    // Clear missing → Ok(false).
    assert!(!r.clear_hint("never-set").unwrap());
}

/// `primary_remote()` honours `checkout.defaultRemote` when it points at
/// a configured remote — covers the early-return branch in the new bulk
/// lookup.
#[test]
fn test_primary_remote_honours_checkout_default_remote() {
    let repo = TestRepo::new();
    repo.run_git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/max-sixty/worktrunk.git",
    ]);
    repo.run_git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/max-sixty/worktrunk.git",
    ]);
    repo.run_git(&["config", "checkout.defaultRemote", "upstream"]);

    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert_eq!(r.primary_remote().unwrap(), "upstream");
    // With no `checkout.defaultRemote`, falls back to the first remote
    // with a URL (the filter-out-phantom-entries path).
    repo.run_git(&["config", "--unset", "checkout.defaultRemote"]);
    let r2 = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert_eq!(r2.primary_remote().unwrap(), "origin");
}

/// `all_remote_urls()` enumerates every configured remote via the bulk
/// map, filtering out phantom entries (keys with `remote.X.*` that have
/// no `.url`).
#[test]
fn test_all_remote_urls_filters_phantom_remotes() {
    let repo = TestRepo::new();
    repo.run_git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/max-sixty/worktrunk.git",
    ]);
    // A phantom entry: remote.X.prunetags set but no URL → should not appear.
    repo.run_git(&["config", "remote.phantom.prunetags", "true"]);

    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let urls = r.all_remote_urls();
    assert_eq!(urls.len(), 1, "expected only origin, got {urls:?}");
    assert_eq!(urls[0].0, "origin");
}

/// `unset_config_value` cleanly removes in-memory state after the bulk
/// cache is populated. Guards against a regression where the in-memory
/// remove used the literal key instead of the canonical form.
#[test]
fn test_unset_config_removes_from_bulk_cache() {
    let repo = TestRepo::new();
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Populate cache, then write a mixed-case variable key (canonical
    // variable name is lowercased by git).
    let _ = r.is_bare();
    r.set_config("branch.main.pushRemote", "origin").unwrap();
    assert_eq!(
        r.config_value("branch.main.pushRemote").unwrap(),
        Some("origin".to_string())
    );

    // Unset removes it — subsequent reads return None.
    assert!(r.unset_config("branch.main.pushRemote").unwrap());
    assert_eq!(r.config_value("branch.main.pushRemote").unwrap(), None);

    // Unsetting again → Ok(false).
    assert!(!r.unset_config("branch.main.pushRemote").unwrap());
}

/// `set_default_branch` → `clear_default_branch_cache` round trip,
/// covering the specialized default-branch writers that route through
/// `set_config_value` / `unset_config_value`.
#[test]
fn test_set_and_clear_default_branch() {
    let repo = TestRepo::new();
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    r.set_default_branch("main").unwrap();
    assert_eq!(r.default_branch(), Some("main".to_string()));

    // Clearing an existing cache returns true; a second clear returns false.
    let r2 = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert!(r2.clear_default_branch_cache().unwrap());
    assert!(!r2.clear_default_branch_cache().unwrap());
}

/// `switch_previous` / `set_switch_previous` round trip. Exercises
/// `worktrunk.history` read + write through the bulk-config helpers.
#[test]
fn test_switch_previous_roundtrip() {
    let repo = TestRepo::new();
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    // Populate the cache first to hit the in-memory update branch.
    let _ = r.is_bare();
    assert_eq!(r.switch_previous(), None);
    r.set_switch_previous(Some("feature-a")).unwrap();
    assert_eq!(r.switch_previous(), Some("feature-a".to_string()));
    // `None` is a no-op — doesn't clear.
    r.set_switch_previous(None).unwrap();
    assert_eq!(r.switch_previous(), Some("feature-a".to_string()));
}

/// `primary_remote_url` composes `primary_remote` + `remote_url`,
/// returning the raw URL for the primary remote. `primary_remote_parsed_url`
/// threads that through `GitRemoteUrl::parse`.
#[test]
fn test_primary_remote_url_composition() {
    let repo = TestRepo::new();
    repo.run_git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/max-sixty/worktrunk.git",
    ]);
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert_eq!(
        r.primary_remote_url(),
        Some("https://github.com/max-sixty/worktrunk.git".to_string())
    );
    let parsed = r.primary_remote_parsed_url().expect("parses");
    assert_eq!(parsed.owner(), "max-sixty");
    assert_eq!(parsed.repo(), "worktrunk");

    // Without a remote, both return None.
    let bare = TestRepo::new();
    let r2 = Repository::at(bare.root_path().to_path_buf()).unwrap();
    assert_eq!(r2.primary_remote_url(), None);
    assert!(r2.primary_remote_parsed_url().is_none());
}

/// `remote_url` for a configured remote round-trips; unknown remotes
/// return `None`. Covers the `.filter(|url| !url.is_empty())` branch
/// via the happy-path URL read.
#[test]
fn test_remote_url_known_and_unknown() {
    let repo = TestRepo::new();
    repo.run_git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:max-sixty/worktrunk.git",
    ]);
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert_eq!(
        r.remote_url("origin"),
        Some("git@github.com:max-sixty/worktrunk.git".to_string())
    );
    assert_eq!(r.remote_url("nonexistent"), None);
}

/// `primary_remote()` errors when no remotes are configured — covers
/// the `ok_or_else(|| anyhow!("No remotes configured"))` final arm.
#[test]
fn test_primary_remote_errors_with_no_remotes() {
    let repo = TestRepo::new(); // TestRepo::new() ships without a remote.
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let err = r.primary_remote().unwrap_err();
    assert!(
        err.to_string().contains("No remotes configured"),
        "unexpected error: {err}"
    );
}

/// `require_target_ref(None)` surfaces `StaleDefaultBranch` when the
/// persisted default branch no longer resolves locally. Covers the
/// `target.is_none()` arm added alongside `require_target_branch` for
/// commands like `wt step commit` that accept any commit-ish target.
#[test]
fn test_require_target_ref_surfaces_stale_default_branch() {
    use worktrunk::git::GitError;
    let repo = TestRepo::new();
    let r = Repository::at(repo.root_path().to_path_buf()).unwrap();
    r.set_config("worktrunk.default-branch", "nonexistent-branch")
        .unwrap();

    // Fresh Repository so the OnceCell re-reads the stale value.
    let r2 = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let err = r2.require_target_ref(None).unwrap_err();
    let gerr = err.downcast_ref::<GitError>().expect("GitError");
    assert!(
        matches!(gerr, GitError::StaleDefaultBranch { branch } if branch == "nonexistent-branch"),
        "expected StaleDefaultBranch, got {gerr:?}"
    );
}

/// `unset_config_value` propagates errors from corrupt git config
/// rather than returning `Ok(false)` (the exit-code-5 "key absent" case).
#[test]
fn test_unset_config_propagates_error_on_corrupt_config() {
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();
    let r = Repository::at(root.clone()).unwrap();
    r.set_default_branch("main").unwrap();

    // Corrupt the git config so subsequent writes fail with a real error
    // (not the benign exit-code-5 that maps to Ok(false)).
    fs::write(root.join(".git/config"), "[invalid section\n").unwrap();
    let err = r.unset_config("worktrunk.default-branch");
    assert!(
        err.is_err(),
        "unset_config should propagate corrupt-config errors: {err:?}"
    );
}

// =============================================================================
// Bug #1: Tag/branch name collision tests
// =============================================================================

/// When a tag and branch share the same name, git resolves unqualified refs to
/// the tag by default. This can cause is_ancestor() to return incorrect results
/// if the tag points to a different commit than the branch.
///
/// This test verifies that integration checking uses qualified refs (refs/heads/)
/// to avoid this ambiguity.
#[test]
fn test_tag_branch_name_collision_is_ancestor() {
    let repo = TestRepo::with_initial_commit();

    // Initial commit already exists from with_initial_commit()
    let main_sha = repo.git_output(&["rev-parse", "HEAD"]);

    // Create feature branch with additional commits
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo.root_path().join("feature.txt"), "feature content").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Feature commit"]);

    // Create a tag named "feature" pointing to the MAIN commit (earlier)
    // This simulates the scenario where someone creates a tag with the same name
    repo.run_git(&["tag", "feature", &main_sha]);

    // Now git has ambiguity: "feature" could be the tag (at main_sha) or the branch (ahead)
    // The branch "feature" is NOT an ancestor of main (it's ahead)
    // But the tag "feature" points to main_sha, which IS an ancestor of main (same commit)

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Without qualified refs, this would incorrectly return true
    // (checking the tag, which equals main, instead of the branch, which is ahead)
    // With the fix (using refs/heads/), this should correctly return false
    let result = repository.is_ancestor("feature", "main").unwrap();

    // The branch "feature" is ahead of main, so it should NOT be an ancestor
    assert!(
        !result,
        "is_ancestor should check the branch 'feature', not the tag 'feature'"
    );
}

/// Test that same_commit() correctly distinguishes between tag and branch
/// when they share the same name but point to different commits.
#[test]
fn test_tag_branch_name_collision_same_commit() {
    let repo = TestRepo::with_initial_commit();

    // Get main's SHA
    let main_sha = repo.git_output(&["rev-parse", "HEAD"]);

    // Create feature branch with additional commits
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo.root_path().join("feature.txt"), "feature content").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Feature commit"]);

    // Create a tag named "feature" pointing to main (different from branch)
    repo.run_git(&["tag", "feature", &main_sha]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // The branch "feature" is NOT at the same commit as main
    // But the tag "feature" IS at the same commit as main
    // Without qualified refs, this would incorrectly return true
    let result = repository.same_commit("feature", "main").unwrap();

    assert!(
        !result,
        "same_commit should check the branch 'feature', not the tag 'feature'"
    );
}

/// Test that trees_match() correctly distinguishes between tag and branch
/// when they share the same name but point to commits with different trees.
#[test]
fn test_tag_branch_name_collision_trees_match() {
    let repo = TestRepo::with_initial_commit();

    // Get main's SHA
    let main_sha = repo.git_output(&["rev-parse", "HEAD"]);

    // Create feature branch with different content
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo.root_path().join("feature.txt"), "feature content").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Feature commit"]);

    // Create a tag named "feature" pointing to main (different tree)
    repo.run_git(&["tag", "feature", &main_sha]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // The branch "feature" has different tree content than main
    // But the tag "feature" has the same tree as main
    // Without qualified refs, this would incorrectly return true
    let result = repository.trees_match("feature", "main").unwrap();

    assert!(
        !result,
        "trees_match should check the branch 'feature', not the tag 'feature'"
    );
}

/// Test that integration functions correctly handle HEAD (not a branch).
#[test]
fn test_integration_functions_handle_head() {
    let repo = TestRepo::new();

    // Create a commit so HEAD differs from an empty state
    fs::write(repo.root_path().join("file.txt"), "content").unwrap();
    repo.run_git(&["add", "file.txt"]);
    repo.run_git(&["commit", "-m", "Add file"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // HEAD should work in all integration functions
    // (resolve_preferring_branch should pass HEAD through unchanged)
    assert!(repository.same_commit("HEAD", "main").unwrap());
    assert!(repository.is_ancestor("main", "HEAD").unwrap());
    assert!(repository.trees_match("HEAD", "main").unwrap());
}

/// Test that integration functions correctly handle commit SHAs.
#[test]
fn test_integration_functions_handle_shas() {
    let repo = TestRepo::with_initial_commit();

    let main_sha = repo.git_output(&["rev-parse", "HEAD"]);

    // Create feature branch
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo.root_path().join("feature.txt"), "content").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Feature"]);

    let feature_sha = repo.git_output(&["rev-parse", "HEAD"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // SHAs should work in all integration functions
    // (resolve_preferring_branch should pass SHAs through unchanged)
    assert!(repository.same_commit(&main_sha, "main").unwrap());
    assert!(!repository.same_commit(&feature_sha, &main_sha).unwrap());
    assert!(repository.is_ancestor(&main_sha, &feature_sha).unwrap());
}

/// Test that integration functions correctly handle remote refs.
#[test]
fn test_integration_functions_handle_remote_refs() {
    let mut repo = TestRepo::with_initial_commit();
    repo.setup_remote("main");

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Remote refs like origin/main should work
    // (resolve_preferring_branch should pass them through unchanged since
    // refs/heads/origin/main doesn't exist)
    assert!(repository.same_commit("origin/main", "main").unwrap());
    assert!(repository.is_ancestor("origin/main", "main").unwrap());
}

// =============================================================================
// merge-tree exit code handling tests
// =============================================================================

/// has_merge_conflicts returns false for clean merges (exit 0)
/// and true for conflicts (exit 1).
#[test]
fn test_has_merge_conflicts_clean_vs_conflicting() {
    let repo = TestRepo::new();
    fs::write(repo.root_path().join("base.txt"), "base\n").unwrap();
    repo.run_git(&["add", "base.txt"]);
    repo.run_git(&["commit", "-m", "Base"]);

    // Clean merge: feature adds a new file (no overlap with main)
    repo.run_git(&["checkout", "-b", "clean-feature"]);
    fs::write(repo.root_path().join("new.txt"), "new\n").unwrap();
    repo.run_git(&["add", "new.txt"]);
    repo.run_git(&["commit", "-m", "Add new file"]);
    repo.run_git(&["checkout", "main"]);

    // Conflicting merge: feature edits the same file differently
    repo.run_git(&["checkout", "-b", "conflict-feature"]);
    fs::write(repo.root_path().join("base.txt"), "conflict\n").unwrap();
    repo.run_git(&["add", "base.txt"]);
    repo.run_git(&["commit", "-m", "Edit base"]);
    repo.run_git(&["checkout", "main"]);
    fs::write(repo.root_path().join("base.txt"), "main-edit\n").unwrap();
    repo.run_git(&["add", "base.txt"]);
    repo.run_git(&["commit", "-m", "Edit base on main"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    assert!(
        !repository
            .has_merge_conflicts("main", "clean-feature")
            .unwrap()
    );
    assert!(
        repository
            .has_merge_conflicts("main", "conflict-feature")
            .unwrap()
    );
}

/// has_merge_conflicts returns true (not Err) for orphan branches,
/// since unrelated histories can't be cleanly merged.
#[test]
fn test_has_merge_conflicts_orphan_branch() {
    let repo = TestRepo::with_initial_commit();

    repo.run_git(&["checkout", "--orphan", "orphan"]);
    repo.run_git(&["rm", "-rf", "."]);
    fs::write(repo.root_path().join("orphan.txt"), "orphan\n").unwrap();
    repo.run_git(&["add", "orphan.txt"]);
    repo.run_git(&["commit", "-m", "Orphan commit"]);
    repo.run_git(&["checkout", "main"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Orphan branches have no merge base — treated as conflicting, not as an error
    assert!(repository.has_merge_conflicts("main", "orphan").unwrap());
}

/// merge_integration_probe short-circuits for orphan branches:
/// would_merge_add=true, is_patch_id_match=false.
#[test]
fn test_merge_integration_probe_orphan_branch() {
    let repo = TestRepo::with_initial_commit();

    repo.run_git(&["checkout", "--orphan", "orphan"]);
    repo.run_git(&["rm", "-rf", "."]);
    fs::write(repo.root_path().join("orphan.txt"), "orphan\n").unwrap();
    repo.run_git(&["add", "orphan.txt"]);
    repo.run_git(&["commit", "-m", "Orphan commit"]);
    repo.run_git(&["checkout", "main"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let probe = repository
        .merge_integration_probe("orphan", "main")
        .unwrap();

    assert!(probe.would_merge_add, "orphan branch always has changes");
    assert!(
        !probe.is_patch_id_match,
        "no patch-id match possible without merge base"
    );
}

/// merge_integration_probe correctly detects already-integrated branches
/// (clean merge that doesn't change target tree).
#[test]
fn test_merge_integration_probe_already_integrated() {
    let repo = TestRepo::with_initial_commit();

    // Create feature, then merge it into main via fast-forward
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(repo.root_path().join("feature.txt"), "content\n").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Feature"]);
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["merge", "feature"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let probe = repository
        .merge_integration_probe("feature", "main")
        .unwrap();

    assert!(!probe.would_merge_add, "already-merged branch adds nothing");
}

// =============================================================================
// Bug: repo_path() inside git submodules
// =============================================================================

/// Test that `repo_path()` returns the correct working directory when run inside
/// a git submodule.
///
/// Previously, `repo_path()` derived the path from `git_common_dir.parent()`, which
/// fails for submodules where git data is stored in `parent/.git/modules/sub`.
/// The fix tries `git rev-parse --show-toplevel` first (works for submodules),
/// falling back to parent of git_common_dir for normal repos.
#[test]
fn test_repo_path_in_submodule() {
    // Create parent and submodule-origin repos
    let parent = TestRepo::new();
    fs::write(parent.path().join("README.md"), "# Parent").unwrap();
    parent.run_git(&["add", "."]);
    parent.run_git(&["commit", "-m", "Initial commit"]);

    let sub_origin = TestRepo::new();
    fs::write(sub_origin.path().join("README.md"), "# Submodule").unwrap();
    sub_origin.run_git(&["add", "."]);
    sub_origin.run_git(&["commit", "-m", "Submodule initial commit"]);

    // Add submodule to parent (using local path directly, with file transport allowed)
    parent
        .repo
        .run_command(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_origin.path().to_str().unwrap(),
            "sub",
        ])
        .unwrap();
    parent.run_git(&["commit", "-m", "Add submodule"]);

    // Now test: create Repository from inside the submodule
    let submodule_path = parent.path().join("sub");
    assert!(
        submodule_path.exists(),
        "Submodule path should exist: {:?}",
        submodule_path
    );

    let repository = Repository::at(submodule_path.clone()).unwrap();

    // The key assertion: repo_path() should return the submodule's working directory,
    // NOT something like parent/.git/modules/sub
    let repo_path = repository.repo_path().unwrap();

    // Canonicalize both paths for comparison (handles symlinks like /var -> /private/var on macOS)
    let expected = dunce::canonicalize(&submodule_path).unwrap();
    let actual = dunce::canonicalize(repo_path).unwrap();

    assert_eq!(
        actual, expected,
        "repo_path() should return submodule's working directory ({:?}), not git modules path",
        expected
    );

    // Also verify that git_common_dir is in the parent's .git/modules/ (confirming this is a real submodule)
    let git_common_dir = repository.git_common_dir();
    // Use components() to check path structure (works on both Unix and Windows)
    let components: Vec<_> = git_common_dir.components().collect();
    let has_git_modules = components.windows(2).any(|pair| {
        matches!(
            (pair[0].as_os_str().to_str(), pair[1].as_os_str().to_str()),
            (Some(".git"), Some("modules"))
        )
    });
    assert!(
        has_git_modules,
        "git_common_dir should be in parent's .git/modules/ for a submodule, got: {:?}",
        git_common_dir
    );

    // Verify list_worktrees() returns corrected paths for submodule main worktree.
    // Git's `worktree list` reports the main worktree as .git/modules/sub for submodules,
    // which is wrong — it should be the actual working directory.
    let worktrees = repository.list_worktrees().unwrap();
    assert!(
        !worktrees.is_empty(),
        "list_worktrees() should return at least the main worktree"
    );
    let main_wt_path = dunce::canonicalize(&worktrees[0].path).unwrap();
    assert_eq!(
        main_wt_path, expected,
        "list_worktrees()[0].path should be the submodule working directory, not .git/modules/sub"
    );

    // Verify worktree_for_branch() returns the corrected path (this is what `wt switch` uses)
    let main_branch = worktrees[0]
        .branch
        .as_deref()
        .expect("submodule main worktree should have a branch");
    let found_path = repository
        .worktree_for_branch(main_branch)
        .unwrap()
        .unwrap();
    let found_canonical = dunce::canonicalize(&found_path).unwrap();
    assert_eq!(
        found_canonical, expected,
        "worktree_for_branch() should return submodule working directory for default branch"
    );
}

// =============================================================================
// branch() error propagation tests (Bug fix: branch() swallows errors)
// =============================================================================

#[test]
fn test_branch_returns_none_for_detached_head() {
    let repo = TestRepo::with_initial_commit();
    let root = repo.root_path().to_path_buf();

    // Detach HEAD by checking out a specific commit
    let sha = repo.git_output(&["rev-parse", "HEAD"]);

    repo.run_git(&["checkout", "--detach", &sha]);

    // Create a fresh repository instance to avoid cached result
    let repository = Repository::at(&root).unwrap();
    // Use worktree_at with explicit path, not current_worktree() which uses base_path()
    let wt = repository.worktree_at(&root);

    let result = wt.branch();
    assert!(
        result.is_ok(),
        "branch() should succeed even for detached HEAD"
    );
    assert!(
        result.unwrap().is_none(),
        "branch() should return None for detached HEAD"
    );
}

#[test]
fn test_branch_returns_branch_for_unborn_repo() {
    let repo = TestRepo::empty();
    let root = repo.root_path().to_path_buf();
    let repository = Repository::at(&root).unwrap();
    let wt = repository.worktree_at(&root);

    let result = wt.branch();
    assert!(
        result.is_ok(),
        "branch() should succeed for unborn repo (no commits)"
    );
    assert_eq!(
        result.unwrap(),
        Some("main".to_string()),
        "branch() should return the default branch name even without commits"
    );
}

#[test]
fn test_branch_returns_branch_name() {
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();
    let repository = Repository::at(&root).unwrap();
    // Use worktree_at with explicit path, not current_worktree() which uses base_path()
    let wt = repository.worktree_at(&root);

    let result = wt.branch();
    assert!(result.is_ok(), "branch() should succeed");
    assert_eq!(
        result.unwrap(),
        Some("main".to_string()),
        "branch() should return the current branch name"
    );
}

#[test]
fn test_branch_caches_result() {
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();
    let repository = Repository::at(&root).unwrap();
    // Use worktree_at with explicit path, not current_worktree() which uses base_path()
    let wt = repository.worktree_at(&root);

    // First call
    let result1 = wt.branch().unwrap();
    // Second call should return cached result
    let result2 = wt.branch().unwrap();

    assert_eq!(result1, result2);
    assert_eq!(result1, Some("main".to_string()));
}

// =============================================================================
// is_dirty() behavior tests
// =============================================================================

#[test]
fn test_is_dirty_does_not_detect_skip_worktree_changes() {
    // This test documents a known limitation: is_dirty() uses `git status --porcelain`
    // which doesn't show files hidden via --skip-worktree or --assume-unchanged.
    //
    // We intentionally don't check for these because:
    // 1. Detecting them requires `git ls-files -v` which lists ALL tracked files
    // 2. On large repos (70k+ files), this adds noticeable latency to every clean check
    // 3. Users who use skip-worktree are power users who understand the implications
    let repo = TestRepo::new();
    let root = repo.root_path().to_path_buf();

    // Create and commit a file
    let file_path = root.join("local.env");
    fs::write(&file_path, "original").unwrap();
    repo.run_git(&["add", "local.env"]);
    repo.run_git(&["commit", "-m", "add local.env"]);

    // Mark with skip-worktree and modify
    repo.run_git(&["update-index", "--skip-worktree", "local.env"]);
    fs::write(&file_path, "modified but hidden").unwrap();

    let repository = Repository::at(&root).unwrap();
    let wt = repository.worktree_at(&root);

    // is_dirty() returns false — this is documented behavior, not a bug
    assert!(
        !wt.is_dirty().unwrap(),
        "is_dirty() does not detect skip-worktree changes by design"
    );
}

// =============================================================================
// sparse_checkout_paths() tests
// =============================================================================

#[test]
fn test_sparse_checkout_paths_empty_for_normal_repo() {
    let repo = TestRepo::new();
    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    let paths = repository.sparse_checkout_paths();
    assert!(
        paths.is_empty(),
        "normal repo should have no sparse checkout paths"
    );
}

#[test]
fn test_sparse_checkout_paths_returns_cone_paths() {
    let repo = TestRepo::new();

    // Create directories with files and commit them
    let dir1 = repo.root_path().join("dir1");
    let dir2 = repo.root_path().join("dir2");
    fs::create_dir_all(&dir1).unwrap();
    fs::create_dir_all(&dir2).unwrap();
    fs::write(dir1.join("file.txt"), "content1").unwrap();
    fs::write(dir2.join("file.txt"), "content2").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "add directories"]);

    // Set up sparse checkout in cone mode
    repo.run_git(&["sparse-checkout", "init", "--cone"]);
    repo.run_git(&["sparse-checkout", "set", "dir1", "dir2"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let paths = repository.sparse_checkout_paths();

    assert_eq!(paths, &["dir1".to_string(), "dir2".to_string()]);
}

#[test]
fn test_sparse_checkout_paths_cached() {
    let repo = TestRepo::new();

    let dir1 = repo.root_path().join("dir1");
    fs::create_dir_all(&dir1).unwrap();
    fs::write(dir1.join("file.txt"), "content").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "add dir1"]);

    repo.run_git(&["sparse-checkout", "init", "--cone"]);
    repo.run_git(&["sparse-checkout", "set", "dir1"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    let first = repository.sparse_checkout_paths();
    let second = repository.sparse_checkout_paths();

    assert_eq!(first, second);
    assert_eq!(first, &["dir1".to_string()]);
}

#[test]
fn test_branch_diff_stats_scoped_to_sparse_checkout() {
    let repo = TestRepo::new();

    // Create two directories with files on main
    let inside = repo.root_path().join("inside");
    let outside = repo.root_path().join("outside");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(inside.join("file.txt"), "base content\n").unwrap();
    fs::write(outside.join("file.txt"), "base content\n").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "add directories"]);

    // Create feature branch and modify files in both directories
    repo.run_git(&["checkout", "-b", "feature"]);
    fs::write(inside.join("file.txt"), "modified inside\nadded line\n").unwrap();
    fs::write(outside.join("file.txt"), "modified outside\nadded line\n").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "modify both dirs"]);

    // Go back to main and set up sparse checkout
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["sparse-checkout", "init", "--cone"]);
    repo.run_git(&["sparse-checkout", "set", "inside"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let stats = repository.branch_diff_stats("main", "feature").unwrap();

    // Only changes in inside/ should be counted
    // inside/file.txt: "base content\n" → "modified inside\nadded line\n" = 2 added, 1 deleted
    assert_eq!(stats.added, 2, "sparse: only inside/ additions");
    assert_eq!(stats.deleted, 1, "sparse: only inside/ deletions");

    // Disable sparse checkout — full stats include both inside/ and outside/
    repo.run_git(&["sparse-checkout", "disable"]);
    let full_repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let full_stats = full_repository
        .branch_diff_stats("main", "feature")
        .unwrap();

    // Both files have identical diffs, so full = 2x sparse
    assert_eq!(full_stats.added, 4, "full: inside/ + outside/ additions");
    assert_eq!(full_stats.deleted, 2, "full: inside/ + outside/ deletions");
}
