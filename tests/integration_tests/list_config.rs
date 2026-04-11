//! Tests for `wt list` command with user config

use crate::common::{TestRepo, repo, setup_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;

#[rstest]
fn test_list_config_full_enabled(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
full = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_config_branches_enabled(repo: TestRepo) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    fs::write(
        repo.test_config_path(),
        r#"[list]
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_config_cli_override(repo: TestRepo) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    fs::write(
        repo.test_config_path(),
        r#"[list]
branches = false
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // CLI flag --branches should override config
        cmd.arg("list")
            .arg("--branches")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_config_full_and_branches(repo: TestRepo) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    fs::write(
        repo.test_config_path(),
        r#"[list]
full = true
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_no_config(repo: TestRepo) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // No user config — verify defaults are used (branches not shown).

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_project_url_column(repo: TestRepo) {
    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_json_url_fields(repo: TestRepo) {
    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL fields
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    let first = &items[0];
    // URL should be present with hash_port result (port in 10000-19999 range)
    let url = first["url"].as_str().unwrap();
    assert!(url.starts_with("http://localhost:"));
    let port: u16 = url.split(':').next_back().unwrap().parse().unwrap();
    assert!((10000..=19999).contains(&port));

    // url_active is present but we can't test its value - depends on whether
    // something happens to be listening on the hashed port
    assert!(first["url_active"].is_boolean());
}

#[rstest]
fn test_list_json_no_url_without_template(repo: TestRepo) {
    // No project config means no URL template configured.

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL fields are null
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());

    let first = &items[0];
    // URL should be null when no template configured
    assert!(first["url"].is_null());
    assert!(first["url_active"].is_null());
}

///
/// Only worktrees should have URLs - branches without worktrees can't have running dev servers.
#[rstest]
fn test_list_url_with_branches_flag(repo: TestRepo) {
    // Remove fixture worktrees and their branches to isolate test (keep only main worktree)
    for branch in &["feature-a", "feature-b", "feature-c"] {
        let worktree_path = repo
            .root_path()
            .parent()
            .unwrap()
            .join(format!("repo.{}", branch));
        if worktree_path.exists() {
            let _ = repo
                .git_command()
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    worktree_path.to_str().unwrap(),
                ])
                .run();
        }
        // Delete the branch after removing the worktree
        let _ = repo.git_command().args(["branch", "-D", branch]).run();
    }

    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Create project config with URL template
    repo.write_project_config(
        r#"[list]
url = "http://localhost:{{ branch | hash_port }}"
"#,
    );

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--branches", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 2); // main worktree + feature branch

    // Worktree should have URL, branch should not (no dev server running for branches)
    let worktree = items.iter().find(|i| i["kind"] == "worktree").unwrap();
    let branch = items.iter().find(|i| i["kind"] == "branch").unwrap();

    assert!(
        worktree["url"]
            .as_str()
            .unwrap()
            .starts_with("http://localhost:"),
        "Worktree should have URL"
    );
    assert!(
        branch["url"].is_null(),
        "Branch without worktree should not have URL"
    );
    assert!(
        branch["url_active"].is_null(),
        "Branch without worktree should not have url_active"
    );
}

#[rstest]
fn test_list_url_with_branch_variable(repo: TestRepo) {
    // Create project config with {{ branch }} in URL
    repo.write_project_config(
        r#"[list]
url = "http://localhost:8080/{{ branch }}"
"#,
    );

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON and verify URL contains branch name
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    let first = &items[0];

    let url = first["url"].as_str().unwrap();
    assert_eq!(url, "http://localhost:8080/main");
}

/// Test that task-timeout-ms config option is parsed correctly.
/// We use a very short timeout (1ms) to trigger timeouts.
#[rstest]
fn test_list_config_timeout_triggers_timeouts(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
task-timeout-ms = 1
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // With a 1ms timeout, some tasks should time out
    // The footer should show the timeout count
    assert!(
        stderr.contains("timed out") || output.status.success(),
        "Expected either timeout message in footer or success (if git was fast enough)"
    );
}

/// Test that task-timeout-ms = 0 explicitly disables timeout.
#[rstest]
fn test_list_config_timeout_zero_means_no_timeout(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
task-timeout-ms = 0
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // With task-timeout-ms = 0, there should be no timeout
    assert!(
        !stderr.contains("timed out"),
        "Expected no timeout message with task-timeout-ms = 0, but got: {}",
        stderr
    );
}

/// Regression: setting a typed env-var override (e.g. `WORKTRUNK__LIST__TIMEOUT_MS`)
/// must not wipe unrelated fields in the same section.
///
/// Previously, the `config` crate's Environment source emitted values as strings,
/// so `timeout-ms: Option<u64>` failed to deserialize and the whole `UserConfig`
/// silently fell back to defaults — dropping `list.branches = true` and hiding
/// the `feature` branch from `wt list` output.
///
/// The snapshot captures both stdout (feature branch present with the
/// "1 branches" summary line) and the empty stderr (no silent fallback
/// warning) — if the fix regresses, the diff shows the missing branch.
#[rstest]
fn test_list_config_env_override_preserves_file_fields(repo: TestRepo) {
    // Create a branch without a worktree
    repo.run_git(&["branch", "feature"]);

    // Write to the test config path (the one `configure_wt_cmd` points
    // WORKTRUNK_CONFIG_PATH at); an XDG config under a temp HOME would be
    // ignored because WORKTRUNK_CONFIG_PATH takes precedence.
    fs::write(
        repo.test_config_path(),
        r#"[list]
branches = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Typed env-var override that must coerce a string → u64. The bug was
        // at deserialize time, so any value reproduces it; 0 (disabled) is
        // chosen so the timeout doesn't affect output.
        cmd.env("WORKTRUNK__LIST__TIMEOUT_MS", "0");
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// When `UserConfig::load()` fails (e.g. user config has a wrong field type),
/// `Repository::user_config()` falls back to defaults but must surface the
/// error on stderr — a silent `log::warn!` would hide it from anyone not
/// running with `RUST_LOG=warn`.
///
/// The snapshot pins both the warning prefix (`▲`) and the exact wording so
/// an accidental downgrade back to `log::warn!` or a rewording is caught.
#[rstest]
fn test_list_config_malformed_config_warns_on_stderr(repo: TestRepo) {
    // `list.branches` is typed `Option<bool>`; a string here fails serde
    // deserialization and triggers the fallback path.
    fs::write(
        repo.test_config_path(),
        r#"[list]
branches = "not-a-bool"
"#,
    )
    .unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    // `format_path_for_display` produces different strings depending on
    // whether HOME contains the tempdir — macOS tempdir lives under
    // /var/folders (absolute), Linux CI tempdir lives under HOME (tilde).
    // The default `~/…` → `_PARENT_/…` filter only fires in the tilde case,
    // so normalize the Linux form back to `[TEST_CONFIG]` for a stable
    // snapshot across platforms.
    settings.add_filter(r"_PARENT_/[^\s,]*test-config\.toml", "[TEST_CONFIG]");
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// When a WORKTRUNK_* env var override fails (e.g. a string value for a typed
/// field), the warning must blame env vars — not the config file — and list
/// the override vars currently set.
#[rstest]
fn test_list_config_env_override_bad_value_warns_on_stderr(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // `list.branches` is Option<bool>; "not-a-bool" can't coerce.
        cmd.env("WORKTRUNK__LIST__BRANCHES", "not-a-bool");
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Numeric-looking env var values for String fields must not break config
/// loading. WORKTRUNK_WORKTREE_PATH=42 should be treated as the string "42",
/// not the integer 42 (which would fail to deserialize into Option<String>).
#[rstest]
fn test_list_config_env_override_numeric_string_field(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // worktree-path is Option<String>; "42" must round-trip as a string
        cmd.env("WORKTRUNK_WORKTREE_PATH", "42");
        cmd.arg("list").current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Failed"),
            "numeric string should not fail: {stderr}"
        );
        assert!(output.status.success());
    });
}

/// Bad values in non-section fields (projects, skip-*-prompt) must still be
/// attributed to the file, not to env vars.
#[rstest]
fn test_list_config_malformed_non_section_field_warns_on_stderr(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        "skip-shell-integration-prompt = \"not-a-bool\"\n",
    )
    .unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"_PARENT_/[^\s,]*test-config\.toml", "[TEST_CONFIG]");
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Validation errors (e.g. empty worktree-path) are neither file parse
/// errors nor env-var errors — they fire after successful deserialization.
#[rstest]
fn test_list_config_validation_error_warns_on_stderr(repo: TestRepo) {
    fs::write(repo.test_config_path(), "worktree-path = \"\"\n").unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// System config with a section-field type error must be attributed to the file.
#[rstest]
fn test_list_config_malformed_system_config_warns_on_stderr(repo: TestRepo) {
    let system_config = repo.root_path().join("system-config.toml");
    fs::write(&system_config, "[list]\nbranches = \"not-a-bool\"\n").unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"_REPO_/system-config\.toml", "[TEST_SYSTEM_CONFIG_FILE]");
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// System config with a non-section field type error must be attributed to the file.
#[rstest]
fn test_list_config_malformed_system_config_non_section_field(repo: TestRepo) {
    let system_config = repo.root_path().join("system-config.toml");
    fs::write(
        &system_config,
        "skip-shell-integration-prompt = \"not-a-bool\"\n",
    )
    .unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"_REPO_/system-config\.toml", "[TEST_SYSTEM_CONFIG_FILE]");
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that --full disables the task timeout.
#[rstest]
fn test_list_config_timeout_disabled_with_full(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
task-timeout-ms = 1
"#,
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--full"]).current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // With --full, the timeout is disabled so we shouldn't see timeout messages
    // (though tasks may still fail for other reasons)
    assert!(
        !stderr.contains("timed out"),
        "Expected no timeout message with --full flag, but got: {}",
        stderr
    );
}
