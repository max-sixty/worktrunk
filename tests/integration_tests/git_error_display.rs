use insta::assert_snapshot;
use std::path::PathBuf;
use worktrunk::git::{GitError, HookType, WorktrunkError, add_hook_skip_hint};

// ============================================================================
// Worktree errors
// ============================================================================

#[test]
fn display_worktree_removal_failed() {
    let err = GitError::WorktreeRemovalFailed {
        branch: "feature-x".into(),
        path: PathBuf::from("/tmp/repo.feature-x"),
        error: "fatal: worktree is dirty\nerror: could not remove worktree".into(),
    };

    assert_snapshot!("worktree_removal_failed", err.to_string());
}

#[test]
fn display_worktree_creation_failed() {
    let err = GitError::WorktreeCreationFailed {
        branch: "feature-y".into(),
        base_branch: Some("main".into()),
        error: "fatal: '/tmp/repo.feature-y' already exists".into(),
    };

    assert_snapshot!("worktree_creation_failed", err.to_string());
}

#[test]
fn display_worktree_missing() {
    let err = GitError::WorktreeMissing {
        branch: "stale-branch".into(),
    };

    assert_snapshot!("worktree_missing", err.to_string());
}

#[test]
fn display_no_worktree_found() {
    let err = GitError::NoWorktreeFound {
        branch: "nonexistent".into(),
    };

    assert_snapshot!("no_worktree_found", err.to_string());
}

#[test]
fn display_worktree_path_occupied() {
    let err = GitError::WorktreePathOccupied {
        branch: "feature-z".into(),
        path: PathBuf::from("/tmp/repo.feature-z"),
        occupant: Some("other-branch".into()),
    };

    assert_snapshot!("worktree_path_occupied", err.to_string());
}

#[test]
fn display_worktree_path_exists() {
    let err = GitError::WorktreePathExists {
        branch: "feature".to_string(),
        path: PathBuf::from("/tmp/repo.feature"),
        create: false,
    };

    assert_snapshot!("worktree_path_exists", err.to_string());
}

#[test]
fn display_cannot_remove_main_worktree() {
    let err = GitError::CannotRemoveMainWorktree;

    assert_snapshot!("cannot_remove_main_worktree", err.to_string());
}

// ============================================================================
// Git state errors
// ============================================================================

#[test]
fn display_detached_head() {
    let err = GitError::DetachedHead {
        action: Some("merge".into()),
    };

    assert_snapshot!("detached_head", err.to_string());
}

#[test]
fn display_detached_head_no_action() {
    let err = GitError::DetachedHead { action: None };

    assert_snapshot!("detached_head_no_action", err.to_string());
}

#[test]
fn display_uncommitted_changes() {
    let err = GitError::UncommittedChanges {
        action: Some("remove worktree".into()),
        branch: None,
        force_hint: false,
    };

    assert_snapshot!("uncommitted_changes", err.to_string());
}

#[test]
fn display_uncommitted_changes_with_branch() {
    let err = GitError::UncommittedChanges {
        action: Some("remove worktree".into()),
        branch: Some("feature-branch".into()),
        force_hint: false,
    };

    assert_snapshot!("uncommitted_changes_with_branch", err.to_string());
}

#[test]
fn display_uncommitted_changes_with_force_hint() {
    let err = GitError::UncommittedChanges {
        action: Some("remove worktree".into()),
        branch: Some("feature-branch".into()),
        force_hint: true,
    };

    assert_snapshot!("uncommitted_changes_with_force_hint", err.to_string());
}

#[test]
fn display_branch_already_exists() {
    let err = GitError::BranchAlreadyExists {
        branch: "feature".into(),
    };

    assert_snapshot!("branch_already_exists", err.to_string());
}

#[test]
fn display_invalid_reference() {
    let err = GitError::InvalidReference {
        reference: "nonexistent-branch".into(),
    };

    assert_snapshot!("invalid_reference", err.to_string());
}

