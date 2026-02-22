use crate::common::{
    TestRepo, repo, set_temp_home_env, set_xdg_config_path, setup_snapshot_settings,
    setup_snapshot_settings_with_home, temp_home, wt_command,
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
        r#"post-create = "npm install"

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

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Both hooks should be present (deep merge preserves differently-named keys)
    assert!(
        stderr.contains("company-lint-tool"),
        "System hook should be preserved with different name, got:\n{stderr}"
    );
    assert!(
        stderr.contains("my-lint-tool"),
        "User hook should be present, got:\n{stderr}"
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

    let stderr = String::from_utf8_lossy(&output.stderr);

    // User's command should replace system's for the same name
    assert!(
        stderr.contains("my-lint-tool"),
        "User's hook command should be present, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("company-lint-tool"),
        "System's hook command should be replaced by user's same-named hook, got:\n{stderr}"
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("company-format-tool"),
        "System's pre-commit hook should be preserved when user doesn't override it, got:\n{stderr}"
    );
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
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should NOT show a full SYSTEM CONFIG heading
    assert!(
        !stderr.contains("SYSTEM CONFIG"),
        "Should not show SYSTEM CONFIG section when absent, got:\n{stderr}"
    );
    // Should show a system config hint under USER CONFIG
    assert!(
        stderr.contains("Optional system config not found")
            && stderr.contains("worktrunk/config.toml"),
        "Expected system config hint in output, got:\n{stderr}"
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

/// Test that config show displays "Outdated" when fish wrapper exists but has different code
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Completions won't work; add to"),
            "Expected compinit warning, got:\n{stderr}"
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Completions won't work; add to"),
            "Expected no compinit warning, got:\n{stderr}"
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

#[rstest]
fn test_config_show_github_remote(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Add GitHub remote
    repo.git_command()
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/example/repo.git",
        ])
        .output()
        .unwrap();

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

    // Add GitLab remote
    repo.git_command()
        .args([
            "remote",
            "add",
            "origin",
            "https://gitlab.com/example/repo.git",
        ])
        .output()
        .unwrap();

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

/// When a config uses deprecated variables (repo_root, worktree, main_worktree),
/// the CLI should:
/// 1. Show a warning pointing to `wt config show` and `wt config update`
/// 2. Create a .new migration file with replacements
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
post-create = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
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

    // Verify migration file was created (config.toml -> config.toml.new)
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );

    // Verify migration file has replacements
    let migrated_content = fs::read_to_string(&migration_file).unwrap();
    assert!(
        migrated_content.contains("{{ repo }}"),
        "Migration should replace main_worktree with repo"
    );
    assert!(
        migrated_content.contains("{{ repo_path }}"),
        "Migration should replace repo_root with repo_path"
    );
    assert!(
        migrated_content.contains("{{ worktree_path }}"),
        "Migration should replace worktree with worktree_path"
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
post-create = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
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
fn test_deprecated_template_variables_hint_deduplication(repo: TestRepo, temp_home: TempDir) {
    // Write project config with deprecated variables
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    // First run - should create migration file and set hint
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let migration_file = project_config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "First run should create migration file"
    );

    let original_content = fs::read_to_string(&migration_file).unwrap();

    // Second run - hint is set, so wt list shows brief warning and skips writing
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "Second run should succeed: {:?}",
            stderr
        );
        assert!(
            stderr.contains("deprecated settings"),
            "Second run should show brief warning, got: {stderr}"
        );
        assert!(
            !stderr.contains("Wrote migrated"),
            "Second run should NOT write migration file (hint is set), got: {stderr}"
        );
    }

    // Content should be unchanged (wt list didn't touch it)
    let current_content = fs::read_to_string(&migration_file).unwrap();
    assert_eq!(
        original_content, current_content,
        "Migration file should be unchanged by second wt list run"
    );
}

