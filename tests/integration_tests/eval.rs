//! Integration tests for `wt step eval`

use crate::common::{TestRepo, make_snapshot_cmd, make_snapshot_cmd_with_global_flags, repo};
use ansi_str::AnsiStr;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

#[rstest]
fn test_eval_branch(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "{{ branch }}"],
        None,
    ));
}

/// A command skips an invalid env override the way `wt list` does: the
/// startup warning names it once, and the command still runs.
#[rstest]
fn test_eval_skips_invalid_env_override(repo: TestRepo) {
    repo.write_test_config("[list]\nbranches = true\n");
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK__LIST__BRANCHES", "not-a-bool")
        .args(["step", "eval", "{{ branch }}"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.ansi_strip();
    assert_eq!(stderr.matches("invalid type:").count(), 1, "{stderr}");

    let settings = crate::common::setup_snapshot_settings(&repo);
    settings.bind(|| insta::assert_snapshot!(stderr));
}

#[rstest]
fn test_eval_hash_port(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "{{ branch | hash_port }}"],
        None,
    ));
}

#[rstest]
fn test_eval_multiple_values(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &[
            "eval",
            "{{ branch | hash_port }},{{ (\"supabase-api-\" ~ branch) | hash_port }}"
        ],
        None,
    ));
}

#[rstest]
fn test_eval_sanitize_db(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "{{ branch | sanitize_db }}"],
        None,
    ));
}

#[rstest]
fn test_eval_template_error(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "{{ undefined_var }}"],
        None,
    ));
}

#[rstest]
fn test_eval_verbose(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["eval", "{{ branch | hash_port }}"],
        None,
        &["--verbose"],
    ));
}

#[rstest]
fn test_eval_format_json(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "--format=json", "{{ branch | hash_port }}"],
        None,
    ));
}

/// `--format=json` and `-v` compose: JSON to stdout, the human expansion view
/// to stderr.
#[rstest]
fn test_eval_format_json_verbose(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["eval", "--format=json", "{{ branch | hash_port }}"],
        None,
        &["--verbose"],
    ));
}

#[rstest]
fn test_eval_owner(repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "git@github.com:max-sixty/worktrunk.git",
    ]);

    let output = repo
        .wt_command()
        .args(["step", "eval", "{{ owner }}/{{ repo }}"])
        .output()
        .expect("Failed to run wt step eval");

    assert!(
        output.status.success(),
        "wt step eval should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "max-sixty/repo"
    );
}

#[rstest]
fn test_eval_remote_repo(repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "git@github.com:company-org/project.git",
    ]);

    let output = repo
        .wt_command()
        .args(["step", "eval", "{{ remote_repo }}/{{ repo }}"])
        .output()
        .expect("Failed to run wt step eval");

    assert!(
        output.status.success(),
        "wt step eval should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "project/repo"
    );
}

#[rstest]
fn test_eval_conditional(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &[
            "eval",
            "{% if branch == 'main' %}production{% else %}development{% endif %}"
        ],
        None,
    ));
}

/// `{{ commit }}` resolves to the running worktree's HEAD SHA on the on-branch
/// hot path. `build_hook_context` resolves it via `git rev-parse <branch>`
/// (always fresh from the ref store), so the value tracks any HEAD movement
/// during the command.
#[rstest]
fn test_eval_commit_matches_head_sha(repo: TestRepo) {
    let expected = repo.git_output(&["rev-parse", "HEAD"]);

    let output = repo
        .wt_command()
        .args(["step", "eval", "{{ commit }}"])
        .output()
        .expect("run wt step eval");

    assert!(
        output.status.success(),
        "wt step eval failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        expected,
        "eval commit should match HEAD SHA"
    );
}