// ============================================================================
// Merge/push errors
// ============================================================================

#[test]
fn display_push_failed() {
    let err = GitError::PushFailed {
        target_branch: "main".into(),
        error: "To /Users/user/workspace/repo/.git\n ! [remote rejected] HEAD -> main (Up-to-date check failed)\nerror: failed to push some refs to '/Users/user/workspace/repo/.git'".into(),
    };

    assert_snapshot!("push_failed", err.to_string());
}

#[test]
fn display_conflicting_changes() {
    let err = GitError::ConflictingChanges {
        target_branch: "main".into(),
        files: vec!["src/main.rs".into(), "src/lib.rs".into()],
        worktree_path: PathBuf::from("/tmp/repo.main"),
    };

    assert_snapshot!("conflicting_changes", err.to_string());
}

#[test]
fn display_not_fast_forward() {
    let err = GitError::NotFastForward {
        target_branch: "main".into(),
        commits_formatted: "abc1234 Fix bug\ndef5678 Add feature".into(),
        in_merge_context: false,
    };

    assert_snapshot!("not_fast_forward", err.to_string());
}

#[test]
fn display_not_fast_forward_merge_context() {
    let err = GitError::NotFastForward {
        target_branch: "main".into(),
        commits_formatted: "abc1234 New commit on main".into(),
        in_merge_context: true,
    };

    assert_snapshot!("not_fast_forward_merge_context", err.to_string());
}

#[test]
fn display_rebase_conflict() {
    let err = GitError::RebaseConflict {
        target_branch: "main".into(),
        git_output: "CONFLICT (content): Merge conflict in src/main.rs".into(),
    };

    assert_snapshot!("rebase_conflict", err.to_string());
}

// ============================================================================
// Validation/other errors
// ============================================================================

#[test]
fn display_not_interactive() {
    let err = GitError::NotInteractive;

    assert_snapshot!("not_interactive", err.to_string());
}

#[test]
fn display_llm_command_failed() {
    let err = GitError::LlmCommandFailed {
        command: "llm --model claude".into(),
        error: "Error: API key not found".into(),
        reproduction_command: None,
    };

    assert_snapshot!("llm_command_failed", err.to_string());
}

#[test]
fn display_llm_command_failed_with_reproduction() {
    let err = GitError::LlmCommandFailed {
        command: "llm --model claude".into(),
        error: "Error: API key not found".into(),
        reproduction_command: Some("wt step commit --show-prompt | llm --model claude".into()),
    };

    assert_snapshot!("llm_command_failed_with_reproduction", err.to_string());
}

#[test]
fn display_project_config_not_found() {
    let err = GitError::ProjectConfigNotFound {
        config_path: PathBuf::from("/tmp/repo/.config/wt.toml"),
    };

    assert_snapshot!("project_config_not_found", err.to_string());
}

#[test]
fn display_parse_error() {
    let err = GitError::ParseError {
        message: "Invalid branch name format".into(),
    };

    assert_snapshot!("parse_error", err.to_string());
}

#[test]
fn display_remote_only_branch() {
    let err = GitError::RemoteOnlyBranch {
        branch: "feature".into(),
        remote: "origin".into(),
    };

    assert_snapshot!("remote_only_branch", err.to_string());
}

#[test]
fn display_other() {
    let err = GitError::Other {
        message: "Unexpected git error".into(),
    };

    assert_snapshot!("other", err.to_string());
}

// ============================================================================
// WorktrunkError display tests
// ============================================================================

#[test]
fn display_hook_command_failed_with_name() {
    let err = WorktrunkError::HookCommandFailed {
        hook_type: HookType::PreMerge,
        command_name: Some("test".into()),
        error: "exit code 1".into(),
        exit_code: Some(1),
    };

    assert_snapshot!("hook_command_failed_with_name", err.to_string());
}