/// This tests the skip-write path for project config with non-config-show commands.
///
/// Migration file write is deduplicated based on file existence:
/// - First run: file doesn't exist → write it
/// - Second run: file exists → skip write, show brief warning only
/// Users can run `wt config show` to force regeneration.
#[rstest]
fn test_wt_list_skips_migration_file_after_first_write(repo: TestRepo, temp_home: TempDir) {
    // Write project config with deprecated variables
    // Use deprecated variable main_worktree (should be repo) in a valid project config field
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"post-create = "ln -sf {{ main_worktree }}/node_modules"
"#,
    )
    .unwrap();

    // First run - creates migration file
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let migration_file = project_config_path.with_extension("toml.new");
    assert!(migration_file.exists());

    // Second run - file exists → skip write, show brief warning only
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });

    // Migration file still exists (not deleted or overwritten)
    assert!(
        migration_file.exists(),
        "Migration file should still exist after second run"
    );
}

/// Migration file is regenerated when deleted (file-existence based deduplication).
#[rstest]
fn test_deleted_migration_file_regenerated(repo: TestRepo, temp_home: TempDir) {
    // Write project config with deprecated variables
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    // First run - creates migration file
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let migration_file = project_config_path.with_extension("toml.new");
    assert!(migration_file.exists());

    // Delete the migration file to simulate user having applied and removed it
    fs::remove_file(&migration_file).unwrap();

    // Second run - should recreate migration file since it doesn't exist
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "Second run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Migration file should exist again
    assert!(
        migration_file.exists(),
        "Migration file should be regenerated after deletion"
    );
}

/// When a user fixes their deprecated config, the hint should be cleared automatically.
/// This ensures that future deprecations (introduced months later) get full treatment.
#[rstest]
fn test_fixing_deprecated_config_clears_hint_for_future_deprecations(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // Write project config with deprecated variable
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"post-create = "ln -sf {{ main_worktree }}/node_modules"
"#,
    )
    .unwrap();

    // First run - creates migration file and sets hint
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(output.status.success());
    }
    assert!(
        project_config_path.with_extension("toml.new").exists(),
        "First run should create migration file"
    );

    // User fixes the config (removes deprecation)
    fs::write(
        &project_config_path,
        r#"post-create = "ln -sf {{ repo }}/node_modules"
"#,
    )
    .unwrap();

    // Clean up migration file
    let migration_file = project_config_path.with_extension("toml.new");
    if migration_file.exists() {
        fs::remove_file(&migration_file).unwrap();
    }

    // Second run with fixed config - hint should be cleared
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("deprecated"),
            "No deprecation warning for fixed config"
        );
    }

    // Months later, a NEW deprecation is introduced - user adds a different deprecated variable
    fs::write(
        &project_config_path,
        r#"post-create = "cd {{ worktree }} && npm install"
"#,
    )
    .unwrap();

    // Third run with new deprecation - should get full treatment
    // because hint was cleared when config was clean
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("deprecated settings"),
            "New deprecation should show warning, got: {stderr}"
        );
    }

    // Migration file should exist for the new deprecation
    assert!(
        migration_file.exists(),
        "Migration file should be created for new deprecation"
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

/// User config migration file write is deduplicated based on file existence.
/// First run creates the migration file. Subsequent runs skip the write
/// if the file already exists (brief warning only).
#[rstest]
fn test_user_config_deprecated_variables_deduplication(repo: TestRepo, temp_home: TempDir) {
    // Write user config with deprecated variables using the test config path
    // (WORKTRUNK_CONFIG_PATH is set by repo.wt_command(), not .config/worktrunk/config.toml)
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    );
    let user_config_path = repo.test_config_path().to_path_buf();

    // First run - should create migration file
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let migration_file = user_config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "First run should create migration file"
    );

    // Second run - hint is already marked shown, skip file write
    // Should show brief warning only, NOT regenerate the file
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", &user_config_path);
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "Second run should succeed: {:?}",
            stderr
        );
        // Should show brief warning (deprecated settings) but NOT write file
        assert!(
            stderr.contains("User config has deprecated settings"),
            "Second run should show brief warning, got: {stderr}"
        );
        assert!(
            !stderr.contains("Wrote migrated"),
            "Second run should NOT regenerate migration file (hint already shown), got: {stderr}"
        );
    }

    // Verify migration file still exists (from first run)
    assert!(migration_file.exists());
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
        cmd.env("WORKTRUNK_DIRECTIVE_FILE", &directive_file);

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
            "C:\\Users\\user\\Documents\\PowerShell\\Modules",
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

    // Verify migration file was created (config.toml -> config.toml.new)
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );

    // Verify migration file has correct transformations
    let migrated_content = fs::read_to_string(&migration_file).unwrap();
    assert!(
        migrated_content.contains("[commit.generation]"),
        "Migration should rename [commit-generation] to [commit.generation]"
    );
    assert!(
        migrated_content.contains("command = \"llm -m haiku\""),
        "Migration should merge args into command"
    );
    assert!(
        !migrated_content.contains("[commit-generation]"),
        "Migration should remove old section name"
    );
    assert!(
        !migrated_content.contains("args ="),
        "Migration should remove args field"
    );
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

    // Verify migration file was created and has correct transformations
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );

    let migrated_content = fs::read_to_string(&migration_file).unwrap();
    assert!(
        migrated_content.contains("[projects.\"github.com/example/repo\".commit.generation]"),
        "Migration should rename project-level section"
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
post-create = "ln -sf {{ repo_root }}/node_modules"
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

    // Verify migration file was created
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );
}

