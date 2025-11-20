use crate::common::{TestRepo, set_temp_home_env, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config shell` with --force flag (skips confirmation)
#[test]
fn test_configure_shell_with_yes() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added [1mzsh[0m ~/.zshrc
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init zsh)"[0m; [1m[35mfi[0m[0m

        ✅ [32mConfigured 1 shell[0m

        💡 [2mRestart your shell or run: source <config-file>[0m

        ----- stderr -----
        "#);
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt init zsh)\""));
}

/// Test `wt config shell` with specific shell
#[test]
fn test_configure_shell_specific_shell() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--shell")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added [1mzsh[0m ~/.zshrc
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init zsh)"[0m; [1m[35mfi[0m[0m

        ✅ [32mConfigured 1 shell[0m

        💡 [2mRestart your shell or run: source <config-file>[0m

        ----- stderr -----
        "#);
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt init zsh)\""));
}

/// Test `wt config shell` when line already exists
#[test]
fn test_configure_shell_already_exists() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file with the line already present
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt init zsh)\"; fi\n",
    )
    .unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--shell")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ [32mAll shells already configured[0m

        ----- stderr -----
        ");
    });

    // Verify the file was not modified (no duplicate)
    let content = fs::read_to_string(&zshrc_path).unwrap();
    let count = content.matches("wt init").count();
    assert_eq!(count, 1, "Should only have one wt init line");
}

/// Test `wt config shell` for Fish (creates new file in conf.d/)
#[test]
fn test_configure_shell_fish() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--shell")
            .arg("fish")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Created [1mfish[0m ~/.config/fish/conf.d/wt.fish
        [40m [0m  [1m[35mif[0m [1m[34mtype[0m [36m-q[0m wt; [1m[34mcommand[0m wt init fish [36m|[0m [1m[34msource[0m; end[0m

        ✅ [32mConfigured 1 shell[0m

        💡 [2mRestart your shell or run: source <config-file>[0m

        ----- stderr -----
        ");
    });

    // Verify the fish conf.d file was created
    let fish_config = temp_home.path().join(".config/fish/conf.d/wt.fish");
    assert!(fish_config.exists(), "Fish config file should be created");

    let content = fs::read_to_string(&fish_config).unwrap();
    assert!(
        content.trim() == "if type -q wt; command wt init fish | source; end",
        "Should contain conditional wrapper: {}",
        content
    );
}

/// Test `wt config shell` when no config files exist
#[test]
fn test_configure_shell_no_files() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    // Normalize bash config file names across platforms
    // Linux: ".bashrc, .bash_profile" → remove ".bashrc, "
    // macOS: ".bash_profile, .profile" → remove ", .profile"
    settings.add_filter(r"\[TEMP_HOME\]/\.bashrc, ", "");
    settings.add_filter(r", \[TEMP_HOME\]/\.profile", "");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----
        ❌ [31mNo shell config files found in $HOME. Checked: [TEMP_HOME]/.bash_profile, [TEMP_HOME]/.zshrc, and more. Create a config file or use --shell to specify a shell.[0m

        ----- stderr -----
        ");
    });
}

/// Test `wt config shell` for Fish with custom prefix
/// Test `wt config shell` with multiple existing config files
#[test]
fn test_configure_shell_multiple_configs() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create multiple shell config files
    let bash_config_path = temp_home.path().join(".bash_profile");
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&bash_config_path, "# Existing bash config\n").unwrap();
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added [1mbash[0m ~/.bash_profile
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init bash)"[0m; [1m[35mfi[0m[0m
        ✅ Added [1mzsh[0m ~/.zshrc
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init zsh)"[0m; [1m[35mfi[0m[0m

        ✅ [32mConfigured 2 shells[0m

        💡 [2mRestart your shell or run: source <config-file>[0m

        ----- stderr -----
        "#);
    });

    // Verify both files were modified
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    assert!(
        bash_content.contains("eval \"$(command wt init bash)\""),
        "Bash config should be updated"
    );

    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        zsh_content.contains("eval \"$(command wt init zsh)\""),
        "Zsh config should be updated"
    );
}

/// Test `wt config shell` shows both shells needing updates and already configured shells
#[test]
fn test_configure_shell_mixed_states() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create bash config with wt already configured
    let bash_config_path = temp_home.path().join(".bash_profile");
    fs::write(
        &bash_config_path,
        "# Existing config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt init bash)\"; fi\n",
    )
    .unwrap();

    // Create zsh config without wt
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("config")
            .arg("shell")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Already configured [1mbash[0m ~/.bash_profile
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init bash)"[0m; [1m[35mfi[0m[0m
        ✅ Added [1mzsh[0m ~/.zshrc
        [40m [0m  [1m[35mif[0m [1m[34mcommand[0m [36m-v[0m wt [36m>[0m/dev/null [33m2[0m>&1; [1m[35mthen[0m [1m[34meval[0m [32m"$([1m[34mcommand[0m wt init zsh)"[0m; [1m[35mfi[0m[0m

        ✅ [32mConfigured 1 shell[0m

        💡 [2mRestart your shell or run: source <config-file>[0m

        ----- stderr -----
        "#);
    });

    // Verify bash was not modified (already configured)
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    let bash_wt_count = bash_content.matches("wt init").count();
    assert_eq!(
        bash_wt_count, 1,
        "Bash should still have exactly one wt init line"
    );

    // Verify zsh was modified
    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        zsh_content.contains("eval \"$(command wt init zsh)\""),
        "Zsh config should be updated"
    );
}
