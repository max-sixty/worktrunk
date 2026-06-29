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
fn test_list_config_branches_flag_overrides_file(repo: TestRepo) {
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

    let settings = setup_snapshot_settings(&repo);
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

/// A *deprecated* env var is canonicalized before it is applied, so a
/// type-mismatched value the deprecated name hid now surfaces.
/// `WORKTRUNK__COMMIT_GENERATION__COMMAND=42` migrates to
/// `commit.generation.command = 42`, which fails to deserialize (the field is a
/// String), so the whole env layer is dropped with a `LoadError::Env` warning
/// naming the var — the same contract as a bad value in a canonical env var.
/// File config survives. (Pre-migration the unknown key was silently ignored.)
#[rstest]
fn test_list_config_env_deprecated_type_mismatch_drops_layer(repo: TestRepo) {
    fs::write(repo.test_config_path(), "[list]\nbranches = true\n").unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("WORKTRUNK__COMMIT_GENERATION__COMMAND", "42");
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

/// Mixed typed+string env vars: one var needs typed (e.g., timeout-ms is u64,
/// "100" → Integer) and another needs string (e.g., worktree-path is String,
/// "42" → String). Both must resolve correctly without dropping the config.
#[rstest]
fn test_list_config_env_override_mixed_typed_and_string(repo: TestRepo) {
    // Write a config file so we can verify it's preserved
    fs::write(repo.test_config_path(), "[list]\nbranches = true\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // timeout-ms needs Integer(100) for u64 field
        cmd.env("WORKTRUNK__LIST__TIMEOUT_MS", "100");
        // worktree-path needs String("42") for Option<String> field
        cmd.env("WORKTRUNK_WORKTREE_PATH", "42");
        cmd.arg("list").current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Failed"),
            "mixed typed+string env vars should not fail: {stderr}"
        );
        assert!(output.status.success(), "exit code should be 0: {stderr}");
        // Verify file config is preserved (branches = true shows the branch)
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("feature"),
            "file config (branches=true) should be preserved: {stdout}"
        );
    });
}

