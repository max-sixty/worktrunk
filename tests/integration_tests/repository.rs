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
    // Should be the full canonical path (security: avoids collisions across unrelated repos)
    let expected = dunce::canonicalize(repo.root_path()).unwrap();
    assert_eq!(id, expected.to_str().unwrap());
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
    let repo = TestRepo::new();

    // Create initial commit on main (already exists from TestRepo::new())
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
    let repo = TestRepo::new();

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
    let repo = TestRepo::new();

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
    let repo = TestRepo::new();

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
    let mut repo = TestRepo::new();
    repo.setup_remote("main");

    let repository = Repository::at(repo.root_path().to_path_buf()).unwrap();

    // Remote refs like origin/main should work
    // (resolve_preferring_branch should pass them through unchanged since
    // refs/heads/origin/main doesn't exist)
    assert!(repository.same_commit("origin/main", "main").unwrap());
    assert!(repository.is_ancestor("origin/main", "main").unwrap());
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
    use tempfile::TempDir;

    // Create parent repository
    let parent_temp = TempDir::new().unwrap();
    let parent_path = parent_temp.path().join("parent");
    fs::create_dir(&parent_path).unwrap();

    // Initialize parent repo with git config
    let mut cmd = std::process::Command::new("git");
    cmd.args(["init", "-q"])
        .current_dir(&parent_path)
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let output = cmd.output().unwrap();
    assert!(output.status.success(), "git init failed for parent");

    // Configure git user for commits
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&parent_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&parent_path)
        .output()
        .unwrap();

    // Create initial commit in parent
    fs::write(parent_path.join("README.md"), "# Parent").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&parent_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&parent_path)
        .output()
        .unwrap();

    // Create submodule repository (as a separate repo first)
    let sub_temp = TempDir::new().unwrap();
    let sub_origin_path = sub_temp.path().join("submodule-origin");
    fs::create_dir(&sub_origin_path).unwrap();

    let mut cmd = std::process::Command::new("git");
    cmd.args(["init", "-q"])
        .current_dir(&sub_origin_path)
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "git init failed for submodule origin"
    );

    // Configure git user for submodule
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&sub_origin_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&sub_origin_path)
        .output()
        .unwrap();

    // Create initial commit in submodule origin
    fs::write(sub_origin_path.join("README.md"), "# Submodule").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&sub_origin_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "Submodule initial commit"])
        .current_dir(&sub_origin_path)
        .output()
        .unwrap();

    // Add submodule to parent (using local path directly, with file transport allowed)
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_origin_path.to_str().unwrap(),
            "sub",
        ])
        .current_dir(&parent_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git submodule add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Commit the submodule addition
    std::process::Command::new("git")
        .args(["commit", "-m", "Add submodule"])
        .current_dir(&parent_path)
        .output()
        .unwrap();

    // Now test: create Repository from inside the submodule
    let submodule_path = parent_path.join("sub");
    assert!(
        submodule_path.exists(),
        "Submodule path should exist: {:?}",
        submodule_path
    );

    let repository = Repository::at(submodule_path.clone()).unwrap();

    // The key assertion: repo_path() should return the submodule's working directory,
    // NOT something like parent/.git/modules/sub
    let repo_path = repository.repo_path();

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
}

// =============================================================================
// branch() error propagation tests (Bug fix: branch() swallows errors)
// =============================================================================

#[test]
fn test_branch_returns_none_for_detached_head() {
    let repo = TestRepo::new();
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
