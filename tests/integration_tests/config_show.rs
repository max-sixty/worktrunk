use crate::common::{
    TestRepo, canonical_temp_home, repo, set_temp_home_env, set_xdg_config_path,
    setup_home_snapshot_settings, setup_snapshot_settings, setup_snapshot_settings_with_home,
    temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

#[rstest]
fn test_config_show_with_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config at XDG path (used on all platforms with etcetera)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();
    fs::write(
        global_config_dir.join("approvals.toml"),
        r#"[projects."test-project"]
approved-commands = ["npm install"]
"#,
    )
    .unwrap();

    // Create project config
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-start = "npm install"

[post-start]
server = "npm run dev"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_no_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config (but no project config) at XDG path
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

// ==================== System Config Tests ====================

#[rstest]
fn test_config_show_with_system_config(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create system config in a temp directory
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"[merge]
squash = true
verify = true

[commit.generation]
command = "company-llm-tool"
"#,
    )
    .unwrap();

    // Create user config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_system_config_values_used_as_defaults(repo: TestRepo) {
    // System config with a distinctive worktree-path template
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        "worktree-path = \".worktrees/{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    // No user config — system config should provide the worktree-path default
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.args(["switch", "--create", "test-feature"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "switch --create should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Worktree should be at the system config template path
    let expected_path = repo.root_path().join(".worktrees").join("test-feature");
    assert!(
        expected_path.exists(),
        "Worktree should be created at system config template path: {}",
        expected_path.display()
    );
}

#[rstest]
fn test_user_config_overrides_system_config(repo: TestRepo) {
    // System config with one template
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        "worktree-path = \".worktrees/system/{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    // User config overrides with a different template
    let user_config_dir = tempfile::tempdir().unwrap();
    let user_config_path = user_config_dir.path().join("config.toml");
    fs::write(
        &user_config_path,
        "worktree-path = \".worktrees/user/{{ branch | sanitize }}\"\n",
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
    cmd.args(["switch", "--create", "test-feature"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "switch --create should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Should use user config template, not system
    let user_path = repo.root_path().join(".worktrees/user/test-feature");
    let system_path = repo.root_path().join(".worktrees/system/test-feature");
    assert!(
        user_path.exists(),
        "Worktree should be at user config template path: {}",
        user_path.display()
    );
    assert!(
        !system_path.exists(),
        "Worktree should NOT be at system config template path"
    );
}

/// System and user config hooks are deep-merged by the config crate at the TOML
/// key level. Differently-named commands within the same hook type coexist —
/// system hooks and user hooks both run. Same-named commands: user replaces system.
#[rstest]
fn test_system_and_user_hooks_deep_merged(repo: TestRepo) {
    // System config defines a named pre-merge hook
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"[pre-merge]
company-lint = "company-lint-tool"
"#,
    )
    .unwrap();

    // User config defines a differently-named pre-merge hook
    let user_config_dir = tempfile::tempdir().unwrap();
    let user_config_path = user_config_dir.path().join("config.toml");
    fs::write(
        &user_config_path,
        r#"[pre-merge]
my-lint = "my-lint-tool"
"#,
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
    cmd.args(["hook", "show", "pre-merge"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "hook show should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Both hooks should be present (deep merge preserves differently-named keys)
    assert!(
        stdout.contains("company-lint-tool"),
        "System hook should be preserved with different name, got:\n{stdout}"
    );
    assert!(
        stdout.contains("my-lint-tool"),
        "User hook should be present, got:\n{stdout}"
    );
}

/// When user config defines a hook with the same name as system config,
/// the user's command replaces the system's command for that name.
#[rstest]
fn test_user_hook_replaces_same_named_system_hook(repo: TestRepo) {
    // System config defines a named hook
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"[pre-merge]
lint = "company-lint-tool"
"#,
    )
    .unwrap();

    // User config defines the same-named hook with different command
    let user_config_dir = tempfile::tempdir().unwrap();
    let user_config_path = user_config_dir.path().join("config.toml");
    fs::write(
        &user_config_path,
        r#"[pre-merge]
lint = "my-lint-tool"
"#,
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
    cmd.args(["hook", "show", "pre-merge"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "hook show should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // User's command should replace system's for the same name
    assert!(
        stdout.contains("my-lint-tool"),
        "User's hook command should be present, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("company-lint-tool"),
        "System's hook command should be replaced by user's same-named hook, got:\n{stdout}"
    );
}

/// When user config doesn't define a hook type, the system config's hook is preserved.
#[rstest]
fn test_system_config_hooks_preserved_when_user_doesnt_override(repo: TestRepo) {
    // System config defines pre-merge and pre-commit hooks
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"[pre-merge]
company-lint = "company-lint-tool"

[pre-commit]
company-format = "company-format-tool"
"#,
    )
    .unwrap();

    // User config only defines pre-merge (should leave system's pre-commit intact)
    let user_config_dir = tempfile::tempdir().unwrap();
    let user_config_path = user_config_dir.path().join("config.toml");
    fs::write(
        &user_config_path,
        r#"[pre-merge]
my-lint = "my-lint-tool"
"#,
    )
    .unwrap();

    // Check pre-commit — should still have system's hook
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
    cmd.args(["hook", "show", "pre-commit"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "hook show should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("company-format-tool"),
        "System's pre-commit hook should be preserved when user doesn't override it, got:\n{stdout}"
    );
}

/// A current-worktree `.config/wt.toml` that uses the deprecated
/// `pre-create`/`post-create` hook keys keeps working through `wt hook show`:
/// `ProjectConfig::load` migrates the keys to canonical `pre-start`/`post-start`
/// before deserializing, and the type-argument value parser accepts both the
/// canonical name and the deprecated alias. The rename is paused at Phase 1
/// (issue #2838), so the migration is silent — no deprecation warning.
#[rstest]
fn test_hook_show_accepts_deprecated_create_hooks(repo: TestRepo) {
    // Single-token commands so the bash highlighter doesn't split them with
    // ANSI codes (same shape as the deep-merge `hook show` tests above).
    repo.write_project_config(
        r#"[pre-create]
deps = "pre-create-tool"

[post-create]
deps = "post-create-tool"
"#,
    );

    // The `-start` arg passes the canonical type name; the `-create` arg
    // exercises the value parser accepting the deprecated alias. Both resolve
    // to the same hook because `ProjectConfig::load` migrates the `[*-create]`
    // config keys to canonical `[*-start]` before deserializing.
    let cases = [
        ("pre-start", "pre-create-tool"),
        ("pre-create", "pre-create-tool"),
        ("post-start", "post-create-tool"),
        ("post-create", "post-create-tool"),
    ];
    for (type_arg, expected) in cases {
        let mut cmd = repo.wt_command();
        cmd.args(["hook", "show", type_arg])
            .current_dir(repo.root_path());
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "`wt hook show {type_arg}` should succeed, stderr: {stderr}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "`wt hook show {type_arg}` should list the migrated hook, got:\n{stdout}"
        );
        // Phase 1 migrates silently — no deprecation warning, no config-show hint.
        assert!(
            !stderr.to_lowercase().contains("deprecated") && !stderr.contains("config show"),
            "`wt hook show {type_arg}` should migrate silently, stderr:\n{stderr}"
        );
    }
}

#[rstest]
fn test_config_show_system_config_hint_under_user_config(repo: TestRepo, temp_home: TempDir) {
    // When no system config exists but user config does, config show should
    // display a hint under USER CONFIG with the platform-specific default path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n",
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    set_xdg_config_path(&mut cmd, temp_home.path());
    cmd.env_remove("WORKTRUNK_SYSTEM_CONFIG_PATH");
    cmd.arg("config").arg("show").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should NOT show a full SYSTEM CONFIG heading
    assert!(
        !stdout.contains("SYSTEM CONFIG"),
        "Should not show SYSTEM CONFIG section when absent, got:\n{stdout}"
    );
    // Should show a system config hint under USER CONFIG
    assert!(
        stdout.contains("Optional system config not found")
            && stdout.contains("worktrunk/config.toml"),
        "Expected system config hint in output, got:\n{stdout}"
    );
}

#[rstest]
fn test_system_config_found_via_xdg_config_dirs(repo: TestRepo) {
    // Create system config in a custom XDG directory
    let xdg_dir = tempfile::tempdir().unwrap();
    let config_dir = xdg_dir.path().join("worktrunk");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"worktree-path = "/xdg-org/{{ repo }}/{{ branch | sanitize }}"
"#,
    )
    .unwrap();

    // Use XDG_CONFIG_DIRS instead of WORKTRUNK_SYSTEM_CONFIG_PATH
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env_remove("WORKTRUNK_SYSTEM_CONFIG_PATH");
    cmd.env("XDG_CONFIG_DIRS", xdg_dir.path());
    cmd.arg("list")
        .arg("--format=json")
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let worktrees = json.as_array().unwrap();

    for wt in worktrees {
        if wt["is_primary"].as_bool() == Some(false) {
            let path = wt["path"].as_str().unwrap();
            assert!(
                path.contains("/xdg-org/"),
                "Expected XDG_CONFIG_DIRS system config, got: {path}"
            );
        }
    }
}

#[rstest]
fn test_system_config_xdg_dirs_set_but_no_config_found(repo: TestRepo) {
    // When XDG_CONFIG_DIRS is set but contains no worktrunk config,
    // system config should be None (no fallback to platform defaults)
    let empty_xdg_dir = tempfile::tempdir().unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env_remove("WORKTRUNK_SYSTEM_CONFIG_PATH");
    cmd.env("XDG_CONFIG_DIRS", empty_xdg_dir.path());
    cmd.arg("list")
        .arg("--format=json")
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    // Without system config, worktree paths should use the default template
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let worktrees = json.as_array().unwrap();

    for wt in worktrees {
        if wt["is_primary"].as_bool() == Some(false) {
            let path = wt["path"].as_str().unwrap();
            assert!(
                !path.contains("/xdg-org/"),
                "Should not use XDG system config path, got: {path}"
            );
        }
    }
}

/// Test that `config show` displays empty system config with a hint
#[rstest]
fn test_config_show_empty_system_config(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create empty system config
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(&system_config_path, "").unwrap();

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that `config show` displays invalid system config with error details
#[rstest]
fn test_config_show_invalid_system_config(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create system config with invalid TOML
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(&system_config_path, "invalid = [toml\n").unwrap();

    // Create user config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that system config with unknown keys triggers a warning during config loading
#[rstest]
fn test_system_config_unknown_keys_warning_during_load(repo: TestRepo) {
    // Create system config with an unknown key
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        "[totally-unknown-section]\nkey = \"value\"",
    )
    .unwrap();

    // Run `wt list` which triggers config loading and unknown key warnings
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has unknown field"),
        "Expected unknown field warning from system config load, got: {stderr}"
    );
}

/// System config should use the same deprecation warning gate as user config.
#[rstest]
fn test_system_config_deprecation_warning_during_load(repo: TestRepo) {
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"[select]
pager = "delta --paging=never"
"#,
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("System config") && stderr.contains("[select]"),
        "Expected deprecation warning from system config load, got: {stderr}"
    );
    assert!(
        stderr.contains("[switch.picker]"),
        "Expected replacement section in warning, got: {stderr}"
    );
}

/// System config is routed through the same deprecation gate as user config:
/// a non-fatal deprecation surfaces a warning, and a deprecated hook key is
/// migrated to its canonical name and stays active.
#[rstest]
fn test_system_config_deprecations_pass_through_gate(repo: TestRepo) {
    let system_config_dir = tempfile::tempdir().unwrap();
    let system_config_path = system_config_dir.path().join("config.toml");
    fs::write(
        &system_config_path,
        r#"post-create = "npm install"

[select]
pager = "delta --paging=never"
"#,
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_SYSTEM_CONFIG_PATH", &system_config_path)
        .env("NO_COLOR", "1");
    cmd.args(["hook", "show", "post-start"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("System config") && stderr.contains("[select]"),
        "non-fatal deprecation in system config should warn, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("npm install"),
        "deprecated post-create key should migrate to post-start and stay active, got: {stdout}"
    );
}

#[rstest]
fn test_config_show_outside_git_repo(mut repo: TestRepo, temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config at XDG path
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_zsh_compinit_warning(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .zshrc WITHOUT compinit - completions won't work
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# wt integration but no compinit!
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Cover the per-shell "Skipped" row in `wt config show`. After #2574,
/// `scan_shell_configs` only adds a shell to `skipped` when its binary is on
/// PATH AND no rc file/dir exists — without an installed binary the entry
/// never reaches `render_shell_status`. Here bash is flagged installed (test
/// override) with no `.bashrc`, so the Skipped row renders and the styling
/// (`<bold>{shell}</>: <dim>Skipped; {path} not found</>`) is verified.
#[rstest]
fn test_config_show_skipped_with_installed_binary(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_BASH_INSTALLED", "1");
        cmd.env("SHELL", "/bin/zsh");

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_partial_shell_config_shows_hint(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .bashrc WITHOUT wt integration
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Some bash config
export PATH="$HOME/bin:$PATH"
"#,
    )
    .unwrap();

    // Create .zshrc WITH wt integration
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# wt integration
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1"); // Bypass zsh subprocess check
        // Set SHELL=zsh so the "verify wrapper loaded" hint appears under zsh
        // (configured + matches current shell) but is suppressed under bash.
        cmd.env("SHELL", "/bin/zsh");

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that config show displays fish shell with completions configured
#[rstest]
fn test_config_show_fish_with_completions(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create fish functions directory with wt.fish (shell extension configured)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    // Create fish completions file (completions configured)
    let completions = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions).unwrap();
    fs::write(completions.join("wt.fish"), "# fish completions\n").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that config show displays fish shell without completions configured
#[rstest]
fn test_config_show_fish_without_completions(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create fish functions directory with wt.fish (shell extension configured)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    // Do NOT create fish completions file - completions not configured

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that config show displays "Outdated" when fish wrapper exists but has different code.
///
/// The wrapper file contains `function wt` which matches the command name at a word boundary,
/// but `is_shell_integration_line()` won't match it (it looks for eval/source patterns).
/// This should NOT trigger a "Found wt ... but not detected as integration" warning because
/// the wrapper file IS the integration — `scan_shell_configs` already identified it (as
/// outdated). Only the "Outdated shell extension" warning should appear.
#[rstest]
fn test_config_show_fish_outdated_wrapper(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create fish functions directory with an outdated wt.fish (different functional code)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    fs::write(
        functions.join("wt.fish"),
        "# worktrunk shell integration for fish\nfunction wt\n    command wt-old $argv\nend\n",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that config show displays "Outdated" when nushell wrapper exists but has different code.
///
/// Same false-positive suppression as the fish variant: the wrapper file contains `def --wrapped wt`
/// which matches the command name, but `scan_shell_configs` already recognized the file as
/// integration (outdated). Only the "Outdated" warning should appear, not the generic
/// "not detected as integration" warning.
#[rstest]
fn test_config_show_nushell_outdated_wrapper(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create the nushell vendor-autoload directory with an outdated wt.nu.
    // Pin the dir via the test override so the path is deterministic across
    // platforms (and independent of whether `nu` is installed).
    let autoload = canonical_temp_home(&temp_home).join(".local/share/nushell/vendor/autoload");
    fs::create_dir_all(&autoload).unwrap();
    fs::write(
        autoload.join("wt.nu"),
        "# worktrunk shell integration for nushell\ndef --wrapped wt [...args] {\n    command wt-old ...$args\n}\n",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_NU_VENDOR_AUTOLOAD_DIR", &autoload);

        assert_cmd_snapshot!(cmd);
    });
}

/// Cover the `.exe` alias hint inside `render_already_configured`. The hint
/// fires when at least one matched integration line in the rc file uses
/// `wt.exe` — common on Git Bash where the binary is `wt.exe` but POSIX
/// aliases must still target `wt`. We pair the canonical line (so the row
/// hits `AlreadyExists`) with an extra `wt.exe` line in the same file so
/// `detection.matched_lines` carries the `.exe` content.
#[rstest]
fn test_config_show_bash_matched_exe_emits_alias_hint(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Canonical line so the row reports AlreadyExists.
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init bash)"; fi
# Stale wt.exe form left over from a Windows install — scan_for_detection_details
# still recognizes it as a matched integration line.
eval "$(wt.exe config shell init bash)"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1");
        cmd.env("WORKTRUNK_TEST_BASH_INSTALLED", "1");
        // SHELL=bash so current_shell() == Bash and the verify-wrapper hint
        // appears under bash (not duplicated for other shells).
        cmd.env("SHELL", "/bin/bash");

        // Pre-snapshot guard: confirm the unredacted output really did emit
        // the alias hint. The shared `wt\.exe` → `wt` redaction collapses
        // both `<underline>wt</>` and `<underline>wt.exe</>` to the same
        // visual `wt` in the snapshot, which would otherwise hide whether
        // the .exe branch ran at all.
        let raw = String::from_utf8_lossy(&cmd.output().unwrap().stdout).to_string();
        assert!(
            raw.contains("Creates shell function") && raw.contains("not ") && raw.contains(".exe"),
            "Expected the .exe alias hint in unredacted output, got:\n{raw}"
        );

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_zsh_compinit_correct_order(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .zshrc with compinit enabled - completions will work
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# compinit enabled
autoload -Uz compinit && compinit

# wt integration
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1"); // Bypass zsh subprocess check (unreliable on CI)

        assert_cmd_snapshot!(cmd);
    });
}

/// Smoke-test the actual zsh probe path (no WORKTRUNK_TEST_COMPINIT_* overrides).
///
/// This is behind shell-integration-tests because it requires `zsh` to be installed.
#[rstest]
#[cfg(all(unix, feature = "shell-integration-tests"))]
fn test_config_show_zsh_compinit_real_probe_warns_when_missing(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .zshrc with the canonical integration line (exact match required for config show),
    // plus an explicit removal of compdef so the probe is deterministic.
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"unset -f compdef 2>/dev/null
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Keep PATH minimal so the probe zsh doesn't find a globally-installed `wt`.
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env(
            "ZDOTDIR",
            crate::common::canonicalize(temp_home.path())
                .unwrap_or_else(|_| temp_home.path().to_path_buf()),
        );
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Completions won't work; add to"),
            "Expected compinit warning, got:\n{stdout}"
        );
    });
}

/// Smoke-test the actual zsh probe path when compdef exists.
///
/// This is behind shell-integration-tests because it requires `zsh` to be installed.
#[rstest]
#[cfg(all(unix, feature = "shell-integration-tests"))]
fn test_config_show_zsh_compinit_no_warning_when_present(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Define compdef directly to avoid relying on compinit behavior (which can warn
    // about insecure directories in CI). The probe checks for compdef presence.
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"compdef() { :; }
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        // Keep PATH minimal so the probe zsh doesn't find a globally-installed `wt`.
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env(
            "ZDOTDIR",
            crate::common::canonicalize(temp_home.path())
                .unwrap_or_else(|_| temp_home.path().to_path_buf()),
        );
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("Completions won't work; add to"),
            "Expected no compinit warning, got:\n{stdout}"
        );
    });
}

#[rstest]
fn test_config_show_warns_unknown_project_keys(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    // Create project config with typo: post-merge-command instead of post-merge
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "[post-merge-command]\ndeploy = \"task deploy\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_warns_unknown_user_keys(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config with typo: commit-gen instead of commit-generation
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n\n[commit-gen]\ncommand = \"llm\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Tests that loading a config with a truly unknown key (not valid in either config type)
/// emits a warning during config loading (not just config show).
#[rstest]
fn test_unknown_project_key_warning_during_load(repo: TestRepo, temp_home: TempDir) {
    // Create project config with truly unknown key (not valid in either config type)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "[invalid-section-name]\nkey = \"value\"",
    )
    .unwrap();

    // Run `wt list` which loads project config via ProjectConfig::load()
    // This triggers warn_unknown_fields (different from warn_unknown_keys used by config show)
    let mut cmd = repo.wt_command();
    cmd.arg("list").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has unknown field"),
        "Expected unknown field warning during config load, got: {stderr}"
    );
}

/// Tests that when a user-config-only key (commit-generation) appears in project config,
/// the warning suggests moving it to user config.
#[rstest]
fn test_config_show_suggests_user_config_for_commit_generation(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create empty global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    // Create project config with commit-generation (which belongs in user config)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "[commit-generation]\ncommand = \"claude\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Tests that when a project-config-only key (ci) appears in user config,
/// the warning suggests moving it to project config.
#[rstest]
fn test_config_show_suggests_project_config_for_ci(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config with ci section (which belongs in project config)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n\n[ci]\nplatform = \"github\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_invalid_user_toml(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config with invalid TOML syntax
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "this is not valid toml {{{",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_invalid_project_toml(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create valid global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    // Create project config with invalid TOML syntax
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), "invalid = [unclosed bracket").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_full_not_configured(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create isolated config directory
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Inject current version for deterministic version check output
        cmd.env("WORKTRUNK_TEST_LATEST_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_full_command_not_found(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create isolated config directory
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[commit.generation]
command = "nonexistent-llm-command-12345 -m test-model"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Inject current version for deterministic version check output
        cmd.env("WORKTRUNK_TEST_LATEST_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_full_update_available(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create isolated config directory
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Inject a higher version to trigger update-available message
        cmd.env("WORKTRUNK_TEST_LATEST_VERSION", "99.0.0");
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_full_version_check_unavailable(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Simulate a fetch failure
        cmd.env("WORKTRUNK_TEST_LATEST_VERSION", "error");
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config show --full` against a Gitea remote reports the `tea` CLI row.
#[rstest]
fn test_config_show_full_gitea_remote(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    // The Gitea diagnostics row only depends on `tea`'s state; mock it as
    // installed (no login configured in the temp home).
    repo.setup_mock_tea_installed();

    // The fixture already has an `origin`; point it at a Gitea host.
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitea.example.com/example/repo.git",
    ]);

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.env("WORKTRUNK_TEST_LATEST_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_github_remote(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // The fixture already has an `origin`; point it at a GitHub host.
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/example/repo.git",
    ]);

    // Create fake global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_gitlab_remote(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // The fixture already has an `origin`; point it at a GitLab host.
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/example/repo.git",
    ]);

    // Create fake global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_empty_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create empty project config file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_whitespace_only_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create project config file with only whitespace
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), "   \n\t\n  ").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

///
/// Should show a hint about creating the config and display the default configuration.
#[rstest]
fn test_config_show_no_user_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Don't create any user config file - temp_home is empty

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

///
/// When a shell config contains `wt` at a word boundary but it's NOT detected as
/// shell integration, show a warning with file:line format to help debug detection.
#[rstest]
fn test_config_show_unmatched_candidate_warning(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .bashrc with a line containing `wt` but NOT a valid integration pattern
    // This should trigger the "unmatched candidate" warning
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Some bash config
export PATH="$HOME/bin:$PATH"
alias wt="git worktree"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1");

        assert_cmd_snapshot!(cmd);
    });
}

/// Cover the unmatched-candidate `.exe` hint in `render_shell_status`. When
/// the detector finds a `wt`-mentioning line that doesn't look like a valid
/// integration AND any of the unmatched candidates contains `.exe`, an
/// extra hint explains the function-name vs alias-name distinction.
#[rstest]
fn test_config_show_unmatched_candidate_exe_emits_extra_hint(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // `.bashrc` line containing `wt` at a word boundary but not in a
    // shell-integration shape — the detector flags it as an unmatched
    // candidate, and the `.exe` token triggers the extra alias hint.
    // Use `export` rather than `alias` so the line doesn't get classified
    // as a bypass alias (which would emit a separate warning instead).
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Custom shim path pointing at a Windows binary
export WT_BIN="/c/Program Files/wt.exe"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1");

        // Pre-snapshot guard: the shared `wt\.exe` → `wt` redaction would
        // otherwise hide whether the unmatched-candidate `.exe` branch ran.
        let raw = String::from_utf8_lossy(&cmd.output().unwrap().stdout).to_string();
        assert!(
            raw.contains("creates shell function") && raw.contains(".exe"),
            "Expected the .exe alias hint in unredacted output, got:\n{raw}"
        );

        assert_cmd_snapshot!(cmd);
    });
}

/// Verify that the unmatched candidate warning fires for a bash alias while being suppressed
/// for a Fish wrapper file in the same `config show` run. This ensures wrapper-file suppression
/// is path-specific and doesn't accidentally silence all unmatched candidate warnings.
#[rstest]
fn test_config_show_unmatched_candidate_not_suppressed_by_wrapper(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Fish wrapper (outdated) — should NOT trigger "not detected" warning
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    fs::write(
        functions.join("wt.fish"),
        "# worktrunk shell integration for fish\nfunction wt\n    command wt-old $argv\nend\n",
    )
    .unwrap();

    // Bash alias — SHOULD trigger "not detected" warning
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Some bash config
alias wt="git worktree"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1");

        assert_cmd_snapshot!(cmd);
    });
}

/// When a config uses deprecated variables (repo_root, worktree, main_worktree),
/// the CLI should warn and `wt config update` should apply the variable
/// renames in place.
#[rstest]
fn test_deprecated_template_variables_show_warning(repo: TestRepo, temp_home: TempDir) {
    // Write config with deprecated variables to the test config path
    // (WORKTRUNK_CONFIG_PATH overrides XDG paths in tests)
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        // Use all deprecated variables: repo_root, worktree, main_worktree
        // Note: hooks are at top-level in user config, not in a [hooks] section
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
pre-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#,
    )
    .unwrap();

    // Use `wt list` which loads config through UserConfig::load() and triggers deprecation check
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

        assert_cmd_snapshot!(cmd);
    });

    // `wt list` emits warnings but never writes a .new file — that's
    // `wt config update`'s job. Drive an update explicitly and verify the
    // in-place migration applies all three variable renames.
    let mut cmd = repo.wt_command();
    cmd.args(["config", "update", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
    assert!(cmd.output().unwrap().status.success());

    let migrated = fs::read_to_string(config_path).unwrap();
    assert!(migrated.contains("{{ repo }}"));
    assert!(migrated.contains("{{ repo_path }}"));
    assert!(migrated.contains("{{ worktree_path }}"));
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "config update must not leave a .new file"
    );
}

/// With -v flag, the brief deprecation warning includes the mv command hint
/// and template expansion logs are shown
#[rstest]
fn test_deprecated_template_variables_verbose_shows_content(repo: TestRepo, temp_home: TempDir) {
    // Write config with deprecated variables
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
pre-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["-v", "list"]).current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

        assert_cmd_snapshot!(cmd);
    });
}

/// When a migration file has already been written, subsequent `wt list` runs should:
/// 1. Still show a brief deprecation warning
/// 2. NOT write or overwrite the migration file (skip write since hint is set)
///
/// The file remains available for the user. If they want a fresh one, `wt config show` regenerates.
#[rstest]
fn test_wt_list_never_writes_migration_file(repo: TestRepo, temp_home: TempDir) {
    // Write project config with deprecated variables
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    let original = r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
    fs::write(&project_config_path, original).unwrap();

    // `wt list` should emit a deprecation warning but never write a .new file
    // or modify the config. Materializing migrations is `wt config update`'s job.
    for _ in 0..2 {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "wt list should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert!(
        !project_config_path.with_extension("toml.new").exists(),
        "wt list must not write a .new migration file"
    );
    assert_eq!(
        fs::read_to_string(&project_config_path).unwrap(),
        original,
        "wt list must not modify the config"
    );
}

/// Fixing a deprecated config and later introducing a new one still shows a
/// warning on the new deprecation — no stale state persists across process
/// runs now that `.new` files are gone, so this just exercises the plain
/// per-process warning path.
#[rstest]
fn test_fixing_deprecated_config_then_reintroducing_still_warns(
    repo: TestRepo,
    temp_home: TempDir,
) {
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");

    fs::write(
        &project_config_path,
        r#"pre-start = "ln -sf {{ main_worktree }}/node_modules"
"#,
    )
    .unwrap();
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        assert!(cmd.output().unwrap().status.success());
    }

    fs::write(
        &project_config_path,
        r#"pre-start = "ln -sf {{ repo }}/node_modules"
"#,
    )
    .unwrap();
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("deprecated"),
            "No deprecation warning for clean config"
        );
    }

    fs::write(
        &project_config_path,
        r#"pre-start = "cd {{ worktree }} && npm install"
"#,
    )
    .unwrap();
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("is deprecated"),
            "New deprecation should show warning, got: {stderr}"
        );
    }

    assert!(
        !project_config_path.with_extension("toml.new").exists(),
        "wt list must never write a .new file"
    );
}

/// Deprecation warnings should only appear in the main worktree where the migration
/// file can be applied. Running from a feature worktree should skip the warning entirely.
#[rstest]
fn test_deprecated_project_config_silent_in_feature_worktree(repo: TestRepo, temp_home: TempDir) {
    // Create a feature worktree first (before adding project config)
    {
        let mut cmd = repo.wt_command();
        cmd.args(["switch", "--create", "feature"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "Creating feature worktree should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Get the feature worktree path
    let feature_path = repo.root_path().parent().unwrap().join(format!(
        "{}.feature",
        repo.root_path().file_name().unwrap().to_string_lossy()
    ));

    // Write project config with deprecated variables IN THE FEATURE WORKTREE
    // (project config is loaded from the current worktree root, not the main worktree)
    let feature_config_dir = feature_path.join(".config");
    fs::create_dir_all(&feature_config_dir).unwrap();
    fs::write(
        feature_config_dir.join("wt.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Run wt list from the feature worktree - should NOT show deprecation warning
    // because warn_and_migrate is false for non-main worktrees
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(&feature_path);
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "wt list from feature worktree should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("deprecated template variables"),
            "Deprecation warning should NOT appear in feature worktree, got: {stderr}"
        );
        assert!(
            !stderr.contains("Wrote migrated"),
            "Migration file should NOT be written from feature worktree, got: {stderr}"
        );
    }
}

/// `wt list` emits a user-config deprecation warning but never writes a
/// `.new` file. Materializing migrations is `wt config update`'s job; passive
/// commands stay side-effect-free on disk.
#[rstest]
fn test_user_config_deprecation_warns_without_writing(repo: TestRepo, temp_home: TempDir) {
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    );
    let user_config_path = repo.test_config_path().to_path_buf();
    let original = fs::read_to_string(&user_config_path).unwrap();

    let mut cmd = repo.wt_command();
    cmd.arg("list").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "wt list should succeed: {stderr}");
    assert!(
        stderr.contains("User config:") && stderr.contains("is deprecated"),
        "Should emit user-config deprecation warning, got: {stderr}"
    );
    assert!(
        !user_config_path.with_extension("toml.new").exists(),
        "wt list must not write a .new migration file"
    );
    assert_eq!(
        fs::read_to_string(&user_config_path).unwrap(),
        original,
        "wt list must not modify user config"
    );
}

#[rstest]
fn test_config_show_shell_integration_active(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    // Create a temp file for the directive file
    let directive_file = temp_home.path().join("directive");
    fs::write(&directive_file, "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        // Set WORKTRUNK_DIRECTIVE_FILE to simulate shell integration being active
        cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive_file);

        assert_cmd_snapshot!(cmd);
    });
}

/// When shell integration is active at runtime (WORKTRUNK_DIRECTIVE_FILE set) but the
/// init line is NOT in the scanned config file (e.g., sourced from another file), config
/// show should report "Configured ... (not found in ...)" instead of "Not configured".
/// Regression test for https://github.com/max-sixty/worktrunk/issues/1306
#[rstest]
fn test_config_show_shell_active_but_not_in_config_file(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n",
    )
    .unwrap();

    // Create ~/.zshrc WITHOUT the init line (simulates it being in a sourced file)
    fs::write(temp_home.path().join(".zshrc"), "# my zsh config\n").unwrap();

    let directive_file = temp_home.path().join("directive");
    fs::write(&directive_file, "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive_file);
        // Set SHELL to zsh so current_shell() returns Some(Zsh)
        cmd.env("SHELL", "/bin/zsh");

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_plugin_installed(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock claude CLI and plugin as installed
    repo.setup_mock_claude_installed();
    TestRepo::setup_plugin_installed(temp_home.path());

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_claude_available_plugin_not_installed(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock claude as available (but plugin not installed)
    repo.setup_mock_claude_installed();

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_statusline_configured(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock claude CLI, plugin, AND statusline
    repo.setup_mock_claude_installed();
    TestRepo::setup_plugin_installed(temp_home.path());
    TestRepo::setup_statusline_configured(temp_home.path());

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_codex_available(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock codex as available
    repo.setup_mock_codex_installed();

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_opencode_available_plugin_not_installed(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock opencode as available (but plugin not installed)
    repo.setup_mock_opencode_installed();

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_opencode_plugin_installed(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock opencode CLI and plugin as installed
    repo.setup_mock_opencode_installed();
    TestRepo::setup_opencode_plugin_installed(temp_home.path());

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_opencode_plugin_outdated(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock opencode CLI as installed
    repo.setup_mock_opencode_installed();

    // Write an outdated plugin file (different content from embedded source)
    let plugins_dir = temp_home.path().join("opencode-config/plugins");
    fs::create_dir_all(&plugins_dir).unwrap();
    fs::write(
        plugins_dir.join("worktrunk.ts"),
        "// outdated plugin content\n",
    )
    .unwrap();

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_gemini_available_extension_not_installed(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock gemini as available (but extension not installed)
    repo.setup_mock_gemini_installed();

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_gemini_extension_invalid_manifest(mut repo: TestRepo, temp_home: TempDir) {
    // A malformed gemini-extension.json should fall through the JSON-parse
    // branch and report the extension as not installed (the install hint).
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_gemini_installed();

    let extension_dir = temp_home.path().join(".gemini/extensions/worktrunk");
    fs::create_dir_all(&extension_dir).unwrap();
    fs::write(extension_dir.join("gemini-extension.json"), "not json\n").unwrap();

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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_config_show_gemini_extension_installed(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();
    // Setup mock gemini CLI and extension as installed
    repo.setup_mock_gemini_installed();
    TestRepo::setup_gemini_extension_installed(temp_home.path());

    // Create global config
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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Each `is_*_available()` short-circuits on the `WORKTRUNK_TEST_*_INSTALLED`
/// override the harness sets, so the production `which::which` PATH lookup is
/// otherwise never exercised. `setup_mock_clis_on_path()` drops the overrides
/// and prepends real mock executables, so this single run covers the
/// PATH-detection path for all four AI CLIs at once.
#[rstest]
fn test_config_show_clis_detected_via_path(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_clis_on_path();

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
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

// =============================================================================
// OpenCode plugin install/uninstall
// =============================================================================

/// Fresh install writes the plugin to the expected path.
#[rstest]
fn test_opencode_install_creates_plugin(temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugin_path = canonical_home.join("opencode-config/plugins/worktrunk.ts");
    assert!(
        plugin_path.exists(),
        "Plugin file should exist after install"
    );
    let content = fs::read_to_string(&plugin_path).unwrap();
    assert!(
        content.contains("session.status"),
        "Plugin should contain event handler"
    );
}

/// When the plugin is already installed with current content, show info message.
#[rstest]
fn test_opencode_install_already_installed(temp_home: TempDir) {
    // Pre-install the plugin
    TestRepo::setup_opencode_plugin_installed(temp_home.path());

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });
}

/// When an outdated plugin exists, install replaces it with current content.
#[rstest]
fn test_opencode_install_updates_outdated(temp_home: TempDir) {
    // Write an outdated plugin file
    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugins_dir = canonical_home.join("opencode-config/plugins");
    fs::create_dir_all(&plugins_dir).unwrap();
    fs::write(plugins_dir.join("worktrunk.ts"), "// outdated\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    // Verify content was updated
    let content = fs::read_to_string(plugins_dir.join("worktrunk.ts")).unwrap();
    assert!(
        content.contains("session.status"),
        "Plugin should be updated to current content"
    );
}

/// Uninstall removes the plugin file.
#[rstest]
fn test_opencode_uninstall_removes_plugin(temp_home: TempDir) {
    // Pre-install the plugin
    TestRepo::setup_opencode_plugin_installed(temp_home.path());
    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugin_path = canonical_home.join("opencode-config/plugins/worktrunk.ts");
    assert!(plugin_path.exists(), "Plugin should exist before uninstall");

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.args(["config", "plugins", "opencode", "uninstall", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    assert!(
        !plugin_path.exists(),
        "Plugin file should be removed after uninstall"
    );
}

/// Uninstall when not installed shows info message.
#[rstest]
fn test_opencode_uninstall_not_installed(temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.args(["config", "plugins", "opencode", "uninstall", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });
}

/// Install honours `XDG_CONFIG_HOME` when `OPENCODE_CONFIG_DIR` is unset.
///
/// Cross-platform: OpenCode's documented global-config location is
/// `~/.config/opencode/` (or `$XDG_CONFIG_HOME/opencode/`) on every OS, so the
/// plugin must land at `{temp_home}/.config/opencode/plugins/worktrunk.ts`
/// regardless of platform. Previously this test was Linux-only because the
/// implementation called `dirs::config_dir()`, which on macOS returns
/// `~/Library/Application Support` (the *managed* settings path, not the
/// plugin path) — see #2654.
#[rstest]
fn test_opencode_install_uses_xdg_config_home(temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        // Remove OPENCODE_CONFIG_DIR so the code falls through to XDG_CONFIG_HOME
        cmd.env_remove("OPENCODE_CONFIG_DIR");
        cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    // set_temp_home_env sets XDG_CONFIG_HOME = $HOME/.config
    let plugin_path = canonical_home.join(".config/opencode/plugins/worktrunk.ts");
    assert!(
        plugin_path.exists(),
        "Plugin file should exist at XDG_CONFIG_HOME fallback path: {}",
        plugin_path.display()
    );
}

/// Install defaults to `$HOME/.config/opencode/plugins/` when neither
/// `OPENCODE_CONFIG_DIR` nor `XDG_CONFIG_HOME` is set.
///
/// Cross-platform regression guard for #2654: with the previous
/// `dirs::config_dir()`-based fallback, this test passed on Linux (XDG default)
/// but failed on macOS (`~/Library/Application Support`). After the fix, the
/// fallback is `$HOME/.config/opencode/plugins/worktrunk.ts` on every OS, in
/// line with OpenCode's own global-config precedence.
#[rstest]
fn test_opencode_install_defaults_to_home_dot_config(temp_home: TempDir) {
    let mut cmd = wt_command();
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env_remove("OPENCODE_CONFIG_DIR");
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

    let output = cmd.output().expect("install command should run");
    assert!(
        output.status.success(),
        "install failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugin_path = canonical_home.join(".config/opencode/plugins/worktrunk.ts");
    assert!(
        plugin_path.exists(),
        "Plugin should default to $HOME/.config/opencode/plugins/worktrunk.ts but file not found at: {}",
        plugin_path.display(),
    );
}

/// Install prompt declined (no `--yes`, piped stdin → empty → declined).
/// Exercises the `return Ok(())` branch at lines 83-84 of opencode.rs.
#[rstest]
fn test_opencode_install_prompt_declined(temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        // No --yes, piped stdin sends empty → prompt declines
        cmd.args(["config", "plugins", "opencode", "install"]);

        assert_cmd_snapshot!(cmd);
    });

    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugin_path = canonical_home.join("opencode-config/plugins/worktrunk.ts");
    assert!(
        !plugin_path.exists(),
        "Plugin should NOT be installed when prompt is declined"
    );
}

/// Uninstall prompt declined (no `--yes`, piped stdin → empty → declined).
/// Exercises the `return Ok(())` branch at lines 129-130 of opencode.rs.
#[rstest]
fn test_opencode_uninstall_prompt_declined(temp_home: TempDir) {
    // Pre-install the plugin so we reach the prompt
    TestRepo::setup_opencode_plugin_installed(temp_home.path());
    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    let plugin_path = canonical_home.join("opencode-config/plugins/worktrunk.ts");
    assert!(plugin_path.exists(), "Plugin should exist before test");

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        // No --yes, piped stdin sends empty → prompt declines
        cmd.args(["config", "plugins", "opencode", "uninstall"]);

        assert_cmd_snapshot!(cmd);
    });

    assert!(
        plugin_path.exists(),
        "Plugin should still exist when uninstall prompt is declined"
    );
}

/// When $SHELL is not set but PSModulePath is, config show should display
/// "Detected shell: powershell" in the diagnostics and show the verification hint.
#[rstest]
fn test_config_show_powershell_detected_via_psmodulepath(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .bashrc with wt integration
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init bash)"; fi
"#,
    )
    .unwrap();

    // Create PowerShell profile with wt integration (covers Get-Command hint branch)
    // Must use the canonical config line (what `wt config shell install` writes)
    let ps_profile_dir = temp_home.path().join(".config").join("powershell");
    fs::create_dir_all(&ps_profile_dir).unwrap();
    fs::write(
        ps_profile_dir.join("Microsoft.PowerShell_profile.ps1"),
        "if (Get-Command wt -ErrorAction SilentlyContinue) { Invoke-Expression (& wt config shell init powershell | Out-String) }\n",
    )
    .unwrap();

    let mut settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    // PowerShell config state is platform-dependent: the profile path differs between
    // Windows (Documents\PowerShell\) and Unix (~/.config/powershell/). The broad
    // PowerShell filter strips status lines, but the Get-Command hint and "To configure"
    // hint also vary by platform (present only when profile is found). Filter them too.
    settings.add_filter(r"(?m)^.*Get-Command.*\n", "");
    settings.add_filter(r"(?m)^.*To configure, run.*\n", "");
    // Collapse triple newlines that may result from stripping adjacent lines
    settings.add_filter(r"\n\n\n", "\n\n");
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());
        // Enable PowerShell scanning so the profile above is detected
        cmd.env("WORKTRUNK_TEST_POWERSHELL_ENV", "1");
        // Ensure SHELL is NOT set (already removed by configure_cli_command)
        cmd.env_remove("SHELL");
        // Set PSModulePath to trigger PowerShell detection fallback
        cmd.env(
            "PSModulePath",
            r"C:\Users\user\Documents\PowerShell\Modules",
        );

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that deprecated [commit-generation] section shows warning and creates migration file
#[rstest]
fn test_deprecated_commit_generation_section_shows_warning(repo: TestRepo, temp_home: TempDir) {
    // Write user config with deprecated [commit-generation] section
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#,
    )
    .unwrap();

    // Use `wt list` which loads config through UserConfig::load() and triggers deprecation check
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

        assert_cmd_snapshot!(cmd);
    });

    // Drive the migration explicitly via `wt config update`; `wt list` only warns.
    let mut cmd = repo.wt_command();
    cmd.args(["config", "update", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
    assert!(cmd.output().unwrap().status.success());

    let migrated = fs::read_to_string(config_path).unwrap();
    assert!(
        migrated.contains("[commit.generation]"),
        "Should rename [commit-generation] to [commit.generation]"
    );
    assert!(
        migrated.contains("command = \"llm -m haiku\""),
        "Should merge args into command"
    );
    assert!(!migrated.contains("[commit-generation]"));
    assert!(!migrated.contains("args ="));
}

/// Test that deprecated project-level [projects."...".commit-generation] shows warning
#[rstest]
fn test_deprecated_commit_generation_project_level_shows_warning(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // Write user config with deprecated project-level commit-generation
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/example/repo".commit-generation]
command = "llm -m gpt-4"
"#,
    )
    .unwrap();

    // Use `wt list` which loads config through UserConfig::load() and triggers deprecation check
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

        assert_cmd_snapshot!(cmd);
    });

    // Drive the migration explicitly via `wt config update`; `wt list` only warns.
    let mut cmd = repo.wt_command();
    cmd.args(["config", "update", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
    assert!(cmd.output().unwrap().status.success());

    let migrated = fs::read_to_string(config_path).unwrap();
    assert!(
        migrated.contains("[projects.\"github.com/example/repo\".commit.generation]"),
        "Should rename project-level section"
    );
}

/// Test that `wt config show` displays full deprecation details including inline diff
#[rstest]
fn test_config_show_displays_deprecation_details(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Write user config with deprecated variables at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
pre-start = "ln -sf {{ repo_root }}/node_modules"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });

    // `wt config show` renders the diff in memory — nothing persists on disk.
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "wt config show must not leave a .new file behind"
    );
}

/// Test that `wt config show` from linked worktree shows hint to run from main worktree
///
/// When project config has deprecations and you run from a linked worktree, it should
/// show a hint to run `wt config show` from the main worktree.
#[rstest]
fn test_config_show_from_linked_worktree_shows_main_worktree_hint(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    // Setup mock gh/glab/claude for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();

    // Write project config with deprecated variables
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    fs::write(
        project_config_dir.join("wt.toml"),
        r#"pre-start = "ln -sf {{ main_worktree }}/node_modules"
"#,
    )
    .unwrap();
    repo.commit("Add deprecated project config");

    // Create a linked worktree using git directly
    let feature_path = repo.root_path().parent().unwrap().join("feature-test");
    repo.run_git(&[
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature-test",
    ]);

    // Run wt config show from the linked worktree
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(&feature_path);
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that `wt config show` displays project-level commit-generation deprecations
#[rstest]
fn test_config_show_displays_project_commit_generation_deprecations(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    repo.setup_mock_ci_tools_unauthenticated();

    // Write user config with deprecated project-level commit-generation
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/example/repo".commit-generation]
command = "llm -m gpt-4"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });

    // `wt config show` renders the diff in memory — nothing persists on disk.
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "wt config show must not leave a .new file behind"
    );
}

/// Test that deprecated approved-commands in [projects] sections are copied to approvals.toml
#[rstest]
fn test_config_update_copies_approved_commands_to_approvals_file(
    repo: TestRepo,
    temp_home: TempDir,
) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#,
    )
    .unwrap();

    // Passive load must NOT copy approvals or modify the config.
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
        assert!(cmd.output().unwrap().status.success());
    }
    assert!(
        !config_path.with_file_name("approvals.toml").exists(),
        "wt list must not copy approvals"
    );
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "wt list must not write .new file"
    );

    // `wt config update --yes` migrates in place: config.toml is rewritten
    // without approved-commands, and approvals.toml is created alongside it.
    let mut cmd = repo.wt_command();
    cmd.args(["config", "update", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "config update should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let migrated = fs::read_to_string(config_path).unwrap();
    assert!(
        !migrated.contains("approved-commands"),
        "config.toml should no longer contain approved-commands: {migrated}"
    );
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "config update must not leave a .new file behind"
    );

    let approvals_file = config_path.with_file_name("approvals.toml");
    assert!(approvals_file.exists(), "approvals.toml should be created");
    let approvals = fs::read_to_string(&approvals_file).unwrap();
    assert!(
        approvals.contains("npm install") && approvals.contains("npm test"),
        "approvals.toml should carry both commands: {approvals}"
    );
}

// ==================== config update tests ====================

/// `wt config update` migrates project config in place (from the main
/// worktree). Covers the project-config path in `check_project_config`.
#[rstest]
fn test_config_update_applies_project_config_migration(repo: TestRepo) {
    repo.write_project_config(
        r#"pre-start = "ln -sf {{ main_worktree }}/node_modules"
"#,
    );
    repo.commit("Add deprecated project config");
    let project_config_path = repo.root_path().join(".config").join("wt.toml");

    let output = repo
        .wt_command()
        .args(["config", "update", "--yes"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "config update should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let updated = fs::read_to_string(&project_config_path).unwrap();
    assert!(updated.contains("pre-start"));
    assert!(updated.contains("{{ repo }}"));
    assert!(!updated.contains("main_worktree"));
}

/// `wt config update` with a clean project config (no deprecations) treats
/// the repo as nothing-to-do — covers the project-config path through
/// `check_and_migrate` when it returns `info == None`.
#[rstest]
fn test_config_update_clean_project_config_is_noop(repo: TestRepo) {
    repo.write_project_config(
        r#"pre-start = "echo ready"
"#,
    );
    repo.commit("Add clean project config");

    let output = repo
        .wt_command()
        .args(["config", "update"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No deprecated settings found"),
        "Expected no-op message, got: {stderr}"
    );
}

/// `wt config update` from a linked worktree declines to mutate project
/// config and instead points at the main worktree. Covers the `is_linked`
/// branch in `check_project_config`.
#[rstest]
fn test_config_update_project_config_from_linked_worktree_shows_hint(repo: TestRepo) {
    repo.write_project_config(
        r#"pre-start = "ln -sf {{ main_worktree }}/node_modules"
"#,
    );
    repo.commit("Add deprecated project config");
    let project_config_path = repo.root_path().join(".config").join("wt.toml");
    let before = fs::read_to_string(&project_config_path).unwrap();

    let feature_path = repo.root_path().parent().unwrap().join("feature-test");
    repo.run_git(&[
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature-test",
    ]);

    let output = repo
        .wt_command()
        .args(["config", "update", "--yes"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("To update project config:"),
        "Should hint at main worktree, got: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(&project_config_path).unwrap(),
        before,
        "Project config must not change when run from linked worktree"
    );
}

/// `wt config update --print` with both user- and project-config deprecations
/// emits both, separated by labeled headers on stdout.
#[rstest]
fn test_config_update_print_emits_both_configs(repo: TestRepo) {
    let user_config_path = repo.test_config_path();
    fs::write(
        user_config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();
    repo.write_project_config(
        r#"pre-start = "ln -sf {{ main_worktree }}/node_modules"
"#,
    );
    repo.commit("Add deprecated project config");

    let output = repo
        .wt_command()
        .args(["config", "update", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# User config"));
    assert!(stdout.contains("# Project config"));
    assert!(stdout.contains("{{ repo }}"));
    assert!(stdout.contains("pre-start"));
}

/// `wt config update --print` on a clean config exits silently with empty
/// stdout — no "nothing to do" noise to corrupt a pipe.
#[rstest]
fn test_config_update_print_on_clean_config_is_silent(repo: TestRepo) {
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["config", "update", "--print"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "stdout must be empty on clean config"
    );
}

/// `wt config update --print` emits the migrated TOML to stdout without
/// touching the config file. Stderr stays empty so the output is pipeable.
#[rstest]
fn test_config_update_print_emits_migrated_without_writing(repo: TestRepo) {
    let config_path = repo.test_config_path();
    let original = r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
    fs::write(config_path, original).unwrap();

    let output = repo
        .wt_command()
        .args(["config", "update", "--print"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "config update --print should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "--print must keep stderr empty for pipe-friendliness, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("{{ repo }}") && !stdout.contains("{{ main_worktree }}"),
        "stdout should contain migrated content, got: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(config_path).unwrap(),
        original,
        "--print must not modify the config file"
    );
    assert!(
        !config_path.with_extension("toml.new").exists(),
        "--print must not write a .new file"
    );
}

/// `wt config update` with no deprecated settings reports nothing to do
#[rstest]
fn test_config_update_no_deprecations(repo: TestRepo) {
    // Write a clean config with no deprecated patterns
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        cmd.args(["config", "update", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config update --yes` applies template variable migration
#[rstest]
fn test_config_update_applies_template_var_migration(repo: TestRepo) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
pre-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        cmd.args(["config", "update", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    // Config file should now contain the updated variables
    let updated = fs::read_to_string(config_path).unwrap();
    assert!(
        updated.contains("{{ repo }}"),
        "Should replace main_worktree with repo"
    );
    assert!(
        updated.contains("{{ repo_path }}"),
        "Should replace repo_root with repo_path"
    );
    assert!(
        updated.contains("{{ worktree_path }}"),
        "Should replace worktree with worktree_path"
    );

    // Migration .new file should be gone (renamed over original)
    assert!(
        !config_path.with_extension("toml.new").exists(),
        ".new file should be consumed by the update"
    );
}

/// `wt config update` migrates the deprecated `commits` squash-template
/// variable to `commit_details` (see #2984). The rewrite is a plain identifier
/// rename — the loop variable stays bare — because each `commit_details`
/// element renders as its subject, so the migrated template renders identically
/// to the original.
#[rstest]
fn test_config_update_migrates_commits_squash_var(repo: TestRepo) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"[commit.generation]
squash-template = """
Combine {{ commits | length }} commits:
{% for c in commits %}- {{ c }}
{% endfor %}"""
"#,
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["config", "update", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let updated = fs::read_to_string(config_path).unwrap();
    assert!(
        updated.contains("{{ commit_details | length }}"),
        "filter use of commits should migrate: {updated}"
    );
    assert!(
        updated.contains("for c in commit_details"),
        "loop source should migrate: {updated}"
    );
    // The loop variable stays bare — no `.subject` is injected, since the
    // element renders as its subject on its own.
    assert!(
        updated.contains("- {{ c }}"),
        "loop body should be left untouched: {updated}"
    );
    assert!(
        !updated.contains("{{ commits"),
        "no deprecated commits reference should remain: {updated}"
    );
}

/// `wt config show` displays deprecation details for `[select]` → `[switch.picker]`.
/// Uses user config so the warning label reads "User config".
#[rstest]
fn test_config_show_displays_select_section_deprecation(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"[select]
pager = "delta --paging=never"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config show` displays deprecation details for `[merge] no-ff` → `ff` (inverted).
#[rstest]
fn test_config_show_displays_no_ff_deprecation(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"[merge]
no-ff = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config show` displays deprecation details for `[switch] no-cd` → `cd` (inverted).
#[rstest]
fn test_config_show_displays_no_cd_deprecation(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    let config_path = global_config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"[switch]
no-cd = true
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config update --yes` applies commit-generation section rename
#[rstest]
fn test_config_update_applies_commit_generation_migration(repo: TestRepo) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        cmd.args(["config", "update", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    // Config file should have the renamed section and merged args
    let updated = fs::read_to_string(config_path).unwrap();
    assert!(
        updated.contains("[commit.generation]"),
        "Should rename section"
    );
    assert!(
        updated.contains("command = \"llm -m haiku\""),
        "Should merge args into command"
    );
    assert!(
        !updated.contains("[commit-generation]"),
        "Old section name should be gone"
    );
    assert!(!updated.contains("args ="), "Args field should be removed");
}

/// `wt config update --yes` handles approved-commands migration
#[rstest]
fn test_config_update_applies_approved_commands_migration(repo: TestRepo) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        cmd.args(["config", "update", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    // Config should no longer have approved-commands
    let updated = fs::read_to_string(config_path).unwrap();
    assert!(
        !updated.contains("approved-commands"),
        "approved-commands should be removed from config"
    );

    // Approvals should be in approvals.toml
    let approvals_file = config_path.with_file_name("approvals.toml");
    assert!(approvals_file.exists(), "approvals.toml should exist");
    let approvals = fs::read_to_string(&approvals_file).unwrap();
    assert!(approvals.contains("npm install"));
    assert!(approvals.contains("npm test"));
}

/// Test that explicitly specified --config path that doesn't exist shows a warning
#[rstest]
fn test_explicit_config_path_not_found_shows_warning(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("--config")
            .arg("/nonexistent/worktrunk/config.toml")
            .arg("list")
            .current_dir(repo.root_path());

        // Should show warning about missing config file but still succeed
        assert_cmd_snapshot!(cmd);
    });
}

// ==================== Plugin Install/Uninstall Tests ====================

#[rstest]
fn test_plugins_claude_install(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_install_invalid_plugins_json(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins();

    // Write invalid JSON to the plugins file — is_plugin_installed() should
    // treat this as "not installed" and the install command should proceed
    let plugins_dir = temp_home.path().join(".claude/plugins");
    fs::create_dir_all(&plugins_dir).unwrap();
    fs::write(plugins_dir.join("installed_plugins.json"), "not valid json").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_install_already_installed(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins();
    TestRepo::setup_plugin_installed(temp_home.path());

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_install_claude_not_found(repo: TestRepo) {
    // Don't call setup_mock_claude_installed — claude CLI not available
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_uninstall(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins();
    TestRepo::setup_plugin_installed(temp_home.path());

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "uninstall", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_uninstall_not_installed(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins();
    // Don't setup plugin as installed

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "uninstall", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

// ==================== Codex Plugin Install/Uninstall Tests ====================

#[rstest]
fn test_plugins_codex_install(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_codex_with_plugins();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "codex", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_codex_install_codex_not_found(repo: TestRepo) {
    // Don't call setup_mock_codex_installed — codex CLI not available
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "codex", "install", "--yes"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_codex_uninstall(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_codex_with_plugins();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "codex", "uninstall", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_codex_install_command_fails(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_codex_with_plugins_failing();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "codex", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[test]
fn test_codex_plugin_metadata_is_valid_json() {
    let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project_root.join("plugins/worktrunk/.codex-plugin/plugin.json"))
            .unwrap(),
    )
    .unwrap();
    // Codex discovers a marketplace's plugin list strictly from
    // <repo-root>/.agents/plugins/marketplace.json (no fallback to the Claude
    // .claude-plugin/marketplace.json). Each plugin's `source` must be an
    // object pointing at a non-empty subdirectory — codex rejects a bare or
    // root-relative source ("local plugin source path must not be empty"), so
    // the plugin lives in plugins/worktrunk/ rather than at the repo root.
    // Verified end-to-end against codex-cli 0.130.0: with this layout the
    // worktrunk marketplace surfaces in /plugins and the plugin installs;
    // without it the marketplace registers but enumerates zero plugins.
    let marketplace: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project_root.join(".agents/plugins/marketplace.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(plugin["name"], "worktrunk");
    assert_eq!(plugin["skills"], "./skills/");
    // Metadata must not drift back toward the Claude plugin: the Codex plugin
    // ships only the configuration skill, and its URLs are the canonical site
    // (not the `/claude-code/` doc slug).
    for key in ["description", "homepage"] {
        let val = plugin[key].as_str().unwrap();
        assert!(
            !val.contains("activity") && !val.contains("claude-code"),
            "plugin.json `{key}` regressed toward Claude/activity wording: {val}"
        );
    }
    let iface = &plugin["interface"];
    assert!(
        !iface["longDescription"]
            .as_str()
            .unwrap()
            .contains("activity"),
        "plugin.json interface.longDescription must not claim activity tracking"
    );
    assert!(
        !iface["websiteURL"]
            .as_str()
            .unwrap()
            .contains("claude-code"),
        "plugin.json interface.websiteURL must point at the canonical site"
    );
    // The Codex plugin ships no activity-marker hooks: Codex's
    // HookEventNameWire vocabulary (codex-cli 0.130.0) has no `Stop`/turn-end
    // event, so a 🤖 set on UserPromptSubmit could never return to 💬 within a
    // session. Keep the Codex manifest free of a `hooks` key, and its wrapper
    // dir manifest-only, until Codex adds a turn-end hook event — see CLAUDE.md
    // → "Plugin Layout". (plugins/worktrunk/hooks/ exists post-consolidation,
    // but it is the *Claude* plugin's — Codex's manifest never references it.)
    assert_eq!(plugin.get("hooks"), None);
    assert!(
        !project_root
            .join("plugins/worktrunk/.codex-plugin/hooks")
            .exists(),
        "the Codex wrapper dir must hold only plugin.json"
    );
    assert_eq!(marketplace["plugins"][0]["name"], "worktrunk");
    // Source is a non-empty subdir object; a bare "./" is rejected by codex.
    assert_eq!(marketplace["plugins"][0]["source"]["source"], "local");
    assert_eq!(
        marketplace["plugins"][0]["source"]["path"],
        "./plugins/worktrunk"
    );
    assert_eq!(
        marketplace["plugins"][0]["policy"]["installation"],
        "AVAILABLE"
    );
    // Codex validates `interface` is an object and requires `displayName`;
    // omitting it is what made the plugin undiscoverable in /plugins.
    assert_eq!(marketplace["interface"]["displayName"], "Worktrunk");
}

/// Claude Code + Codex share `plugins/worktrunk/`; Gemini's manifest is a
/// third loader-mandated repo-root pointer (`gemini-extension.json` +
/// `hooks/hooks.json` — Gemini hard-probes `${extensionPath}/{hooks,skills}/`
/// at the extension root with no path indirection). Verified end-to-end
/// against the real CLIs: Claude (claude-cli 2.1.x) wants its manifest at the
/// plugin root with NO `.claude-plugin/` wrapper (`source:
/// "./plugins/worktrunk"` + `<subdir>/.claude-plugin/` fails "Plugin not
/// found"); Gemini (gemini-cli 0.42) resolves the extension at the repo root,
/// so `${extensionPath}/skills/` is the real single-sourced repo-root
/// `skills/` and its hooks call the canonical
/// `${extensionPath}/plugins/worktrunk/hooks/wt.sh` — no symlink, no bundled
/// copy, and `gemini extensions install owner/repo` works natively.
///
/// Duplicated strings can't be `include!`d into JSON, so this test is the
/// drift guard: the Claude marketplace/manifest descriptions stay
/// byte-identical, every product description shares the canonical opening
/// sentence, and every repo-root skill is listed in the Claude manifest
/// (Claude has no skill auto-discovery — an unlisted skill is silently
/// dropped).
#[test]
fn test_plugin_layout_is_consolidated() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |p: &str| fs::read_to_string(root.join(p)).unwrap();
    let json = |p: &str| serde_json::from_str::<serde_json::Value>(&read(p)).unwrap();

    // Repo root keeps ONLY the two loader-mandated marketplace pointers.
    assert!(
        !root.join(".claude-plugin/plugin.json").exists()
            && !root.join(".claude-plugin/hooks").exists(),
        ".claude-plugin/ at the repo root must hold only marketplace.json"
    );
    let claude_mkt = json(".claude-plugin/marketplace.json");
    assert_eq!(claude_mkt["plugins"][0]["source"], "./plugins/worktrunk");

    // Claude manifest at the plugin root (no wrapper); hooks relative to it.
    let claude = json("plugins/worktrunk/plugin.json");
    assert_eq!(claude["hooks"], "./hooks/hooks.json");
    assert!(
        root.join("plugins/worktrunk/hooks/hooks.json").exists()
            && root.join("plugins/worktrunk/hooks/wt.sh").exists(),
        "Claude hooks must live at the plugin root's hooks/"
    );
    assert!(
        !read("plugins/worktrunk/hooks/hooks.json").contains(".claude-plugin/hooks/"),
        "hooks.json must reference $CLAUDE_PLUGIN_ROOT/hooks/wt.sh, not the old wrapper path"
    );

    // The description is duplicated across the Claude marketplace pointer and
    // the Claude manifest — they must stay byte-identical.
    assert_eq!(
        claude_mkt["plugins"][0]["description"], claude["description"],
        ".claude-plugin/marketplace.json and plugins/worktrunk/plugin.json descriptions drifted"
    );

    // Gemini extension: manifest + hooks are loader-mandated repo-root
    // pointers (Gemini hard-probes ${extensionPath}/{hooks,skills}/ at the
    // extension root). Relocating to the root — instead of a plugins/gemini/
    // sibling — is what makes `gemini extensions install owner/repo` work and
    // lets the extension reuse the real repo-root skills/ + the canonical
    // worktrunk shim instead of a symlink and a bundled copy.
    assert!(
        !root.join("plugins/gemini").exists(),
        "Gemini extension was relocated to the repo root; plugins/gemini/ must not exist"
    );
    let gemini = json("gemini-extension.json");
    assert_eq!(gemini["name"], "worktrunk");
    assert!(
        gemini["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "gemini-extension.json needs a description (shown in `gemini extensions list`)"
    );
    // Gemini reuses the single-sourced repo-root skills/ (a real dir that
    // survives `gemini extensions install`'s copy) — not a symlink or bundle.
    assert!(
        root.join("skills").is_dir() && !root.join("skills").is_symlink(),
        "repo-root skills/ must be a real directory Gemini resolves via ${{extensionPath}}/skills"
    );
    // Gemini hooks call the canonical worktrunk shim by its real path — no
    // bundled ${extensionPath}/hooks/wt.sh, no cross-dir `../`, no old wrapper.
    let gemini_hooks = read("hooks/hooks.json");
    assert!(
        gemini_hooks.contains("${extensionPath}/plugins/worktrunk/hooks/wt.sh")
            && !gemini_hooks.contains("${extensionPath}/hooks/wt.sh")
            && !gemini_hooks.contains("../")
            && !gemini_hooks.contains(".claude-plugin/"),
        "Gemini hooks must call the canonical ${{extensionPath}}/plugins/worktrunk/hooks/wt.sh"
    );
    assert!(
        !root.join("hooks/wt.sh").exists(),
        "no bundled Gemini wt.sh — hooks reference the canonical worktrunk shim"
    );

    // Claude has no skill auto-discovery, so every repo-root skill dir MUST be
    // listed explicitly in plugin.json — otherwise a new skill is silently
    // invisible to Claude while Codex/Gemini (whole-dir) pick it up.
    let listed: std::collections::BTreeSet<&str> = claude["skills"]
        .as_array()
        .expect("plugin.json `skills` must be an array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for entry in fs::read_dir(root.join("skills")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let listed_form = format!("./skills/{}", entry.file_name().to_string_lossy());
            assert!(
                listed.contains(listed_form.as_str()),
                "repo-root skill {listed_form} is not in plugins/worktrunk/plugin.json \
                 `skills` (Claude has no auto-discovery — add it or it is silently dropped)"
            );
        }
    }

    // The WorktreeRemove hook must not force-delete unmerged branches (#2939).
    // Claude Code auto-fires WorktreeRemove on session exit for any worktree
    // with a clean working tree, so a hook command containing `-D` /
    // `--force-delete` silently discards committed-but-unpushed work — the only
    // recovery path is `git fsck`. The safe default retains unmerged branches
    // and prints a `wt remove -D <branch>` hint for the user to act on.
    let hooks = read("plugins/worktrunk/hooks/hooks.json");
    let hooks_json = json("plugins/worktrunk/hooks/hooks.json");

    // Claude hands each hook command to the user's LOGIN shell before `bash`
    // launches, so the outer command line must parse under fish/zsh/bash. fish
    // rejects bash brace syntax ("Variables cannot be bracketed"), so the plugin
    // root must be referenced as `$CLAUDE_PLUGIN_ROOT`, never `${CLAUDE_PLUGIN_ROOT}`.
    let all_commands: Vec<&str> = hooks_json["hooks"]
        .as_object()
        .expect("hooks.json must have a `hooks` object")
        .values()
        .flat_map(|event| event.as_array().expect("each hook event must be an array"))
        .flat_map(|group| {
            group["hooks"]
                .as_array()
                .expect("each hook group must have a `hooks` array")
        })
        .map(|hook| {
            hook["command"]
                .as_str()
                .expect("each hook must define a command")
        })
        .collect();
    for cmd in &all_commands {
        assert!(
            cmd.contains("$CLAUDE_PLUGIN_ROOT") && !cmd.contains("${CLAUDE_PLUGIN_ROOT}"),
            "hook command must use unbraced $CLAUDE_PLUGIN_ROOT (braces break fish \
             login-shell parsing: \"fish: Variables cannot be bracketed\"). command:\n{cmd}"
        );
    }

    let worktree_remove_cmd = hooks_json["hooks"]["WorktreeRemove"][0]["hooks"][0]["command"]
        .as_str()
        .expect("WorktreeRemove hook must define a command");
    assert!(
        !worktree_remove_cmd.contains(" -D") && !worktree_remove_cmd.contains("--force-delete"),
        "WorktreeRemove hook must not pass -D / --force-delete (silently destroys \
         committed-but-unpushed work on session-exit auto-remove; see #2939). \
         hooks.json:\n{hooks}"
    );

    // The product description must not drift across tools. Byte-identical is
    // schema-impossible (Codex omits the activity clause, Gemini says
    // "extension"), but every manifest shares the canonical opening sentence.
    const STEM: &str =
        "Worktrunk is a CLI for Git worktree management, designed for parallel AI agent workflows.";
    let codex = json("plugins/worktrunk/.codex-plugin/plugin.json");
    for (label, val) in [
        ("plugins/worktrunk/plugin.json", &claude["description"]),
        (
            ".claude-plugin/marketplace.json",
            &claude_mkt["plugins"][0]["description"],
        ),
        (
            "plugins/worktrunk/.codex-plugin/plugin.json",
            &codex["description"],
        ),
        ("gemini-extension.json", &gemini["description"]),
    ] {
        let val = val.as_str().unwrap();
        assert!(
            val.starts_with(STEM),
            "{label} description must start with the canonical product sentence; got: {val}"
        );
    }
}

/// Claude hands each hook `command` to the user's LOGIN shell, which parses the
/// whole line before the leading `bash …` ever launches. The command must
/// therefore parse cleanly under fish, zsh, and bash — fish in particular
/// rejects bash brace syntax (`${VAR}` → "Variables cannot be bracketed"). This
/// syntax-checks every Claude hook command under all three shells with their
/// no-execute flags, so a reintroduced brace (or any other non-portable
/// construct) fails here instead of silently breaking fish users at runtime.
#[cfg(all(unix, feature = "shell-integration-tests"))]
#[test]
fn test_claude_hook_commands_parse_in_all_shells() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let hooks_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("plugins/worktrunk/hooks/hooks.json")).unwrap(),
    )
    .unwrap();

    let commands: Vec<&str> = hooks_json["hooks"]
        .as_object()
        .expect("hooks.json must have a `hooks` object")
        .values()
        .flat_map(|event| event.as_array().expect("each hook event must be an array"))
        .flat_map(|group| {
            group["hooks"]
                .as_array()
                .expect("each hook group must have a `hooks` array")
        })
        .map(|hook| {
            hook["command"]
                .as_str()
                .expect("each hook must define a command")
        })
        .collect();
    assert!(
        !commands.is_empty(),
        "expected at least one hook command to syntax-check"
    );

    // Each shell's no-execute flag: parse and report syntax errors without
    // running the command. fish/zsh/bash all spell this `-n`.
    for command in &commands {
        for shell in ["fish", "bash", "zsh"] {
            let output = std::process::Command::new(shell)
                .args(["-n", "-c", command])
                .output()
                .unwrap_or_else(|e| panic!("failed to spawn {shell}: {e}"));
            assert!(
                output.status.success(),
                "{shell} failed to parse hook command:\n  {command}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

// ==================== Plugin Install-Statusline Tests ====================

#[rstest]
fn test_plugins_claude_install_statusline(repo: TestRepo, temp_home: TempDir) {
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);

        // Verify the file was written correctly
        let settings_path = temp_home.path().join(".claude/settings.json");
        let content = fs::read_to_string(&settings_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["statusLine"]["command"],
            "wt list statusline --format=claude-code"
        );
    });
}

#[rstest]
fn test_plugins_claude_install_statusline_already_configured(repo: TestRepo, temp_home: TempDir) {
    TestRepo::setup_statusline_configured(temp_home.path());

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_install_statusline_preserves_existing(repo: TestRepo, temp_home: TempDir) {
    // Write existing settings with other keys
    let claude_dir = temp_home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("settings.json"),
        r#"{"existingKey":"existingValue"}"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);

        // Verify existing keys are preserved
        let settings_path = temp_home.path().join(".claude/settings.json");
        let content = fs::read_to_string(&settings_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["existingKey"], "existingValue");
        assert_eq!(
            parsed["statusLine"]["command"],
            "wt list statusline --format=claude-code"
        );
    });
}

#[rstest]
fn test_plugins_claude_install_statusline_empty_file(repo: TestRepo, temp_home: TempDir) {
    // Write an empty settings.json (edge case: file exists but is empty)
    let claude_dir = temp_home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(claude_dir.join("settings.json"), "").unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);

        // Verify the file was written correctly despite starting empty
        let settings_path = temp_home.path().join(".claude/settings.json");
        let content = fs::read_to_string(&settings_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["statusLine"]["command"],
            "wt list statusline --format=claude-code"
        );
    });
}

#[rstest]
fn test_plugins_claude_install_statusline_honors_claude_config_dir(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // Claude Code relocates its entire config tree under CLAUDE_CONFIG_DIR, so
    // the statusline must land at $CLAUDE_CONFIG_DIR/settings.json, not
    // ~/.claude/settings.json.
    let config_dir = temp_home.path().join("custom-claude");

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("CLAUDE_CONFIG_DIR", &config_dir);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "install-statusline failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Written under CLAUDE_CONFIG_DIR...
    let settings_path = config_dir.join("settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed["statusLine"]["command"],
        "wt list statusline --format=claude-code"
    );

    // ...and NOT under the default ~/.claude.
    assert!(
        !temp_home.path().join(".claude/settings.json").exists(),
        "settings.json must not be written to ~/.claude when CLAUDE_CONFIG_DIR is set"
    );

    // Detection reads the same relocated path: a second run sees it as configured.
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("CLAUDE_CONFIG_DIR", &config_dir);

    let output = cmd.output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("already configured"),
        "second run should detect the relocated statusline as already configured"
    );
}

#[rstest]
fn test_plugins_claude_install_statusline_expands_tilde_in_claude_config_dir(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // A literal `~/` in CLAUDE_CONFIG_DIR (which only reaches us when the
    // variable is set outside a shell) is expanded against the home directory,
    // not treated as a relative path under the cwd.
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("CLAUDE_CONFIG_DIR", "~/custom-claude");

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "install-statusline failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // `~/custom-claude` expanded to <home>/custom-claude.
    let settings_path = temp_home.path().join("custom-claude/settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed["statusLine"]["command"],
        "wt list statusline --format=claude-code"
    );

    // The tilde was not left literal: no `~` directory under the cwd.
    assert!(
        !repo.root_path().join("~").exists(),
        "tilde must be expanded, not treated as a literal relative path"
    );
}

#[rstest]
fn test_plugins_claude_install_statusline_falls_back_to_dot_claude(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // With CLAUDE_CONFIG_DIR unset, paths fall back to the default ~/.claude.
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "plugins", "claude", "install-statusline", "--yes"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env_remove("CLAUDE_CONFIG_DIR");

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "install-statusline failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let settings_path = temp_home.path().join(".claude/settings.json");
    let content = fs::read_to_string(&settings_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed["statusLine"]["command"],
        "wt list statusline --format=claude-code"
    );
}

// ==================== Plugin Command Failure Tests ====================

#[rstest]
fn test_plugins_claude_install_command_fails(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins_failing();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_install_second_step_fails(mut repo: TestRepo, temp_home: TempDir) {
    use crate::common::mock_commands::{MockConfig, MockResponse};

    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_installed();

    // Marketplace add succeeds but plugin install fails
    let mock_bin = repo
        .mock_bin_path()
        .expect("setup_mock_ci_tools_unauthenticated creates mock-bin");
    MockConfig::new("claude")
        .command("plugin marketplace", MockResponse::exit(0))
        .command(
            "plugin install",
            MockResponse::exit(1).with_stderr("error: install failed\n"),
        )
        .write(mock_bin);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "install", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_plugins_claude_uninstall_command_fails(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    repo.setup_mock_claude_with_plugins_failing();
    TestRepo::setup_plugin_installed(temp_home.path());

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.args(["config", "plugins", "claude", "uninstall", "--yes"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

// ==================== Plugin Prompt PTY Tests ====================

#[cfg(all(unix, feature = "shell-integration-tests"))]
mod plugin_prompt_pty {
    use crate::common::pty::{build_pty_command, exec_cmd_in_pty_prompted};
    use crate::common::{TestRepo, repo, temp_home, wt_bin};
    use rstest::rstest;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Build env vars for plugin PTY tests, including mock binary PATH.
    ///
    /// HOME/XDG_CONFIG_HOME are NOT set here — `build_pty_command` handles them
    /// via its `home_dir` parameter.
    fn plugin_env_vars(repo: &TestRepo) -> Vec<(String, String)> {
        let mut vars = repo.test_env_vars();

        // Add mock binary PATH if configured
        if let Some(mock_bin) = repo.mock_bin_path() {
            vars.push((
                "MOCK_CONFIG_DIR".to_string(),
                mock_bin.display().to_string(),
            ));

            // Prepend mock bin to PATH
            let current_path =
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
            let mut paths: Vec<PathBuf> = std::env::split_paths(&current_path).collect();
            paths.insert(0, mock_bin.to_path_buf());
            let new_path = std::env::join_paths(&paths).unwrap();
            vars.retain(|(k, _)| k != "PATH");
            vars.push(("PATH".to_string(), new_path.to_string_lossy().to_string()));
        }

        // Mark claude as installed
        vars.push((
            "WORKTRUNK_TEST_CLAUDE_INSTALLED".to_string(),
            "1".to_string(),
        ));

        vars
    }

    // --- install-statusline prompt tests ---

    #[rstest]
    fn test_plugins_claude_install_statusline_prompt_accept(repo: TestRepo, temp_home: TempDir) {
        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "install-statusline"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["y\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Configure statusline"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            output.contains("Statusline configured"),
            "Should confirm configuration. Output:\n{output}"
        );

        // Verify the file was actually written
        let settings_path = temp_home.path().join(".claude/settings.json");
        let content = std::fs::read_to_string(&settings_path)
            .expect("settings.json should exist after accepting prompt");
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["statusLine"]["command"],
            "wt list statusline --format=claude-code"
        );
    }

    #[rstest]
    fn test_plugins_claude_install_statusline_prompt_preview_then_accept(
        repo: TestRepo,
        temp_home: TempDir,
    ) {
        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "install-statusline"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        // Send "?" to trigger preview, then "y" on the re-prompted prompt
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["?\n", "y\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("statusLine"),
            "Should show preview with statusLine JSON. Output:\n{output}"
        );
        assert!(
            output.contains("Statusline configured"),
            "Should confirm configuration after preview. Output:\n{output}"
        );

        // Verify the file was actually written
        let settings_path = temp_home.path().join(".claude/settings.json");
        let content = std::fs::read_to_string(&settings_path)
            .expect("settings.json should exist after accepting prompt");
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["statusLine"]["command"],
            "wt list statusline --format=claude-code"
        );
    }

    #[rstest]
    fn test_plugins_claude_install_statusline_prompt_decline(repo: TestRepo, temp_home: TempDir) {
        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "install-statusline"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["n\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Configure statusline"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            !output.contains("Statusline configured"),
            "Should NOT configure when declined. Output:\n{output}"
        );

        // Verify the file was NOT written
        let settings_path = temp_home.path().join(".claude/settings.json");
        assert!(
            !settings_path.exists(),
            "settings.json should not exist after declining"
        );
    }

    // --- install prompt tests ---

    #[rstest]
    fn test_plugins_claude_install_prompt_accept(mut repo: TestRepo, temp_home: TempDir) {
        repo.setup_mock_ci_tools_unauthenticated();
        repo.setup_mock_claude_with_plugins();

        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "install"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["y\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Install Worktrunk plugin"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            output.contains("Plugin installed"),
            "Should confirm installation. Output:\n{output}"
        );
    }

    #[rstest]
    fn test_plugins_claude_install_prompt_decline(mut repo: TestRepo, temp_home: TempDir) {
        repo.setup_mock_ci_tools_unauthenticated();
        repo.setup_mock_claude_with_plugins();

        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "install"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["n\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Install Worktrunk plugin"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            !output.contains("Plugin installed"),
            "Should NOT install when declined. Output:\n{output}"
        );
    }

    // --- uninstall prompt tests ---

    #[rstest]
    fn test_plugins_claude_uninstall_prompt_accept(mut repo: TestRepo, temp_home: TempDir) {
        repo.setup_mock_ci_tools_unauthenticated();
        repo.setup_mock_claude_with_plugins();
        TestRepo::setup_plugin_installed(temp_home.path());

        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "uninstall"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["y\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Uninstall Worktrunk plugin"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            output.contains("Plugin uninstalled"),
            "Should confirm uninstallation. Output:\n{output}"
        );
    }

    #[rstest]
    fn test_plugins_claude_uninstall_prompt_decline(mut repo: TestRepo, temp_home: TempDir) {
        repo.setup_mock_ci_tools_unauthenticated();
        repo.setup_mock_claude_with_plugins();
        TestRepo::setup_plugin_installed(temp_home.path());

        let env_vars = plugin_env_vars(&repo);
        let cmd = build_pty_command(
            wt_bin().to_str().unwrap(),
            &["config", "plugins", "claude", "uninstall"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
        );
        let (output, exit_code) = exec_cmd_in_pty_prompted(cmd, &["n\n"], "[y/N");

        assert_eq!(exit_code, 0, "Command should succeed. Output:\n{output}");
        assert!(
            output.contains("Uninstall Worktrunk plugin"),
            "Should show prompt. Output:\n{output}"
        );
        assert!(
            !output.contains("Plugin uninstalled"),
            "Should NOT uninstall when declined. Output:\n{output}"
        );
    }
}

// ============================================================================
// --format=json
// ============================================================================

#[rstest]
fn test_config_show_json(repo: TestRepo, temp_home: TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n",
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_xdg_config_path(&mut cmd, temp_home.path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();

    assert!(json["user"]["exists"].as_bool().unwrap());
    assert!(json["user"]["path"].as_str().is_some());
    assert!(json["user"]["config"].is_object());

    // Project config doesn't exist in this fixture
    assert!(!json["project"]["exists"].as_bool().unwrap());
}

#[rstest]
fn test_config_show_json_with_project_config(repo: TestRepo, temp_home: TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create project config
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "[list]\nurl = \"http://localhost:3000\"\n",
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_xdg_config_path(&mut cmd, temp_home.path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();

    assert!(json["project"]["exists"].as_bool().unwrap());
    assert!(json["project"]["config"].is_object());
}

#[rstest]
fn test_config_show_json_outside_repo(repo: TestRepo, temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ repo }}.{{ branch }}\"\n",
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_xdg_config_path(&mut cmd, temp_home.path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.args(["config", "show", "--format=json"])
        .current_dir(temp_dir.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();

    assert!(json["user"]["exists"].as_bool().unwrap());
    assert!(json["user"]["config"].is_object());

    // Outside a repo: project path and config are null
    assert!(json["project"]["path"].is_null());
    assert!(!json["project"]["exists"].as_bool().unwrap());
    assert!(json["project"]["config"].is_null());
}

/// `WORKTRUNK_PROJECT_CONFIG_PATH` overrides the default `.config/wt.toml`
/// lookup. Mirrors `WORKTRUNK_CONFIG_PATH` / `WORKTRUNK_SYSTEM_CONFIG_PATH`
/// for project config — used to isolate tests (including completion tests)
/// from any `[aliases]` in the developer's own project config.
#[rstest]
fn test_project_config_path_env_var_override(repo: TestRepo, temp_home: TempDir) {
    // Write a project config in the repo that should be *ignored* when the
    // override points elsewhere.
    let in_repo_config = repo.root_path().join(".config").join("wt.toml");
    fs::create_dir_all(in_repo_config.parent().unwrap()).unwrap();
    fs::write(&in_repo_config, "pre-start = \"in-repo-hook\"\n").unwrap();

    // Write the override project config at an arbitrary path.
    let override_dir = tempfile::tempdir().unwrap();
    let override_path = override_dir.path().join("override.toml");
    fs::write(&override_path, "pre-start = \"override-hook\"\n").unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_xdg_config_path(&mut cmd, temp_home.path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_PROJECT_CONFIG_PATH", &override_path);
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(
        json["project"]["config"]["pre-start"], "override-hook",
        "expected override config to be loaded, got: {}",
        json["project"]
    );

    // And a missing override path resolves to no project config (same as a
    // missing `.config/wt.toml`) — doesn't silently fall back to the repo's
    // own file.
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_xdg_config_path(&mut cmd, temp_home.path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env(
        "WORKTRUNK_PROJECT_CONFIG_PATH",
        override_dir.path().join("nonexistent.toml"),
    );
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert!(
        json["project"]["config"].is_null(),
        "missing override path should resolve to no project config, got: {}",
        json["project"]["config"]
    );
}
