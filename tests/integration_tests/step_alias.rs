//! Integration tests for `wt step <alias>`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Alias from project config runs with template expansion
#[rstest]
fn test_step_alias_from_project_config(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["hello"],
        Some(&feature_path),
    ));
}

/// --dry-run shows the expanded command without running it
#[rstest]
fn test_step_alias_dry_run(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["hello", "--dry-run"],
        Some(&feature_path),
    ));
}

/// Unknown alias shows error with available aliases
#[rstest]
fn test_step_alias_unknown_with_available(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello"
deploy = "make deploy"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["nonexistent"],
        Some(&feature_path),
    ));
}

/// Unknown step command with no aliases configured
#[rstest]
fn test_step_alias_unknown_no_aliases(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
    ));
}

/// --var flag adds extra template variables
#[rstest]
fn test_step_alias_with_var(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
greet = "echo Hello {{ name }} from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["greet", "--dry-run", "--var", "name=World"],
        Some(&feature_path),
    ));
}

/// Alias command failure propagates exit code
#[rstest]
fn test_step_alias_exit_code(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
fail = "exit 42"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["fail"],
        Some(&feature_path),
    ));
}

/// Alias from user config works
#[rstest]
fn test_step_alias_from_user_config(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
greet = "echo Greetings from {{ branch }}"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["greet"],
        Some(&feature_path),
    ));
}

/// Alias shadowing a built-in step command shows a warning
#[rstest]
fn test_step_alias_shadows_builtin(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
commit = "echo custom-commit"
hello = "echo hello"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Running a non-shadowed alias should show the warning about "commit"
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["hello", "--dry-run"],
        Some(&feature_path),
    ));
}

/// User config aliases merge with project config aliases
#[rstest]
fn test_step_alias_merge_user_and_project(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
project-cmd = "echo from-project"
shared = "echo project-version"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
user-cmd = "echo from-user"
shared = "echo user-version"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // User alias available
    assert_cmd_snapshot!(
        "user_alias",
        make_snapshot_cmd(
            &repo,
            "step",
            &["user-cmd", "--dry-run"],
            Some(&feature_path),
        )
    );

    // Project alias available
    assert_cmd_snapshot!(
        "project_alias",
        make_snapshot_cmd(
            &repo,
            "step",
            &["project-cmd", "--dry-run"],
            Some(&feature_path),
        )
    );

    // User overrides project on collision
    assert_cmd_snapshot!(
        "user_overrides_project",
        make_snapshot_cmd(&repo, "step", &["shared", "--dry-run"], Some(&feature_path),)
    );
}
