//! Integration tests for `wt step eval`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
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
fn test_eval_dry_run(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["eval", "--dry-run", "{{ branch | hash_port }}"],
        None,
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
