use insta::assert_snapshot;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use worktrunk::git::{
    Diagnostic, FailedCommand, GitError, HookErrorWithHint, HookType, InProgressOperation, RefType,
    StaleWorktreeWork, WorktrunkError, add_hook_skip_hint,
};

use crate::common::{mock_commands::MockConfig, test_tempdir, wt_command};

fn wt_with_git_version(version: &str) -> (TempDir, Command) {
    let mock_bin = test_tempdir();
    MockConfig::new("git")
        .version(version)
        .write(mock_bin.path());

    let mut paths = vec![mock_bin.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));

    let mut cmd = wt_command();
    cmd.env("PATH", std::env::join_paths(paths).unwrap())
        .env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", mock_bin.path());
    (mock_bin, cmd)
}

fn render_cases(cases: impl IntoIterator<Item = (&'static str, String)>) -> String {
    cases
        .into_iter()
        .map(|(name, output)| format!("## {name}\n\n{output}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn worktree_errors_render() {
    let cases = [
        (
            "removal failed",
            GitError::WorktreeRemovalFailed {
                branch: "feature-x".into(),
                path: PathBuf::from("/tmp/repo.feature-x"),
                error: anyhow::anyhow!(
                    "fatal: worktree is dirty\nerror: could not remove worktree"
                ),
                remaining_entries: None,
            }
            .render(),
        ),
        (
            "removal left directories",
            GitError::WorktreeRemovalFailed {
                branch: "feature-x".into(),
                path: PathBuf::from("/tmp/repo.feature-x"),
                error: anyhow::anyhow!(
                    "error: failed to delete '/tmp/repo.feature-x': Directory not empty"
                ),
                remaining_entries: Some(vec![
                    ".vite/".into(),
                    "node_modules/".into(),
                    "target/".into(),
                ]),
            }
            .render(),
        ),
        (
            "removal left one entry",
            GitError::WorktreeRemovalFailed {
                branch: "feature-x".into(),
                path: PathBuf::from("/tmp/repo.feature-x"),
                error: anyhow::anyhow!(
                    "error: failed to remove '/tmp/repo.feature-x/target': Permission denied"
                ),
                remaining_entries: Some(vec!["target/".into()]),
            }
            .render(),
        ),
        (
            "removal truncates a long entry list",
            GitError::WorktreeRemovalFailed {
                branch: "feature-x".into(),
                path: PathBuf::from("/tmp/repo.feature-x"),
                error: anyhow::anyhow!(
                    "error: failed to delete '/tmp/repo.feature-x': Directory not empty"
                ),
                remaining_entries: Some((0..15).map(|i| format!("dir-{i:02}/")).collect()),
            }
            .render(),
        ),
        (
            "creation failed",
            GitError::WorktreeCreationFailed {
                branch: "feature-y".into(),
                base_branch: Some("main".into()),
                error: "fatal: '/tmp/repo.feature-y' already exists".into(),
                command: None,
                leftover_branch: false,
            }
            .render(),
        ),
        (
            "creation failed with command",
            GitError::WorktreeCreationFailed {
                branch: "fix".into(),
                base_branch: Some("main".into()),
                error:
                    "Preparing worktree (new branch 'fix')\nfatal: cannot lock ref 'refs/heads/fix'"
                        .into(),
                command: Some(FailedCommand {
                    command: "git worktree add /tmp/repo.fix -b fix main".into(),
                    exit_info: "exit code 128".into(),
                }),
                leftover_branch: false,
            }
            .render(),
        ),
        (
            "worktree missing",
            GitError::WorktreeMissing {
                branch: "stale-branch".into(),
                repairable_at: None,
            }
            .render(),
        ),
        (
            "worktree missing its .git, directory remaining at a path needing quotes",
            GitError::WorktreeMissing {
                branch: "stale-branch".into(),
                repairable_at: Some(PathBuf::from("/tmp/my repo.stale-branch")),
            }
            .render(),
        ),
        (
            "stale worktree holding staged changes",
            GitError::StaleWorktreeHoldsWork {
                branch: "stale-branch".into(),
                path: PathBuf::from("/tmp/repo.stale-branch"),
                directory_remains: true,
                work: StaleWorktreeWork::StagedChanges,
            }
            .render(),
        ),
        (
            "stale worktree mid-operation, directory gone, at a path needing quotes",
            [
                InProgressOperation::Merge,
                InProgressOperation::Rebase,
                InProgressOperation::CherryPick,
                InProgressOperation::Revert,
                InProgressOperation::Bisect,
            ]
            .map(|operation| {
                GitError::StaleWorktreeHoldsWork {
                    branch: "stale-branch".into(),
                    path: PathBuf::from("/tmp/my repo.stale-branch"),
                    directory_remains: false,
                    work: StaleWorktreeWork::Operation(operation),
                }
                .render()
            })
            .join("\n"),
        ),
        (
            "no worktree at a leftover directory",
            GitError::WorktreeNotFoundAtPath {
                path: PathBuf::from("/tmp/repo/.claude/worktrees/ghost"),
            }
            .render(),
        ),
        (
            "branch not found",
            GitError::BranchNotFound {
                branch: "nonexistent".into(),
                show_create_hint: true,
                last_fetch_ago: None,
                pr_mr_platform: None,
            }
            .render(),
        ),
        (
            "branch not found after stale fetch",
            GitError::BranchNotFound {
                branch: "nonexistent".into(),
                show_create_hint: true,
                last_fetch_ago: Some("last fetched 3h ago".into()),
                pr_mr_platform: None,
            }
            .render(),
        ),
        (
            "branch not found without create hint",
            GitError::BranchNotFound {
                branch: "nonexistent".into(),
                show_create_hint: false,
                last_fetch_ago: None,
                pr_mr_platform: None,
            }
            .render(),
        ),
        (
            "numeric branch on unknown forge",
            GitError::BranchNotFound {
                branch: "2474".into(),
                show_create_hint: true,
                last_fetch_ago: None,
                pr_mr_platform: None,
            }
            .render(),
        ),
        (
            "numeric branch on GitHub",
            GitError::BranchNotFound {
                branch: "2474".into(),
                show_create_hint: true,
                last_fetch_ago: Some("last fetched 9h ago".into()),
                pr_mr_platform: Some(RefType::Pr),
            }
            .render(),
        ),
        (
            "numeric branch on GitLab",
            GitError::BranchNotFound {
                branch: "2474".into(),
                show_create_hint: true,
                last_fetch_ago: None,
                pr_mr_platform: Some(RefType::Mr),
            }
            .render(),
        ),
        (
            "worktree path occupied",
            GitError::WorktreePathOccupied {
                branch: "feature-z".into(),
                path: PathBuf::from("/tmp/repo.feature-z"),
                occupant: Some("other-branch".into()),
            }
            .render(),
        ),
        (
            "worktree path exists",
            GitError::WorktreePathExists {
                branch: "feature".into(),
                path: PathBuf::from("/tmp/repo.feature"),
                create: false,
                from_path_flag: false,
            }
            .render(),
        ),
        (
            "cannot remove main worktree",
            GitError::CannotRemoveMainWorktree.render(),
        ),
    ];

    assert_snapshot!("worktree_errors", render_cases(cases));
}

#[test]
fn git_state_errors_render() {
    let cases = [
        (
            "detached HEAD while merging",
            GitError::DetachedHead {
                action: Some("merge".into()),
                worktree: None,
            }
            .render(),
        ),
        (
            "detached HEAD without action",
            GitError::DetachedHead {
                action: None,
                worktree: None,
            }
            .render(),
        ),
        (
            // The detached worktree is the target the user named, not the one
            // they are standing in, so the hint has to say where to switch.
            "detached HEAD in a named target worktree",
            GitError::DetachedHead {
                action: Some("use /tmp/repo.other as a target".into()),
                worktree: Some(PathBuf::from("/tmp/repo.other")),
            }
            .render(),
        ),
        (
            "rebase in progress",
            GitError::OperationInProgress {
                action: "rebase".into(),
                branch: None,
            }
            .render(),
        ),
        (
            "merge in progress",
            GitError::OperationInProgress {
                action: "merge".into(),
                branch: None,
            }
            .render(),
        ),
        (
            "operation in progress in the target worktree",
            GitError::OperationInProgress {
                action: "push".into(),
                branch: Some("main".into()),
            }
            .render(),
        ),
        (
            "uncommitted changes",
            GitError::UncommittedChanges {
                action: Some("remove worktree".into()),
                branch: None,
                force_hint: false,
                dirty_files: Vec::new(),
            }
            .render(),
        ),
        (
            "uncommitted changes on named branch",
            GitError::UncommittedChanges {
                action: Some("remove worktree".into()),
                branch: Some("feature-branch".into()),
                force_hint: false,
                dirty_files: Vec::new(),
            }
            .render(),
        ),
        (
            "uncommitted changes with force hint",
            GitError::UncommittedChanges {
                action: Some("remove worktree".into()),
                branch: Some("feature-branch".into()),
                force_hint: true,
                dirty_files: Vec::new(),
            }
            .render(),
        ),
        (
            "uncommitted changes with dirty files",
            GitError::UncommittedChanges {
                action: Some("remove worktree after merge".into()),
                branch: Some("feature-auth".into()),
                force_hint: false,
                dirty_files: vec![" M auth.rs".into(), "?? .DS_Store".into()],
            }
            .render(),
        ),
        (
            "branch already exists",
            GitError::BranchAlreadyExists {
                branch: "feature".into(),
            }
            .render(),
        ),
    ];

    assert_snapshot!("git_state_errors", render_cases(cases));
}

#[test]
fn integration_errors_render() {
    let cases = [
        (
            "push failed",
            GitError::PushFailed {
                target_branch: "main".into(),
                error: "To /Users/user/workspace/repo/.git\n ! [remote rejected] HEAD -> main (Up-to-date check failed)\nerror: failed to push some refs to '/Users/user/workspace/repo/.git'".into(),
            }
            .render(),
        ),
        (
            "conflicting changes",
            GitError::ConflictingChanges {
                target_branch: "main".into(),
                files: vec!["src/main.rs".into(), "src/lib.rs".into()],
                worktree_path: PathBuf::from("/tmp/repo.main"),
            }
            .render(),
        ),
        (
            "not fast-forward",
            GitError::NotFastForward {
                target_branch: "main".into(),
                commits_formatted: "abc1234 Fix bug\ndef5678 Add feature".into(),
                in_merge_context: false,
            }
            .render(),
        ),
        (
            "not fast-forward during merge",
            GitError::NotFastForward {
                target_branch: "main".into(),
                commits_formatted: "abc1234 New commit on main".into(),
                in_merge_context: true,
            }
            .render(),
        ),
        (
            "rebase conflict",
            GitError::RebaseConflict {
                target_branch: "main".into(),
                git_output: "CONFLICT (content): Merge conflict in src/main.rs".into(),
            }
            .render(),
        ),
    ];

    assert_snapshot!("integration_errors", render_cases(cases));
}

#[test]
fn command_errors_render() {
    let hook_with_name = WorktrunkError::HookCommandFailed {
        hook_type: HookType::PreMerge,
        source: worktrunk::config::HookSource::User,
        template: Some("echo test".into()),
        command_name: Some("test".into()),
        error: WorktrunkError::ChildProcessExited {
            cancellation: None,
            code: 1,
            physical_signal: None,
        }
        .into(),
    };
    let hook_without_name = WorktrunkError::HookCommandFailed {
        hook_type: HookType::PreCreate,
        source: worktrunk::config::HookSource::User,
        template: Some("echo test".into()),
        command_name: None,
        error: WorktrunkError::ChildProcessExited {
            cancellation: None,
            code: 127,
            physical_signal: None,
        }
        .into(),
    };
    let hook_with_hint = add_hook_skip_hint(
        WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            source: worktrunk::config::HookSource::User,
            template: Some("echo test".into()),
            command_name: Some("test".into()),
            error: WorktrunkError::ChildProcessExited {
                cancellation: None,
                code: 1,
                physical_signal: None,
            }
            .into(),
        }
        .into(),
    );

    let cases = [
        ("non-interactive", GitError::NotInteractive.render()),
        (
            "LLM command failed",
            GitError::LlmCommandFailed {
                command: "llm --model claude".into(),
                error: "Error: API key not found".into(),
                reproduction_command: None,
            }
            .render(),
        ),
        (
            "LLM command failed with reproduction",
            GitError::LlmCommandFailed {
                command: "llm --model claude".into(),
                error: "Error: API key not found".into(),
                reproduction_command: Some(
                    "wt step commit --show-prompt | llm --model claude".into(),
                ),
            }
            .render(),
        ),
        (
            "project config missing",
            GitError::ProjectConfigNotFound {
                config_path: PathBuf::from("/tmp/repo/.config/wt.toml"),
            }
            .render(),
        ),
        (
            "parse error",
            GitError::ParseError {
                message: "Invalid branch name format".into(),
            }
            .render(),
        ),
        (
            "remote-only branch",
            GitError::RemoteOnlyBranch {
                branch: "feature".into(),
                remote: "origin".into(),
            }
            .render(),
        ),
        (
            "other git error",
            GitError::Other {
                message: "Unexpected git error".into(),
            }
            .render(),
        ),
        ("named hook failed", hook_with_name.render()),
        ("unnamed hook failed", hook_without_name.render()),
        (
            "hook failed with skip hint",
            hook_with_hint
                .downcast_ref::<HookErrorWithHint>()
                .expect("wrapped to HookErrorWithHint")
                .render(),
        ),
    ];

    assert_snapshot!("command_errors", render_cases(cases));
}

#[test]
fn multiline_error_helpers_normalize_line_endings() {
    use worktrunk::styling::{error_message, format_with_gutter};

    let message = "fatal: Unable to read current working directory\nerror: Could not determine cwd";
    let rendered = format!(
        "{}\n{}",
        error_message("Command failed"),
        format_with_gutter(message, None)
    );
    assert_snapshot!("multiline_error_formatting", rendered);

    let normalize = |message: &str| message.replace("\r\n", "\n").replace('\r', "\n");
    let expected = format_with_gutter("line1\nline2\nline3", None);
    assert_eq!(
        format_with_gutter(&normalize("line1\r\nline2\r\nline3"), None),
        expected
    );
    assert_eq!(
        format_with_gutter(&normalize("line1\rline2\rline3"), None),
        expected
    );
}

#[test]
#[cfg(unix)]
fn git_unavailable_error_includes_command() {
    let mut cmd = wt_command();
    cmd.arg("list")
        .env("PATH", "/nonexistent")
        .env_remove("GIT_EXEC_PATH");

    let output = cmd.output().expect("run wt without git");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Failed to run git --version"),
        "stderr was:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_git_older_than_the_supported_minimum() {
    let (_mock_bin, mut cmd) = wt_with_git_version("git version 2.42.4");
    cmd.arg("list");
    let output = cmd.output().unwrap();

    assert_snapshot!(String::from_utf8_lossy(&output.stderr), @"[31m✗[39m [31mGit 2.42.4 is unsupported; Worktrunk requires Git 2.43.0 or newer[39m");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn shell_init_remains_available_on_older_git() {
    let (_mock_bin, mut cmd) = wt_with_git_version("git version 2.42.4");
    cmd.args(["config", "shell", "init", "nu"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "shell init failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("export def \"nu-complete wt\""),
        "shell init must emit the Nushell integration:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Errors before dispatch leave stdout empty and explain the failure on stderr.
#[test]
fn json_failure_before_dispatch() {
    let (_mock_bin, mut cmd) = wt_with_git_version("git version 2.42.4");
    let output = cmd.args(["list", "--format=json"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("2.42.4"));
}

/// A dispatched command with no result leaves stdout empty on failure.
#[test]
fn json_failure_without_repository() {
    let output = wt_command()
        .args(["list", "--format=json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

/// A display-only Git failure must not hide the commit already created.
#[cfg(unix)]
#[rstest::rstest]
#[case::commit("commit", 0)]
#[case::squash_staged_changes("squash", 0)]
#[case::squash_commits("squash", 2)]
fn json_outcome_retains_commit_after_display_failure(
    #[case] command: &str,
    #[case] commits_ahead: usize,
) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let repo = crate::common::TestRepo::standard();
    repo.run_git(&["checkout", "-b", "late-reporting"]);
    for index in 0..commits_ahead {
        repo.commit_in_worktree(
            repo.root_path(),
            "new.txt",
            &format!("content {index}"),
            &format!("feat: change {index}"),
        );
    }
    assert_eq!(
        repo.git_output(&["rev-list", "--count", "main..HEAD"]),
        commits_ahead.to_string(),
    );
    let previous_head = repo.git_output(&["rev-parse", "HEAD"]);
    if commits_ahead == 0 {
        fs::write(repo.root_path().join("new.txt"), "new content").unwrap();
    }

    // Keep the real commit and full-OID lookup; fail only abbreviation for
    // the success message, after the command has a complete result to retain.
    let wrapper = test_tempdir();
    let real_git = which::which("git").unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1 $2" = 'rev-parse --short' ]; then
  echo 'display abbreviation failed' >&2
  exit 42
fi
exec {} "$@"
"#,
        shell_escape::unix::escape(real_git.to_string_lossy()),
    );
    let wrapper_git = wrapper.path().join("git");
    fs::write(&wrapper_git, script).unwrap();
    fs::set_permissions(&wrapper_git, fs::Permissions::from_mode(0o755)).unwrap();

    let mut paths = vec![wrapper.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = repo
        .wt_command()
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env(
            "WORKTRUNK_COMMIT__GENERATION__COMMAND",
            "cat >/dev/null && echo 'feat: retained commit'",
        )
        .args(["step", command, "--format=json", "--no-hooks"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let head = repo.git_output(&["rev-parse", "HEAD"]);
    assert_ne!(head, previous_head, "the commit must have completed");
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["commit"], head, "{response}");
    assert_eq!(response["message"], "feat: retained commit", "{response}");
    assert_eq!(response["stage_mode"], "all", "{response}");
    assert_eq!(repo.git_output(&["rev-list", "--count", "main..HEAD"]), "1");
    if command == "squash" {
        assert_eq!(response["outcome"], "squashed", "{response}");
    }
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("display abbreviation failed"),
        "{response}"
    );
}

/// Post-commit template validation can fail after Git has created the commit.
#[rstest::rstest]
#[case::commit("commit")]
#[case::merge("merge")]
fn json_retains_committed_result_after_invalid_post_commit_hook(#[case] command: &str) {
    let repo = crate::common::TestRepo::standard();
    repo.run_git(&["checkout", "-b", "late-reporting"]);
    let previous_head = repo.git_output(&["rev-parse", "HEAD"]);
    std::fs::write(repo.root_path().join("new.txt"), "new content").unwrap();
    repo.write_test_config("[post-commit]\ninvalid = 'echo {{ unclosed'\n");

    let mut cmd = repo.wt_command();
    if command == "commit" {
        cmd.arg("step");
    }
    cmd.args([command, "--format=json"]);
    if command == "merge" {
        cmd.arg("--no-squash");
    }
    let output = cmd.output().unwrap();

    assert!(!output.status.success(), "{output:?}");
    let head = repo.git_output(&["rev-parse", "HEAD"]);
    assert_ne!(head, previous_head, "the commit must have completed");
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    if command == "commit" {
        assert_eq!(response["commit"], head, "{response}");
    } else {
        assert_eq!(response["committed"], true, "{response}");
        assert_eq!(response["merged"], false, "{response}");
        assert_eq!(repo.git_output(&["rev-parse", "main"]), previous_head);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("syntax error"), "{stderr}");
    assert!(stderr.contains("post-commit"), "{stderr}");
}
