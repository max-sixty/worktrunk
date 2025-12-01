use insta::assert_snapshot;
use std::path::PathBuf;
use worktrunk::git::GitError;

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
        path: PathBuf::from("/tmp/repo.feature"),
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
    };

    assert_snapshot!("uncommitted_changes", err.to_string());
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
        error: "To /Users/user/workspace/repo/.git\n ! [remote rejected] HEAD -> main (Up-to-date check failed)\nerror: failed to push some refs to '/Users/user/workspace/repo/.git'".into(),
    };

    assert_snapshot!("push_failed", err.to_string());
}

#[test]
fn display_conflicting_changes() {
    let err = GitError::ConflictingChanges {
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
fn display_merge_commits_found() {
    let err = GitError::MergeCommitsFound;

    assert_snapshot!("merge_commits_found", err.to_string());
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
    };

    assert_snapshot!("llm_command_failed", err.to_string());
}

#[test]
fn display_project_config_not_found() {
    let err = GitError::ProjectConfigNotFound {
        config_path: PathBuf::from("/tmp/repo/.config/wt.toml"),
    };

    assert_snapshot!("project_config_not_found", err.to_string());
}

#[test]
fn display_worktree_path_mismatch() {
    let err = GitError::WorktreePathMismatch {
        branch: "feature".into(),
        expected_path: PathBuf::from("/tmp/repo.feature"),
        actual_path: PathBuf::from("/tmp/repo.other"),
    };

    assert_snapshot!("worktree_path_mismatch", err.to_string());
}

#[test]
fn display_parse_error() {
    let err = GitError::ParseError {
        message: "Invalid branch name format".into(),
    };

    assert_snapshot!("parse_error", err.to_string());
}

#[test]
fn display_other() {
    let err = GitError::Other {
        message: "Unexpected git error".into(),
    };

    assert_snapshot!("other", err.to_string());
}
