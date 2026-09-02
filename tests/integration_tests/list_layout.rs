use crate::common::{TestRepo, list_snapshots, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

fn snapshot_list(repo: &TestRepo, name: &str, width: usize) {
    let settings = setup_snapshot_settings(repo);
    let mut cmd = list_snapshots::command_with_width(repo, width);
    settings.bind(|| assert_cmd_snapshot!(name, cmd));
}

/// One deliberately heterogeneous table exercises the integration between git
/// state collection and the renderer. Exhaustive column geometry belongs in
/// the direct layout tests in `src/commands/list/layout.rs`.
#[rstest]
fn diverse_rows_stay_aligned(mut repo: TestRepo) {
    let feature_a = repo.worktrees["feature-a"].clone();
    let many_lines = (0..120)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    std::fs::write(feature_a.join("feature-a.txt"), many_lines).unwrap();
    repo.run_git_in(&feature_a, &["add", "feature-a.txt"]);
    std::fs::write(feature_a.join("untracked.txt"), "new\n").unwrap();

    let feature_b = repo.worktrees["feature-b"].clone();
    std::fs::remove_file(feature_b.join("feature-b.txt")).unwrap();

    repo.add_worktree("日本語");

    snapshot_list(&repo, "diverse_rows", 180);
}

/// These widths represent the three distinct user-facing regimes: the minimum
/// useful table, a compact status table, and the usual rich table. Exact
/// allocation boundaries are covered by the direct layout tests.
#[rstest]
fn representative_widths_render_useful_tables(mut repo: TestRepo) {
    let long_branch = repo.add_worktree("feature/implement-oauth2-social-login");
    std::fs::write(long_branch.join("oauth.rs"), "changed\n").unwrap();

    for width in [30, 60, 120] {
        snapshot_list(&repo, &format!("width_{width}"), width);
    }
}

/// With no remote configured, `Remote⇅` is blank on every row, so it ranks
/// below every populated column and drops first. At this width it used to
/// hold its blank seven columns open while Message — which has something to
/// say on every row — was dropped for want of them.
#[rstest]
fn empty_remote_column_drops_before_populated_ones(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("feature-with-a-longer-name");

    snapshot_list(&repo, "no_remote_width_100", 100);
}

/// One very long branch must not size the Branch column for every row: past
/// the cap the name is elided, so the columns that answer "what is going on
/// here" still fit. Without it, 60 columns degenerates into a branch list and
/// 200 still loses Message.
#[rstest]
fn one_long_branch_does_not_size_the_table(mut repo: TestRepo) {
    repo.add_worktree("a-very-long-branch-name-that-is-fifty-eight-chars-long-ok");

    for width in [60, 100, 200] {
        snapshot_list(&repo, &format!("long_branch_width_{width}"), width);
    }
}
