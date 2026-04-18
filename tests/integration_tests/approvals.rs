//! Integration tests for add-approvals and clear-approvals commands

use crate::common::{
    BareRepoTest, TestRepo, TestRepoBase, make_snapshot_cmd, repo, set_temp_home_env,
    setup_snapshot_settings, setup_snapshot_settings_with_home, setup_temp_snapshot_settings,
    temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;
use worktrunk::config::Approvals;

/// Helper to snapshot add-approvals command
fn snapshot_add_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "config", &[], None);
        cmd.arg("approvals").arg("add").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to snapshot clear-approvals command
fn snapshot_clear_approvals(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "config", &[], None);
        cmd.arg("approvals").arg("clear").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// ============================================================================
// add-approvals tests
// ============================================================================

#[rstest]
fn test_add_approvals_no_config(repo: TestRepo) {
    snapshot_add_approvals("add_approvals_no_config", &repo, &[]);
}

#[rstest]
fn test_add_approvals_all_with_none_approved(repo: TestRepo) {
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    snapshot_add_approvals("add_approvals_all_none_approved", &repo, &["--all"]);
}

#[rstest]
fn test_add_approvals_empty_config(repo: TestRepo) {
    repo.write_project_config("");
    repo.commit("Add empty config");

    snapshot_add_approvals("add_approvals_empty_config", &repo, &[]);
}

/// `wt hook approvals` is the deprecated alias for `wt config approvals`.
/// Both `add` and `clear` must emit the deprecation warning and still forward
/// to the same handler.
#[rstest]
#[case::add("add", "hook_approvals_deprecated_add")]
#[case::clear("clear", "hook_approvals_deprecated_clear")]
fn test_hook_approvals_emits_deprecation_warning(
    repo: TestRepo,
    #[case] action: &str,
    #[case] snapshot_name: &str,
) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "hook", &[], None);
        cmd.arg("approvals").arg(action);
        assert_cmd_snapshot!(snapshot_name, cmd);
    });
}

/// Regression: `wt config approvals add` must walk project aliases as well as
/// hooks. With only an alias declared (no hook commands), the alias should
/// appear in the approval batch.
#[rstest]
fn test_add_approvals_includes_aliases(repo: TestRepo) {
    repo.write_project_config(
        r#"[aliases]
deploy = "echo deploying {{ branch }}"
"#,
    );
    repo.commit("Add alias-only project config");

    snapshot_add_approvals("add_approvals_includes_aliases", &repo, &[]);
}

// ============================================================================
// clear-approvals tests
// ============================================================================

#[rstest]
fn test_clear_approvals_no_approvals(repo: TestRepo) {
    snapshot_clear_approvals("clear_approvals_no_approvals", &repo, &[]);
}

#[rstest]
fn test_clear_approvals_with_approvals(repo: TestRepo) {
    // Remove origin so project_id uses directory name (matches test expectation)
    repo.run_git(&["remote", "remove", "origin"]);
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command by writing to test config
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            project_id,
            "echo 'test'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();

    // Now clear approvals
    snapshot_clear_approvals("clear_approvals_with_approvals", &repo, &[]);
}

#[rstest]
fn test_clear_approvals_global_no_approvals(repo: TestRepo) {
    snapshot_clear_approvals("clear_approvals_global_no_approvals", &repo, &["--global"]);
}

#[rstest]
fn test_clear_approvals_global_with_approvals(repo: TestRepo) {
    // Remove origin so project_id uses directory name (matches test expectation)
    repo.run_git(&["remote", "remove", "origin"]);
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            project_id,
            "echo 'test'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();

    // Now clear all global approvals
    snapshot_clear_approvals(
        "clear_approvals_global_with_approvals",
        &repo,
        &["--global"],
    );
}

#[rstest]
fn test_clear_approvals_after_clear(repo: TestRepo) {
    // Remove origin so project_id uses directory name (matches test expectation)
    repo.run_git(&["remote", "remove", "origin"]);
    let project_id = format!("{}/origin", repo.root_path().display());
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            project_id.clone(),
            "echo 'test'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();

    // Clear approvals
    let mut cmd = make_snapshot_cmd(&repo, "config", &[], None);
    cmd.arg("approvals").arg("clear");
    cmd.output().unwrap();

    // Try to clear again (should show "no approvals")
    snapshot_clear_approvals("clear_approvals_after_clear", &repo, &[]);
}

