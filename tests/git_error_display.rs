use insta::assert_snapshot;
use worktrunk::git::GitError;

#[test]
fn display_worktree_removal_failed() {
    let err = GitError::WorktreeRemovalFailed {
        branch: "feature-x".to_string(),
        path: std::path::PathBuf::from("/tmp/repo.feature-x"),
        error: "fatal: worktree is dirty\nerror: could not remove worktree".to_string(),
    };

    assert_snapshot!("worktree_removal_failed", err.to_string());
}

#[test]
fn display_push_failed() {
    let err = GitError::PushFailed {
        error: "To /Users/user/workspace/repo/.git\n ! [remote rejected] HEAD -> main (Up-to-date check failed)\nerror: failed to push some refs to '/Users/user/workspace/repo/.git'"
            .to_string(),
    };

    assert_snapshot!("push_failed", err.to_string());
}
