//! Tests for git repository methods to improve code coverage.

use std::fs;

use worktrunk::git::Repository;

use crate::common::TestRepo;

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
    let repo = TestRepo::new();
    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .output()
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
    let repo = TestRepo::new();
    // Create some branches
    repo.git_command()
        .args(["branch", "alpha"])
        .output()
        .unwrap();
    repo.git_command()
        .args(["branch", "beta"])
        .output()
        .unwrap();

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
    let mut repo = TestRepo::new();
    repo.setup_remote("main");
    // Override the remote URL to https format
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "https://github.com/user/repo.git",
        ])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_http() {
    let mut repo = TestRepo::new();
    repo.setup_remote("main");
    // Override the remote URL to http format (no SSL)
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "http://gitlab.example.com/team/project.git",
        ])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "gitlab.example.com/team/project");
}

#[test]
fn test_project_identifier_ssh_colon() {
    let mut repo = TestRepo::new();
    repo.setup_remote("main");
    // Override the remote URL to SSH format with colon
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "git@github.com:user/repo.git",
        ])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_ssh_protocol() {
    let mut repo = TestRepo::new();
    repo.setup_remote("main");
    // Override the remote URL to ssh:// format
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "ssh://git@github.com/user/repo.git",
        ])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // ssh://git@github.com/user/repo.git -> github.com/user/repo
    assert_eq!(id, "github.com/user/repo");
}

#[test]
fn test_project_identifier_ssh_protocol_with_port() {
    let mut repo = TestRepo::new();
    repo.setup_remote("main");
    // Override the remote URL to ssh:// format with port
    repo.git_command()
        .args([
            "remote",
            "set-url",
            "origin",
            "ssh://git@gitlab.example.com:2222/team/project.git",
        ])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // The port colon gets converted to /
    assert_eq!(id, "gitlab.example.com/2222/team/project");
}

#[test]
fn test_project_identifier_no_remote_fallback() {
    let repo = TestRepo::new();
    // Remove origin (fixture has it) for this no-remote test
    repo.run_git(&["remote", "remove", "origin"]);

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let id = repository.project_identifier().unwrap();
    // Should be the repo directory name
    assert_eq!(id, "repo");
}

// =============================================================================
// get_config/set_config tests
// =============================================================================

#[test]
fn test_get_config_exists() {
    let repo = TestRepo::new();
    repo.git_command()
        .args(["config", "test.key", "test-value"])
        .output()
        .unwrap();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let value = repository.get_config("test.key").unwrap();
    assert_eq!(value, Some("test-value".to_string()));
}

#[test]
fn test_get_config_not_exists() {
    let repo = TestRepo::new();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    let value = repository.get_config("nonexistent.key").unwrap();
    assert!(value.is_none());
}

#[test]
fn test_set_config() {
    let repo = TestRepo::new();

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();
    repository.set_config("test.setting", "new-value").unwrap();

    // Verify it was set
    let value = repository.get_config("test.setting").unwrap();
    assert_eq!(value, Some("new-value".to_string()));
}
