//! Integration tests for `wt hook show` command

use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_home_snapshot_settings,
    setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

#[rstest]
fn test_hook_show_with_both_configs(repo: TestRepo, temp_home: TempDir) {
    // Create user config with hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[pre-commit]
user-lint = "pre-commit run --all-files"
"#,
    )
    .unwrap();

    // Create project config with hooks
    repo.write_project_config(
        r#"pre-merge = [
    {build = "cargo build"},
    {test = "cargo test"},
]

[post-start]
deps = "npm install"
"#,
    );
    repo.commit("Add project config");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_no_hooks(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Helper to set up a repo with all hook types configured for filter tests
fn setup_all_hook_types(repo: &TestRepo, temp_home: &TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    repo.write_project_config(
        r#"pre-merge = [
    {build = "cargo build"},
    {test = "cargo test"},
]

[post-start]
deps = "npm install"

[post-merge]
deploy = "scripts/deploy.sh"

[pre-remove]
cleanup = "echo cleanup"

[post-remove]
notify = "echo removed"
"#,
    );
    repo.commit("Add project config");
}

#[rstest]
fn test_hook_show_filter_by_type(repo: TestRepo, temp_home: TempDir) {
    setup_all_hook_types(&repo, &temp_home);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-merge")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_filter_post_merge(repo: TestRepo, temp_home: TempDir) {
    setup_all_hook_types(&repo, &temp_home);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("post-merge")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_filter_pre_remove(repo: TestRepo, temp_home: TempDir) {
    setup_all_hook_types(&repo, &temp_home);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-remove")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_filter_post_remove(repo: TestRepo, temp_home: TempDir) {
    setup_all_hook_types(&repo, &temp_home);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("post-remove")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_approval_status(repo: TestRepo, temp_home: TempDir) {
    // Remove origin so project_identifier uses full canonical path
    repo.run_git(&["remote", "remove", "origin"]);

    // Get the canonical path for the project identifier (escaped for TOML)
    let project_id_str = repo.project_id();

    // Create user config at XDG path with one approved command
    // Use canonical path to handle macOS /var -> /private/var symlinks
    let canonical_home = crate::common::canonicalize(temp_home.path())
        .unwrap_or_else(|_| temp_home.path().to_path_buf());
    let global_config_dir = canonical_home.join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();
    let approvals_path = global_config_dir.join("approvals.toml");
    fs::write(
        &approvals_path,
        format!(
            r#"[projects.'{project_id_str}']
approved-commands = ["cargo build"]
"#
        ),
    )
    .unwrap();

    // Create project config with approved and unapproved hooks
    repo.write_project_config(
        r#"pre-merge = [
    {build = "cargo build"},
    {test = "cargo test"},
]
"#,
    );
    repo.commit("Add project config");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Override config and approvals paths to point to our test files
        cmd.env("WORKTRUNK_CONFIG_PATH", &config_path);
        cmd.env("WORKTRUNK_APPROVALS_PATH", &approvals_path);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_error_with_context_formatting(temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Run wt remove outside a git repo - should show "Failed to remove worktree" context
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("remove").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_project_config_no_hooks(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config without any hook sections
    repo.write_project_config(
        r#"# Project config with no hooks
[list]
url = "http://localhost:8080"
"#,
    );
    repo.commit("Add project config without hooks");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_hook_show_outside_git_repo(temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[pre-commit]
lint = "pre-commit run"
"#,
    )
    .unwrap();

    let mut settings = setup_home_snapshot_settings(&temp_home);
    // Replace temp home path with ~ for stable snapshots (override the [TEMP_HOME] filter)
    // Canonicalize to handle macOS /var -> /private/var symlinks
    let canonical_home = crate::common::canonicalize(temp_home.path())
        .unwrap_or_else(|_| temp_home.path().to_path_buf());
    settings.add_filter(&regex::escape(&canonical_home.to_string_lossy()), "~");
    // Normalize thread IDs in panic messages (e.g., "thread 'main' (1234567)")
    settings.add_filter(r"thread '([^']+)' \(\d+\)", "thread '$1' ([THREAD_ID])");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("hook").arg("show").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that syntax errors in templates are shown (not swallowed) with --expanded.
#[rstest]
fn test_hook_show_expanded_syntax_error(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config with broken template syntax (unclosed brace)
    repo.write_project_config(
        r#"[pre-commit]
broken = "echo {{ branch"
"#,
    );
    repo.commit("Add project config with broken template");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-commit")
            .arg("--expanded")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that undefined variable errors show both template and error with --expanded.
/// The `base` variable is only defined for pre-start hooks, so using it in pre-commit
/// will trigger an undefined variable error that shows both the error and raw template.
#[rstest]
fn test_hook_show_expanded_undefined_var(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config with `base` variable (only defined for pre-start hooks)
    // In pre-commit context, this will be undefined and should show error + template
    repo.write_project_config(
        r#"[pre-commit]
optional-var = "echo {{ base }}"
"#,
    );
    repo.commit("Add project config with optional variable");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-commit")
            .arg("--expanded")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `hook show --format=json` emits one record per configured command, with
/// type / source / template / approval status, plus `expanded` when requested.
#[rstest]
fn test_hook_show_json(repo: TestRepo, temp_home: TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[pre-commit]
user-lint = "pre-commit run --all-files"
"#,
    )
    .unwrap();

    repo.write_project_config(
        r#"pre-merge = [
    {build = "cargo build"},
]

[post-start]
deps = "npm install"
"#,
    );
    repo.commit("Add project config");

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env(
        "WORKTRUNK_CONFIG_PATH",
        global_config_dir.join("config.toml"),
    );
    cmd.args(["hook", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "hook show --format=json failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = parsed.as_array().expect("array");
    assert_eq!(
        entries.len(),
        3,
        "user pre-commit + project pre-merge + project post-start"
    );

    // User entry
    let user_lint = entries
        .iter()
        .find(|e| e["name"] == "user-lint")
        .expect("user-lint present");
    assert_eq!(user_lint["type"], "pre-commit");
    assert_eq!(user_lint["source"], "user");
    assert_eq!(user_lint["template"], "pre-commit run --all-files");
    assert_eq!(user_lint["needs_approval"], false);

    // Project entry — project hooks need approval until approved
    let build = entries
        .iter()
        .find(|e| e["name"] == "build")
        .expect("build present");
    assert_eq!(build["type"], "pre-merge");
    assert_eq!(build["source"], "project");
    assert_eq!(build["needs_approval"], true);
}

/// `hook show <type> --expanded --format=json` filters by hook type across
/// both user and project hooks, and includes the rendered `expanded` field
/// for each surviving entry.
#[rstest]
fn test_hook_show_filtered_expanded_json(repo: TestRepo, temp_home: TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[pre-commit]
user-lint = "echo {{ branch }}"

[post-start]
user-greet = "echo hi"
"#,
    )
    .unwrap();
    repo.write_project_config(
        r#"[pre-commit]
project-fmt = "echo fmt {{ branch }}"

[post-start]
project-deps = "echo deps"
"#,
    );
    repo.commit("Add project config");

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env(
        "WORKTRUNK_CONFIG_PATH",
        global_config_dir.join("config.toml"),
    );
    cmd.args(["hook", "show", "pre-commit", "--expanded", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "hook show --expanded --format=json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = parsed.as_array().expect("array");

    // Filter dropped the post-start hooks from both user and project.
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(entries.len(), 2, "only pre-commit hooks survive: {names:?}");
    assert!(names.contains(&"user-lint"));
    assert!(names.contains(&"project-fmt"));
    for entry in entries {
        assert_eq!(entry["type"], "pre-commit");
        let expanded = entry["expanded"].as_str().expect("expanded field present");
        assert!(
            expanded.starts_with("echo "),
            "expanded should render template: {expanded}"
        );
    }
}

/// Test that valid templates expand correctly with --expanded.
#[rstest]
fn test_hook_show_expanded_valid_template(repo: TestRepo, temp_home: TempDir) {
    // Create user config without hooks
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config with valid template using defined variables
    repo.write_project_config(
        r#"[pre-commit]
valid = "echo branch={{ branch }} repo={{ repo }}"
"#,
    );
    repo.commit("Add project config with valid template");

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook")
            .arg("show")
            .arg("pre-commit")
            .arg("--expanded")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}
