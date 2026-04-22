use crate::common::{
    TestRepo, repo, set_temp_home_env, set_xdg_config_path, setup_home_snapshot_settings,
    setup_snapshot_settings, setup_snapshot_settings_with_home, temp_home, wt_command,
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

    // Create nushell vendor/autoload directory with an outdated wt.nu
    let autoload = temp_home.path().join(".config/nushell/vendor/autoload");
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
        .run()
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
        .run()
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

/// Install uses dirs::config_dir() fallback when OPENCODE_CONFIG_DIR is unset.
///
/// This exercises the `dirs::config_dir()` branch in `opencode_plugins_dir()`
/// (lines 26-28 of opencode.rs). On Linux with XDG_CONFIG_HOME set, dirs
/// resolves to `$XDG_CONFIG_HOME`, so the plugin lands at
/// `{temp_home}/.config/opencode/plugins/worktrunk.ts`.
///
/// Linux-only: `dirs::config_dir()` resolves differently per platform
/// (macOS: `~/Library/Application Support`, Windows: native API), making
/// the path assertion platform-specific. The core install logic is tested
/// cross-platform via `OPENCODE_CONFIG_DIR` in other tests.
#[cfg(target_os = "linux")]
#[rstest]
fn test_opencode_install_uses_dirs_fallback(temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        // Remove OPENCODE_CONFIG_DIR so the code falls through to dirs::config_dir()
        cmd.env_remove("OPENCODE_CONFIG_DIR");
        cmd.args(["config", "plugins", "opencode", "install", "--yes"]);

        assert_cmd_snapshot!(cmd);
    });

    let canonical_home =
        crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().into());
    // dirs::config_dir() uses XDG_CONFIG_HOME on Linux → {temp_home}/.config
    let plugin_path = canonical_home.join(".config/opencode/plugins/worktrunk.ts");
    assert!(
        plugin_path.exists(),
        "Plugin file should exist at dirs::config_dir() fallback path: {}",
        plugin_path.display()
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
/// touching the config file. Warnings still go to stderr.
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

/// `wt config show` displays deprecation details for pre-* hooks in table form.
/// Uses project config with two multi-entry pre-* tables to cover the
/// "Project config" label and the multi-hook list form of the warning.
#[rstest]
fn test_config_show_displays_pre_hook_table_form_deprecation(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
    repo.setup_mock_ci_tools_unauthenticated();

    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("wt.toml");
    fs::write(
        &project_config_path,
        r#"[pre-merge]
test = "cargo test"
lint = "cargo clippy"

[pre-start]
install = "npm ci"
env = "cp .env.example .env"
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

/// `post-create` was renamed to `pre-start` in v0.32.0. Project configs that
/// still carry the removed key fail to load with a fatal error naming the
/// replacement. Verify via `wt switch --create`, which propagates project
/// config load failures as a non-zero exit.
#[rstest]
fn test_post_create_in_project_config_is_fatal(repo: TestRepo, temp_home: TempDir) {
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    fs::write(
        project_config_dir.join("wt.toml"),
        "post-create = \"npm install\"\n",
    )
    .unwrap();

    let mut cmd = repo.wt_command();
    cmd.args(["switch", "--create", "new-branch"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "project config with post-create must fail to load, stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("post-create"),
        "error should name the offending key, got: {stderr}"
    );
    assert!(
        stderr.contains("pre-start"),
        "error should point at the replacement key, got: {stderr}"
    );
}

/// User config is loaded on a best-effort basis: a fatal deprecation surfaces
/// as a `LoadError::Validation` warning, wt continues without it, and the user
/// still gets the fatal message telling them to rename the key.
#[rstest]
fn test_post_create_in_user_config_warns_and_skips(repo: TestRepo, temp_home: TempDir) {
    let config_path = repo.test_config_path();
    fs::write(config_path, "post-create = \"npm install\"\n").unwrap();

    let mut cmd = repo.wt_command();
    cmd.arg("list").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt list should still succeed when user config fails validation, stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("post-create") && stderr.contains("pre-start"),
        "warning should describe the rename, got: {stderr}"
    );
}

/// `wt config show` renders the fatal post-create error inline for both user
/// and project configs, continuing the show flow so the user can still see
/// their file and other sections.
#[rstest]
fn test_config_show_renders_post_create_error(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();

    // User config at XDG path.
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "post-create = \"npm install\"\n",
    )
    .unwrap();

    // Project config in the repo.
    let project_config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&project_config_dir).unwrap();
    fs::write(
        project_config_dir.join("wt.toml"),
        "post-create = \"bundle install\"\n",
    )
    .unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    repo.configure_mock_commands(&mut cmd);
    cmd.args(["config", "show"]).current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    set_xdg_config_path(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt config show should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Both sections should render the rename guidance.
    assert!(
        combined.matches("post-create").count() >= 2,
        "expected post-create message in both user and project sections, got: {combined}"
    );
    assert!(
        combined.contains("User config: `post-create`"),
        "user section should name the removed key, got: {combined}"
    );
    assert!(
        combined.contains("Project config: `post-create`"),
        "project section should name the removed key, got: {combined}"
    );
}