/// `clear` reads approvals from the legacy `[projects.X]` section in `config.toml`
/// when `approvals.toml` is absent, and clears them. Exercises the
/// `Approvals::load` fallback path documented in `src/config/approvals.rs`.
#[rstest]
fn test_clear_approvals_legacy_config_storage(repo: TestRepo, temp_home: TempDir) {
    // Remove origin so project_identifier uses full canonical path
    repo.run_git(&["remote", "remove", "origin"]);

    // Get the canonical path for the project identifier (escaped for TOML)
    let project_id_str = repo.project_id();

    // Write approved commands as a sibling config.toml of the test approvals path.
    // The fallback reads config.toml from the same directory as approvals.toml.
    let config_path = repo.test_approvals_path().with_file_name("config.toml");
    fs::write(
        &config_path,
        format!(
            r#"worktree-path = "../{{{{ repo }}}}.{{{{ branch }}}}"

[projects.'{project_id_str}']
approved-commands = ["cargo build", "cargo test", "npm install"]
"#
        ),
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("WORKTRUNK_CONFIG_PATH", &config_path);
        cmd.args(["config", "approvals", "clear"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!("clear_approvals_legacy_config_storage", cmd);
    });
}

#[rstest]
fn test_clear_approvals_multiple_approvals(repo: TestRepo) {
    // Remove origin so project_id uses directory name (matches test expectation)
    repo.run_git(&["remote", "remove", "origin"]);
    repo.write_project_config(
        r#"
post-create = "echo 'first'"
post-start = "echo 'second'"
[pre-commit]
lint = "echo 'third'"
"#,
    );
    repo.commit("Add config with multiple commands");

    // Manually approve all commands
    let project_id = format!("{}/origin", repo.root_path().display());
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            project_id.clone(),
            "echo 'first'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();
    approvals
        .approve_command(
            project_id.clone(),
            "echo 'second'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();
    approvals
        .approve_command(
            project_id,
            "echo 'third'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();

    // Now clear approvals (should show count of 3)
    snapshot_clear_approvals("clear_approvals_multiple_approvals", &repo, &[]);
}

// ============================================================================
// add-approvals additional coverage tests
// ============================================================================

#[rstest]
fn test_add_approvals_all_already_approved(repo: TestRepo) {
    // Remove origin so project_identifier uses the canonical worktree path —
    // matches what `Repository::project_identifier` computes at runtime.
    repo.run_git(&["remote", "remove", "origin"]);
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create = "echo 'test'""#);
    repo.commit("Add config");

    // Manually approve the command using the same project id wt will compute.
    let mut approvals = Approvals::default();
    approvals
        .approve_command(
            repo.project_id(),
            "echo 'test'".to_string(),
            Some(repo.test_approvals_path()),
        )
        .unwrap();

    // Try to add approvals - should show "all already approved"
    snapshot_add_approvals("add_approvals_all_already_approved", &repo, &[]);
}

#[rstest]
fn test_add_approvals_project_config_no_commands(repo: TestRepo) {
    // Create project config with only non-hook settings
    repo.write_project_config(
        r#"# Project config without any hook sections
[list]
url = "http://localhost:8080"
"#,
    );
    repo.commit("Add config without hooks");

    // Try to add approvals - should show "no commands configured"
    snapshot_add_approvals("add_approvals_no_commands", &repo, &[]);
}

// ============================================================================
// bare repository tests
// ============================================================================

/// Regression test for #1744: `wt config approvals add` should find project config
/// in a bare repo's primary worktree. `config create --project` should place it
/// there (not in the bare repo root), consistent with `ProjectConfig::load`.
#[test]
fn test_add_approvals_bare_repo_config_in_primary_worktree() {
    let test = BareRepoTest::new();
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Write project config in the primary worktree's .config/wt.toml
    // This is where `config create --project` should place it for bare repos
    let config_dir = main_worktree.join(".config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("wt.toml"),
        r#"post-create = "echo 'hello'"
"#,
    )
    .unwrap();

    let settings = setup_temp_snapshot_settings(test.temp_path());
    settings.bind(|| {
        // Run `wt config approvals add --all` from the main worktree
        let mut cmd = wt_command();
        test.configure_wt_cmd(&mut cmd);
        cmd.current_dir(&main_worktree)
            .args(["config", "approvals", "add", "--all"]);
        assert_cmd_snapshot!("add_approvals_bare_repo_config_in_primary_worktree", cmd);
    });
}

/// Test that `project_config_path` returns None (and config create errors)
/// when no linked worktrees exist in a bare repo.
#[test]
fn test_config_create_project_bare_repo_no_worktrees_errors() {
    let test = BareRepoTest::new();
    // Don't create any worktrees — no config location available

    // Run `wt config create --project` from the bare repo root — should fail
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.current_dir(test.bare_repo_path())
        .args(["config", "create", "--project"]);
    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "wt config create --project should fail with no worktrees"
    );

    // Config should NOT be created at the bare repo root
    let bare_root_config = test.bare_repo_path().join(".config").join("wt.toml");
    assert!(
        !bare_root_config.exists(),
        "Config should NOT be created in bare repo root at {:?}",
        bare_root_config
    );
}

/// `config approvals add` and `hook show` should error in a bare repo with
/// no linked worktrees (project_config_path returns None).
#[test]
fn test_hook_commands_bare_repo_no_worktrees_errors() {
    let test = BareRepoTest::new();

    // config approvals add --all should fail
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.current_dir(test.bare_repo_path())
        .args(["config", "approvals", "add", "--all"]);
    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "config approvals add should fail with no worktrees"
    );

    // hook show should fail
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.current_dir(test.bare_repo_path())
        .args(["hook", "show"]);
    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "hook show should fail with no worktrees"
    );
}

/// Regression test for #1744: `wt config create --project` in a bare repo
/// should create config in the primary worktree, not the bare repo root.
#[test]
fn test_config_create_project_bare_repo_uses_primary_worktree() {
    let test = BareRepoTest::new();
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit");

    // Run `wt config create --project` from the bare repo root
    let mut cmd = wt_command();
    test.configure_wt_cmd(&mut cmd);
    cmd.current_dir(test.bare_repo_path())
        .args(["config", "create", "--project"]);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt config create --project failed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Config should be in the primary worktree, NOT the bare repo root
    let primary_config = main_worktree.join(".config").join("wt.toml");
    let bare_root_config = test.bare_repo_path().join(".config").join("wt.toml");
    assert!(
        primary_config.exists(),
        "Config should be created in primary worktree at {:?}",
        primary_config
    );
    assert!(
        !bare_root_config.exists(),
        "Config should NOT be created in bare repo root at {:?}",
        bare_root_config
    );
}
