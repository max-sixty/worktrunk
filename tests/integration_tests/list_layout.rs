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