#[test]
fn display_hook_command_failed_without_name() {
    let err = WorktrunkError::HookCommandFailed {
        hook_type: HookType::PostCreate,
        command_name: None,
        error: "command not found".into(),
        exit_code: Some(127),
    };

    assert_snapshot!("hook_command_failed_without_name", err.to_string());
}

/// Shows the complete error with hint, as users would see it.
#[test]
fn display_hook_command_failed_with_skip_hint() {
    let err: anyhow::Error = WorktrunkError::HookCommandFailed {
        hook_type: HookType::PreMerge,
        command_name: Some("test".into()),
        error: "exit code 1".into(),
        exit_code: Some(1),
    }
    .into();

    // Wrap with hint (as done by commands supporting --no-verify)
    let err_with_hint = add_hook_skip_hint(err);

    assert_snapshot!(
        "hook_command_failed_with_skip_hint",
        err_with_hint.to_string()
    );
}

// ============================================================================
// Multiline error formatting (tests the pattern used in main.rs catchall)
// ============================================================================

/// Test that multiline errors without context are formatted with header + gutter.
/// This is the pattern used in main.rs for untyped anyhow errors.
#[test]
fn multiline_error_formatting() {
    use worktrunk::styling::{error_message, format_with_gutter};

    // Simulate what main.rs does for multiline errors without context:
    // 1. Show "Command failed" header
    // 2. Show the error content in a gutter

    let multiline_error =
        "fatal: Unable to read current working directory\nerror: Could not determine cwd";

    let header = error_message("Command failed").to_string();
    let gutter = format_with_gutter(multiline_error, None);

    // Verify header has error symbol
    assert!(
        header.contains("Command failed"),
        "Header should contain 'Command failed'"
    );

    // Verify gutter contains both lines
    assert!(
        gutter.contains("fatal: Unable to read"),
        "Gutter should contain first line"
    );
    assert!(
        gutter.contains("Could not determine cwd"),
        "Gutter should contain second line"
    );

    // Snapshot the combined output
    assert_snapshot!(
        "multiline_error_formatting",
        format!("{}\n{}", header, gutter)
    );
}

/// Test that CRLF and CR line endings are normalized before formatting.
/// main.rs normalizes: msg.replace("\r\n", "\n").replace('\r', "\n")
#[test]
fn multiline_error_crlf_normalization() {
    use worktrunk::styling::format_with_gutter;

    // Test CRLF (Windows line endings)
    let crlf_error = "line1\r\nline2\r\nline3";
    let normalized = crlf_error.replace("\r\n", "\n").replace('\r', "\n");
    let gutter = format_with_gutter(&normalized, None);

    // All three lines should appear
    assert!(gutter.contains("line1"), "Should contain line1");
    assert!(gutter.contains("line2"), "Should contain line2");
    assert!(gutter.contains("line3"), "Should contain line3");

    // Test CR only (old Mac line endings)
    let cr_error = "line1\rline2\rline3";
    let normalized = cr_error.replace("\r\n", "\n").replace('\r', "\n");
    let gutter = format_with_gutter(&normalized, None);

    assert!(gutter.contains("line1"), "CR: Should contain line1");
    assert!(gutter.contains("line2"), "CR: Should contain line2");
    assert!(gutter.contains("line3"), "CR: Should contain line3");
}

// ============================================================================
// Integration test: verify error message includes command when git unavailable
// ============================================================================

/// This is an integration test because it requires running the actual binary.
#[test]
#[cfg(unix)]
fn git_unavailable_error_includes_command() {
    use insta_cmd::get_cargo_bin;
    use std::process::Command;

    let mut cmd = Command::new(get_cargo_bin("wt"));
    cmd.arg("list")
        // Set PATH to empty so git isn't found
        .env("PATH", "/nonexistent")
        // Prevent any fallback mechanisms
        .env_remove("GIT_EXEC_PATH");

    let output = cmd.output().expect("Failed to run wt");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The error should include the git command that failed
    assert!(
        stderr.contains("Failed to execute: git"),
        "Error should include 'Failed to execute: git', got: {}",
        stderr
    );
}
