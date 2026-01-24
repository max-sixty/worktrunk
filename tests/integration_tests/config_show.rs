use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_snapshot_settings_with_home, temp_home, wt_command,
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

[projects."test-project"]
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

        assert_cmd_snapshot!(cmd);
    });
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
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1"); // Bypass zsh subprocess check

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
        cmd.env("ZDOTDIR", temp_home.path());
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

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
        cmd.env("ZDOTDIR", temp_home.path());
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

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
        // Override WORKTRUNK_CONFIG_PATH to point to our test config
        cmd.env("WORKTRUNK_CONFIG_PATH", &config_path);
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

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
        // Override WORKTRUNK_CONFIG_PATH to point to our test config
        cmd.env("WORKTRUNK_CONFIG_PATH", &config_path);
        cmd.arg("config")
            .arg("show")
            .arg("--full")
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

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
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1");

        assert_cmd_snapshot!(cmd);
    });
}

/// When a config uses deprecated variables (repo_root, worktree, main_worktree),
/// the CLI should:
/// 1. Show a warning listing the deprecated variables and their replacements
/// 2. Create a .new migration file with replacements
/// 3. Show a hint with the mv command to apply the migration
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

        assert_cmd_snapshot!(cmd);
    });
}

/// When a migration file has already been written, subsequent commands should:
/// 1. Still show the deprecation warning
/// 2. NOT overwrite the migration file
/// 3. Show a hint about how to regenerate the migration file
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Wrote migrated"),
            "First run should write migration file, got: {stderr}"
        );
    }

    let migration_file = project_config_path.with_extension("toml.new");
    assert!(migration_file.exists());

    let original_content = fs::read_to_string(&migration_file).unwrap();

    // Second run - if .new exists, we always regenerate (overwrite)
    // This ensures users always have a fresh migration file
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
            stderr.contains("Wrote migrated"),
            "Second run should regenerate migration file, got: {stderr}"
        );
    }

    // Content should be the same (same input produces same output)
    let current_content = fs::read_to_string(&migration_file).unwrap();
    assert_eq!(
        original_content, current_content,
        "Regenerated content should be the same"
    );
}

/// This tests the skip-write path for project config
#[rstest]
fn test_deprecated_config_deleted_shows_regenerate_hint(repo: TestRepo, temp_home: TempDir) {
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

    // First run - creates migration file and sets hint
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

    // Delete the migration file (user doesn't want it)
    fs::remove_file(&migration_file).unwrap();

    // Second run - .new doesn't exist but hint is set → skip write, show clear hint
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });

    // Migration file should NOT exist (we skipped writing)
    assert!(
        !migration_file.exists(),
        "Migration file should not be recreated when hint is set"
    );
}

#[rstest]
fn test_deprecated_variables_hint_clear_regenerates(repo: TestRepo, temp_home: TempDir) {
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

    // First run - creates migration file and sets hint
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

    // Clear the hint
    {
        let mut cmd = repo.wt_command();
        cmd.args(["config", "state", "hints", "clear", "deprecated-config"])
            .current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "Hint clear should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Third run - should create migration file again
    {
        let mut cmd = repo.wt_command();
        cmd.arg("list").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "Third run should succeed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Wrote migrated"),
            "After clearing hint, should write migration file again, got: {stderr}"
        );
    }

    // Migration file should exist again
    assert!(
        migration_file.exists(),
        "Migration file should be regenerated after clearing hint"
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

/// User config has no repo context, so hint deduplication is based on file existence.
/// When the .new file already exists, subsequent runs should show a hint about deleting
/// the file to regenerate (not about clearing a git config hint).
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
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "First run should succeed: {:?}",
            stderr
        );
        assert!(
            stderr.contains("Wrote migrated"),
            "First run should write migration file, got: {stderr}"
        );
    }

    let migration_file = user_config_path.with_extension("toml.new");
    assert!(migration_file.exists());

    // Second run - user config always regenerates (overwrites .new file)
    // Should still show "Wrote migrated" since we overwrite existing .new files
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
            stderr.contains("Wrote migrated"),
            "Second run should regenerate migration file, got: {stderr}"
        );
    }

    // Verify migration file still exists
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

/// Test that `wt config show` shows regenerate hint when user deleted migration file
///
/// When the hint is set but migration file doesn't exist, `wt config show` should
/// show how to regenerate it (via `wt config state hints clear deprecated-config`).
#[rstest]
fn test_config_show_user_deleted_migration_shows_regenerate_hint(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
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

    // Delete the migration file (simulating user doesn't want it)
    fs::remove_file(&migration_file).unwrap();

    // Run wt config show - should show regenerate hint
    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });

    // Migration file should NOT be recreated (hint is set)
    assert!(
        !migration_file.exists(),
        "Migration file should not be recreated when hint is set"
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
