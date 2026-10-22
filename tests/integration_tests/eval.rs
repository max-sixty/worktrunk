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

/// A strict command keeps the invalid override's diagnosis once in its fatal
/// block, independently of the earlier tolerant-load warning.
#[rstest]
fn test_eval_invalid_env_override_keeps_one_fatal_diagnosis(repo: TestRepo) {
    repo.write_test_config("[list]\nbranches = true\n");
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK__LIST__BRANCHES", "not-a-bool")
        .args(["step", "eval", "{{ branch }}"]);

    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.ansi_strip();
    let (_, fatal) = stderr.split_once("✗ Failed to load config").unwrap();
    assert_eq!(fatal.matches("invalid type:").count(), 1, "{stderr}");

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

/// `expected_worktree_path_of_branch` renders the `worktree-path` template for a branch
/// even when another branch occupies that path, which is where
/// `worktree_path_of_branch` comes back empty.
#[rstest]
fn test_eval_expected_worktree_path_of_branch(repo: TestRepo) {
    repo.write_test_config(
        "worktree-path = \"{{ repo_path }}/../{{ repo }}.wt.{{ branch | sanitize }}\"\n",
    );
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let occupied = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{repo_name}.wt.feature-x"));
    repo.run_git(&["worktree", "add", "-b", "other", occupied.to_str().unwrap()]);

    let output = repo
        .wt_command()
        .args([
            "step",
            "eval",
            "{{ expected_worktree_path_of_branch('feature/x') }}|{{ worktree_path_of_branch('feature/x') }}|{{ expected_worktree_path_of_branch('main') }}",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split('|').collect();
    let file_name = |p: &str| p.rsplit(['/', '\\']).next().unwrap().to_string();
    assert_eq!(file_name(parts[0]), format!("{repo_name}.wt.feature-x"));
    assert_eq!(parts[1], "");
    // The default branch lives at the repo root, not a templated path.
    assert_eq!(file_name(parts[2]), repo_name);
}

/// A `worktree-path` template that calls `expected_worktree_path_of_branch` errors
/// instead of recursing.
#[rstest]
fn test_eval_expected_worktree_path_of_branch_in_worktree_path_template(repo: TestRepo) {
    repo.write_test_config("worktree-path = \"{{ expected_worktree_path_of_branch(branch) }}\"\n");
    let output = repo
        .wt_command()
        .args([
            "step",
            "eval",
            "{{ expected_worktree_path_of_branch('feature') }}",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "expected_worktree_path_of_branch cannot be used in the worktree-path template"
        ),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
