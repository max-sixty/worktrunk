//! `wt config plugins claude hook`, the command behind every hook in the
//! Claude Code plugin's `hooks/hooks.json`.
use crate::common::{TestRepo, repo};
use rstest::rstest;
use std::io::Write as _;
use std::path::Path;
use std::process::{Output, Stdio};

/// Runs the hook with `payload` on stdin, as Claude Code does from `cwd` for
/// a session launched in `project_dir`.
fn run_hook(repo: &TestRepo, cwd: &Path, project_dir: &Path, payload: &str) -> Output {
    let mut cmd = repo.wt_command();
    cmd.args(["config", "plugins", "claude", "hook"])
        .current_dir(cwd)
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The activity marker of the worktree at `dir`.
fn marker(repo: &TestRepo, dir: &Path) -> String {
    let output = repo
        .wt_command()
        .args(["config", "state", "marker", "get"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Markers land on the session's launch worktree even after a shell `cd`
/// moves the hook's cwd elsewhere (#3921).
#[rstest]
fn test_claude_hook_markers_follow_project_dir(mut repo: TestRepo) {
    let feature = repo.add_worktree("feature");
    let root = repo.root_path().to_path_buf();

    let prompt = run_hook(
        &repo,
        &feature,
        &root,
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
    );
    assert!(prompt.status.success());
    assert_eq!(marker(&repo, &root), "🤖");
    assert_eq!(marker(&repo, &feature), "");

    let stop = run_hook(&repo, &feature, &root, r#"{"hook_event_name":"Stop"}"#);
    assert!(stop.status.success());
    assert_eq!(marker(&repo, &root), "💬");

    let end = run_hook(
        &repo,
        &feature,
        &root,
        r#"{"hook_event_name":"SessionEnd","reason":"exit"}"#,
    );
    assert!(end.status.success());
    assert_eq!(marker(&repo, &root), "");
}

/// A marker that can't be set never fails the hook.
#[rstest]
fn test_claude_hook_marker_failure_succeeds(repo: TestRepo) {
    let missing = repo.home_path().join("missing");
    let output = run_hook(
        &repo,
        repo.root_path(),
        &missing,
        r#"{"hook_event_name":"Notification","message":"waiting"}"#,
    );
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
}

/// `WorktreeCreate` prints the new worktree's path; a failed create exits
/// nonzero with nothing on stdout, so Claude Code reports wt's error rather
/// than a successful hook with no path (#3545).
#[rstest]
fn test_claude_hook_worktree_create(repo: TestRepo) {
    let root = repo.root_path().to_path_buf();
    let payload = r#"{"hook_event_name":"WorktreeCreate","name":"agent-task"}"#;

    let created = run_hook(&repo, &root, &root, payload);
    assert!(created.status.success(), "{created:?}");
    let path = stdout(&created);
    let path = Path::new(path.trim_end());
    assert!(path.join(".git").exists(), "no worktree at {path:?}");
    assert!(path.ends_with("repo.agent-task"), "{path:?}");

    let collision = run_hook(&repo, &root, &root, payload);
    assert!(!collision.status.success());
    assert_eq!(stdout(&collision), "");
}

/// `WorktreeRemove` resolves against the path Claude Code hands it, even when
/// the session's project dir is no repository at all (#3754), and keeps an
/// unmerged branch rather than force-deleting it (#2939).
#[rstest]
fn test_claude_hook_worktree_remove(mut repo: TestRepo) {
    let feature = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature, "work.txt", "work", "unpushed work");
    let outside = repo.home_path().to_path_buf();

    let payload = serde_json::json!({
        "hook_event_name": "WorktreeRemove",
        "worktree_path": feature,
    });
    let output = run_hook(&repo, &outside, &outside, &payload.to_string());
    assert!(output.status.success(), "{output:?}");
    assert!(!feature.exists());
    let branches = repo
        .git_command()
        .args(["branch", "--list", "feature"])
        .run()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("feature"));

    // A path that is already gone is nothing to remove.
    let again = run_hook(&repo, &outside, &outside, &payload.to_string());
    assert!(again.status.success());
}
