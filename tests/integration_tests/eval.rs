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