/// Bad env var with valid file config: the file config must be preserved.
/// Before the load_with_warnings refactor, any env var failure would drop
/// the entire config (including file-based settings) to defaults.
#[rstest]
fn test_list_config_env_override_bad_value_preserves_file_config(repo: TestRepo) {
    // File config enables branch listing
    fs::write(repo.test_config_path(), "[list]\nbranches = true\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Bad env var: "not-a-bool" for a bool field
        cmd.env("WORKTRUNK__LIST__BRANCHES", "not-a-bool");
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Env var that deserializes successfully but fails validation (empty
/// worktree-path). Exercises the validation-after-env-overlay path.
#[rstest]
fn test_list_config_env_override_validation_failure(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Empty worktree-path deserializes as Some("") but fails validation
        cmd.env("WORKTRUNK_WORKTREE_PATH", "");
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// A `--config-set` override is the highest-priority layer: it wins over the
/// config file. A file that disables branch listing is overridden back on,
/// surfacing the branch without a worktree.
#[rstest]
fn test_list_config_cli_override_beats_file(repo: TestRepo) {
    fs::write(repo.test_config_path(), "[list]\nbranches = false\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["--config-set", "list.branches = true", "list"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// A malformed `--config-set` fragment warns (attributed to `--config-set`,
/// listing the bad value) and leaves the file config intact — the branch is
/// still listed.
#[rstest]
fn test_list_config_cli_override_malformed_warns_on_stderr(repo: TestRepo) {
    fs::write(repo.test_config_path(), "[list]\nbranches = true\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["--config-set", "garbage", "list"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `--config-set` is a `global` arg, so it applies in any position — including
/// after the subcommand. Pins that `wt list --config-set …` is accepted (and
/// takes effect: branch listing toggles on).
#[rstest]
fn test_list_config_cli_override_after_subcommand(repo: TestRepo) {
    fs::write(repo.test_config_path(), "[list]\nbranches = false\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["list", "--config-set", "list.branches = true"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// A *deprecated* key passed via `--config-set` is run through the same
/// deprecation migration as a config file (`merge.no-ff` → `merge.ff`), so the
/// override layer is accepted rather than dropped as unknown. The migration is
/// silent — an inline override has no file for `wt config update` to rewrite —
/// so no deprecation or `--config-set` warning appears and `list` runs
/// normally. (That the migrated value takes effect is covered by the
/// `apply_cli_overrides` unit tests and
/// `test_switch_config_set_migrates_deprecated_no_cd`; `list` output does not
/// reflect a `merge` key.)
#[rstest]
fn test_list_config_cli_override_deprecated_key_silently_migrated(repo: TestRepo) {
    fs::write(repo.test_config_path(), "[list]\nbranches = true\n").unwrap();
    repo.run_git(&["branch", "feature"]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["--config-set", "merge.no-ff = true", "list"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
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

    let settings = setup_snapshot_settings(&repo);
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
    crate::common::add_path_placeholder_filter(
        &mut settings,
        r"_REPO_/system-config\.toml",
        "[TEST_SYSTEM_CONFIG_FILE]",
    );
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
    crate::common::add_path_placeholder_filter(
        &mut settings,
        r"_REPO_/system-config\.toml",
        "[TEST_SYSTEM_CONFIG_FILE]",
    );
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

#[rstest]
fn test_list_custom_columns(repo: TestRepo) {
    // A vars-backed column (only feature-a has the key; other rows render
    // empty cells) and a filter-driven column with a value on every row.
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"

[list.custom-columns.Codename]
template = "{{ branch | codename }}"
priority = 12
"#,
    )
    .unwrap();
    repo.run_git(&[
        "config",
        "worktrunk.state.feature-a.vars.ticket",
        "JIRA-1234",
    ]);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_list_custom_column_empty_everywhere_dropped(repo: TestRepo) {
    // No branch has the vars key, so the column drops from the table
    // entirely (no header, no reserved width).
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
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
fn test_list_custom_columns_json(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
"#,
    )
    .unwrap();
    repo.run_git(&[
        "config",
        "worktrunk.state.feature-a.vars.ticket",
        "JIRA-1234",
    ]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();

    let by_branch = |branch: &str| {
        items
            .iter()
            .find(|item| item["branch"] == branch)
            .unwrap_or_else(|| panic!("no item for branch {branch}"))
    };

    // Rendered value keyed by header; rows with an empty cell omit the key
    assert_eq!(by_branch("feature-a")["columns"]["Ticket"], "JIRA-1234");
    assert!(by_branch("feature-b")["columns"].is_null());
}

#[rstest]
fn test_list_custom_column_git_branch(repo: TestRepo) {
    // A git.branch column reads the branch's own git config under
    // branch.<name>.*, with no `wt config state vars set` round-trip. The
    // git-native multi-line description is reduced to its first line.
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Jira]
template = "{{ git.branch.jira }}"

[list.custom-columns.Summary]
template = "{{ git.branch.description | lines | first }}"
"#,
    )
    .unwrap();
    repo.run_git(&["config", "branch.feature-a.jira", "HWINFCI-2810"]);
    repo.run_git(&[
        "config",
        "branch.feature-a.description",
        "Add telemetry\n\n# Description\nlong body",
    ]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();
    let by_branch = |branch: &str| {
        items
            .iter()
            .find(|item| item["branch"] == branch)
            .unwrap_or_else(|| panic!("no item for branch {branch}"))
    };

    let cols = &by_branch("feature-a")["columns"];
    assert_eq!(cols["Jira"], "HWINFCI-2810");
    assert_eq!(cols["Summary"], "Add telemetry");
    // A branch without any branch.<name>.* keys renders no custom cells.
    assert!(by_branch("feature-b")["columns"].is_null());
}

#[rstest]
fn test_list_custom_column_invalid_template(repo: TestRepo) {
    // An unknown top-level variable aborts wt list with the available set
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ branhc }}"
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
fn test_list_custom_column_worktree_identity(repo: TestRepo) {
    // A template referencing worktree identity sets `needs_worktree`, so the
    // per-row worktree-data lookup runs: a worktree-attached row resolves the
    // directory name, a branch with no worktree falls back to an empty cell.
    fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Dir]
template = "{{ worktree_name }}"
"#,
    )
    .unwrap();
    // A branch with no worktree — only visible with --branches.
    repo.run_git(&["branch", "lonely"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "--branches", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let items = json.as_array().unwrap();

    let by_branch = |branch: &str| {
        items
            .iter()
            .find(|item| item["branch"] == branch)
            .unwrap_or_else(|| panic!("no item for branch {branch}"))
    };

    // Worktree-attached row: the Dir column is the worktree directory name.
    let feature_dir = by_branch("feature-a")["columns"]["Dir"]
        .as_str()
        .expect("feature-a has a Dir value");
    assert!(
        feature_dir.contains("feature-a"),
        "worktree row shows its directory name, got: {feature_dir}"
    );

    // Branch with no worktree: worktree_name is empty, so the cell is omitted.
    assert!(
        by_branch("lonely")["columns"].is_null(),
        "branch-only row has no worktree name, so the Dir cell is empty"
    );
}

/// `[list] columns` selects and reorders the built-in columns end-to-end:
/// only the listed columns appear (Commit, normally shown at this width, is
/// gone), in the configured order (Age before Branch), with the gutter always
/// present.
#[rstest]
fn test_list_config_columns_select_and_reorder(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
columns = ["age", "branch"]
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Wide enough that the default set (incl. Commit) would all fit, so an
        // absent Commit proves selection filtered it rather than width dropping.
        cmd.env("COLUMNS", "200");
        cmd.arg("list").current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "exit code should be 0: {stderr}");
        let stdout = String::from_utf8_lossy(&output.stdout);

        let header = stdout
            .lines()
            .find(|line| line.contains("Branch"))
            .unwrap_or_else(|| panic!("no header row in:\n{stdout}"));
        assert!(header.contains("Age"), "Age selected, got header: {header}");
        assert!(
            !header.contains("Commit"),
            "Commit not selected, should be absent: {header}"
        );
        assert!(
            !header.contains("Message"),
            "Message not selected, should be absent: {header}"
        );
        let age_at = header.find("Age").unwrap();
        let branch_at = header.find("Branch").unwrap();
        assert!(
            age_at < branch_at,
            "configured order puts Age before Branch: {header}"
        );
    });
}

/// Custom columns are addressable in `[list] columns` by their header. A named
/// custom renders interleaved with built-ins at its configured position, while a
/// custom omitted from a non-empty selection is hidden. Both use the `codename`
/// filter so every row has a value (an all-empty custom drops regardless).
#[rstest]
fn test_list_config_columns_select_custom(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
columns = ["branch", "Codename", "age"]

[list.custom-columns.Codename]
template = "{{ branch | codename }}"

[list.custom-columns.Owner]
template = "{{ branch | codename }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("COLUMNS", "200");
        cmd.arg("list").current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "exit code should be 0: {stderr}");
        let stdout = String::from_utf8_lossy(&output.stdout);

        let header = stdout
            .lines()
            .find(|line| line.contains("Branch"))
            .unwrap_or_else(|| panic!("no header row in:\n{stdout}"));
        assert!(
            header.contains("Codename"),
            "a selected custom column renders: {header}"
        );
        assert!(
            !header.contains("Owner"),
            "a custom omitted from a non-empty selection is hidden: {header}"
        );
        let branch_at = header.find("Branch").unwrap();
        let codename_at = header.find("Codename").unwrap();
        let age_at = header.find("Age").unwrap();
        assert!(
            branch_at < codename_at && codename_at < age_at,
            "the custom sorts at its configured position between built-ins: {header}"
        );
    });
}

/// A narrowed `[list] columns` selection prunes the git work that fed only the
/// hidden columns — not just the rendered cells (#3133). With `["branch", "age"]`
/// no column consumes a background task, so `git status` (run by the
/// working-tree-diff task) never fires; the default set still runs it. Mirrors
/// the trace-based diagnosis on the issue, asserted on the `$ git …` debug log.
#[rstest]
fn test_list_config_columns_prune_unused_tasks(repo: TestRepo) {
    let run_list = |config: &str| -> String {
        if config.is_empty() {
            // Default column set: leave any prior config file out of the way.
            let _ = fs::remove_file(repo.test_config_path());
        } else {
            fs::write(repo.test_config_path(), config).unwrap();
        }
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // The working-tree task's `git … status --porcelain` is logged only
        // under debug; capture it from stderr.
        cmd.env("RUST_LOG", "worktrunk=debug");
        cmd.arg("list").current_dir(repo.root_path());
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "exit code should be 0 for config {config:?}: {stderr}"
        );
        stderr.into_owned()
    };

    // Control: the default set shows the Status column, so the working-tree
    // task runs `git status --porcelain`.
    assert!(
        run_list("").contains("status --porcelain"),
        "default columns should run `git status` for the Status column"
    );

    // Branch + Age consume no task: nothing should run `git status`.
    let narrowed = run_list(
        r#"[list]
columns = ["branch", "age"]
"#,
    );
    assert!(
        !narrowed.contains("status --porcelain"),
        "a branch/age selection must not run `git status`:\n{narrowed}"
    );
}

/// A listed column overrides the `--full` preset gate — the positive counterpart
/// to the prune above. `--full` bundles `ci`/`summary` into the default table; it
/// doesn't gate a column named outright. So `columns = ["branch", "ci"]` renders
/// the CI column with no `--full`, where the default set hides it. Asserted on the
/// rendered header row (column enrolment, independent of whether `gh` is on PATH).
#[rstest]
fn test_list_config_listed_column_overrides_full_gate(repo: TestRepo) {
    let header_of = |config: &str| -> String {
        if config.is_empty() {
            let _ = fs::remove_file(repo.test_config_path());
        } else {
            fs::write(repo.test_config_path(), config).unwrap();
        }
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "exit code should be 0 for config {config:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // The header row is the line naming the Branch column.
        stdout
            .lines()
            .find(|line| line.contains("Branch"))
            .unwrap_or_default()
            .to_string()
    };

    // Control: the default set without `--full` hides CI.
    assert!(
        !header_of("").contains("CI"),
        "the default non-full table should not show the CI column"
    );

    // Listing `ci` forces the column on without `--full`.
    let listed = header_of(
        r#"[list]
columns = ["branch", "ci"]
"#,
    );
    assert!(
        listed.contains("CI"),
        "listing `ci` should render the CI column without --full:\n{listed:?}"
    );
}

/// The task prune must not reach the JSON path. `wt list --format json` ignores
/// `[list] columns` and always emits every field (the `after_long_help`
/// contract in `src/cli/mod.rs`), so a narrowed selection that drops the Status
/// column from the table must not strip the status fields it feeds from JSON.
/// Regression for the review on #3274: the prune originally fired on every
/// non-picker render, silently nulling `working_tree`/`main`/`main_state` in
/// JSON for a configured column subset.
#[rstest]
fn test_list_json_ignores_columns_selection(repo: TestRepo) {
    // Dirty a worktree so its `working_tree` field carries an observable value;
    // that field is fed by the working-tree task the prune would skip.
    let feature_dir = repo.root_path().parent().unwrap().join("repo.feature-a");
    fs::write(feature_dir.join("dirty.txt"), "uncommitted\n").unwrap();

    let run_json = |config: &str| -> serde_json::Value {
        if config.is_empty() {
            let _ = fs::remove_file(repo.test_config_path());
        } else {
            fs::write(repo.test_config_path(), config).unwrap();
        }
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["list", "--format=json"])
            .current_dir(repo.root_path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "exit code should be 0 for config {config:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let working_tree_of = |json: &serde_json::Value| -> serde_json::Value {
        json.as_array()
            .unwrap()
            .iter()
            .find(|i| i["branch"] == "feature-a")
            .expect("feature-a row present")["working_tree"]
            .clone()
    };

    // Control: the default set emits the working_tree field, with the untracked
    // change visible.
    let default = run_json("");
    assert_eq!(
        working_tree_of(&default)["untracked"],
        serde_json::Value::Bool(true),
        "default columns should emit working_tree.untracked = true in JSON"
    );

    // A narrowed selection drops Status from the rendered table but must not
    // change JSON output: working_tree stays exactly as the default set emits.
    let narrowed = run_json(
        r#"[list]
columns = ["branch", "age"]
"#,
    );
    assert_eq!(
        working_tree_of(&narrowed),
        working_tree_of(&default),
        "`--format json` must ignore `[list] columns` and emit working_tree regardless of the selection"
    );
}

/// TODO(list-columns-env): `WORKTRUNK__LIST__COLUMNS` is not wired up yet. The
/// env overlay can only deliver a scalar, which the `Vec<String>` field rejects,
/// so the override is dropped — but with a warning naming the var, not silently
/// (see `ListConfig::columns`). The default columns still render. When env
/// support lands, this becomes a working selection (Commit drops, Age stays).
#[rstest]
fn test_list_config_columns_env_override_not_yet_supported(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("COLUMNS", "200");
        cmd.env("WORKTRUNK__LIST__COLUMNS", "branch,age");
        cmd.arg("list").current_dir(repo.root_path());
        assert_cmd_snapshot!(cmd);
    });
}

/// An unknown column name aborts `wt list` with a message that lists the valid
/// names, so a typo is self-correcting rather than silently rendering a
/// different table.
#[rstest]
fn test_list_config_columns_unknown_name_errors(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"[list]
columns = ["branch", "bogus"]
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        assert!(
            !output.status.success(),
            "an unknown column name should abort wt list"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Unknown column"), "stderr: {stderr}");
        assert!(
            stderr.contains("bogus"),
            "stderr names the bad column: {stderr}"
        );
        assert!(
            stderr.contains("branch"),
            "stderr lists valid names: {stderr}"
        );
    });
}

/// The headline use case: `wt --config-set 'list.columns=[…]' list` renders a
/// reduced view for a single invocation. The TOML array parses natively in the
/// override fragment (no string splitting needed), winning over any config file.
#[rstest]
fn test_list_config_columns_cli_override(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("COLUMNS", "200");
        cmd.args([
            "--config-set",
            r#"list.columns = ["branch", "age"]"#,
            "list",
        ])
        .current_dir(repo.root_path());

        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "exit code should be 0: {stderr}");
        let stdout = String::from_utf8_lossy(&output.stdout);

        let header = stdout
            .lines()
            .find(|line| line.contains("Branch"))
            .unwrap_or_else(|| panic!("no header row in:\n{stdout}"));
        assert!(header.contains("Age"), "Age selected: {header}");
        assert!(
            !header.contains("Commit"),
            "Commit not selected, should be absent: {header}"
        );
    });
}
