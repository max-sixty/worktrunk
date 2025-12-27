use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

/// Test `wt config show` with both global and project configs present
#[rstest]
fn test_config_show_with_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config at XDG path (used on all platforms with etcetera)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` when there is no project config
#[rstest]
fn test_config_show_no_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config (but no project config) at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` outside a git repository
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
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

/// Test `wt config show` warns when zsh compinit is not enabled
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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` shows hint when some shells configured, some not
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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1"); // Bypass zsh subprocess check

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` shows no warning when zsh compinit is enabled
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
        repo.clean_cli_env(&mut cmd);
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
        repo.clean_cli_env(&mut cmd);
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
fn test_config_show_zsh_compinit_real_probe_no_warning_when_present(
    mut repo: TestRepo,
    temp_home: TempDir,
) {
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
        repo.clean_cli_env(&mut cmd);
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

/// Test `wt config show` warns about unknown/misspelled keys in project config
#[rstest]
fn test_config_show_warns_unknown_project_keys(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"",
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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` warns about unknown keys in user config
#[rstest]
fn test_config_show_warns_unknown_user_keys(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create global config with typo: commit-gen instead of commit-generation
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"\n\n[commit-gen]\ncommand = \"llm\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show --full` when commit generation is not configured
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
        "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"",
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
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

/// Test `wt config show --full` when commit generation command doesn't exist
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "nonexistent-llm-command-12345"
args = ["-m", "test-model"]
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
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

/// Test `wt config show` with GitHub remote shows only gh status
#[rstest]
fn test_config_show_github_remote(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Add GitHub remote
    repo.git_command(&[
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` with GitLab remote shows only glab status
#[rstest]
fn test_config_show_gitlab_remote(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Add GitLab remote
    repo.git_command(&[
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` with empty project config file
#[rstest]
fn test_config_show_empty_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` with whitespace-only project config file
#[rstest]
fn test_config_show_whitespace_only_project_config(mut repo: TestRepo, temp_home: TempDir) {
    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
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
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}