/// Test that `wt config show` always regenerates migration file
///
/// Even if the user deleted the migration file previously, `wt config show`
/// should always regenerate it (unlike other commands which skip after first write).
#[rstest]
fn test_config_show_always_regenerates_migration_file(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab/claude for deterministic output
    repo.setup_mock_ci_tools_unauthenticated();

    // Write project config with deprecated variables
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"post-create = "ln -sf {{ main_worktree }}/node_modules"
"#,
    )
    .unwrap();

    // First run with wt list - creates migration file and sets hint
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let migration_file = project_config_path.with_extension("toml.new");
    assert!(migration_file.exists(), "Migration file should be created");

    // Delete the migration file (simulating user applied it or doesn't want it)
    fs::remove_file(&migration_file).unwrap();

    // Run wt config show - should regenerate migration file
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

    // Migration file SHOULD be regenerated by wt config show
    assert!(
        migration_file.exists(),
        "Migration file should be regenerated by wt config show"
    );

    // Verify the regenerated file has the correct content
    let migrated_content = fs::read_to_string(&migration_file).unwrap();
    assert!(
        migrated_content.contains("{{ repo }}"),
        "Migration should replace main_worktree with repo"
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
        r#"post-create = "ln -sf {{ main_worktree }}/node_modules"
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

    // Verify migration file was created
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );
}

/// Test that deprecated approved-commands in [projects] sections are copied to approvals.toml
#[rstest]
fn test_deprecated_approved_commands_copies_to_approvals_file(repo: TestRepo, temp_home: TempDir) {
    // Write user config with approved-commands in [projects] section
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#,
    )
    .unwrap();

    // Use `wt list` which loads config and triggers deprecation check
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

        assert_cmd_snapshot!(cmd);
    });

    // Verify migration file removes approved-commands
    let migration_file = config_path.with_extension("toml.new");
    assert!(
        migration_file.exists(),
        "Migration file should be created at {:?}",
        migration_file
    );
    let migrated_content = fs::read_to_string(&migration_file).unwrap();
    assert!(
        !migrated_content.contains("approved-commands"),
        "Migration should remove approved-commands"
    );

    // Verify approvals were copied to approvals.toml (sibling of config file)
    let approvals_file = config_path.with_file_name("approvals.toml");
    assert!(
        approvals_file.exists(),
        "Approvals should be copied to {:?}",
        approvals_file
    );
    let approvals_content = fs::read_to_string(&approvals_file).unwrap();
    assert!(
        approvals_content.contains("npm install"),
        "Approvals file should contain npm install: {}",
        approvals_content
    );
    assert!(
        approvals_content.contains("npm test"),
        "Approvals file should contain npm test: {}",
        approvals_content
    );
}

// ==================== config update tests ====================

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
post-create = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
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
