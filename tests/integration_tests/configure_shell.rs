use crate::common::{TestRepo, set_temp_home_env, setup_home_snapshot_settings, wt_command};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config shell install` with --force flag (skips confirmation)
#[test]
fn test_configure_shell_with_yes() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m via lazy compdef (no file needed)
        💡 [2mSkipped [1mbash[0m; ~/.bashrc not found[0m
        💡 [2mSkipped [1mfish[0m; ~/.config/fish/conf.d not found[0m

        ✅ Configured 1 shell
        💡 [2mRestart shell or run: source ~/.zshrc[0m

        ----- stderr -----
        ");
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt config shell init zsh)\""));
}

/// Test `wt config shell install` with specific shell
#[test]
fn test_configure_shell_specific_shell() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m via lazy compdef (no file needed)

        ✅ Configured 1 shell
        💡 [2mRestart shell or run: source ~/.zshrc[0m

        ----- stderr -----
        ");
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt config shell init zsh)\""));
}

/// Test `wt config shell install` when line already exists
#[test]
fn test_configure_shell_already_exists() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file with the line already present
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Already configured shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ✅ All shells already configured

        ----- stderr -----
        ");
    });

    // Verify the file was not modified (no duplicate)
    let content = fs::read_to_string(&zshrc_path).unwrap();
    let count = content.matches("wt config shell init").count();
    assert_eq!(count, 1, "Should only have one wt config shell init line");
}

/// Test `wt config shell install` for Fish (creates new file in conf.d/)
#[test]
fn test_configure_shell_fish() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Created shell extension for [1mfish[0m @ [1m~/.config/fish/conf.d/wt.fish[0m
        ✅ Created completions for [1mfish[0m @ [1m~/.config/fish/completions/wt.fish[0m

        ✅ Configured 2 shells
        💡 [2mRestart shell or run: source ~/.config/fish/conf.d/wt.fish[0m

        ----- stderr -----
        ");
    });

    // Verify the fish conf.d file was created
    let fish_config = temp_home.path().join(".config/fish/conf.d/wt.fish");
    assert!(fish_config.exists(), "Fish config file should be created");

    let content = fs::read_to_string(&fish_config).unwrap();
    assert!(
        content.trim() == "if type -q wt; command wt config shell init fish | source; end",
        "Should contain conditional wrapper: {}",
        content
    );
}

/// Test `wt config shell install` when no config files exist
#[test]
fn test_configure_shell_no_files() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----
        💡 [2mSkipped [1mbash[0m; ~/.bashrc not found[0m
        💡 [2mSkipped [1mzsh[0m; ~/.zshrc not found[0m
        💡 [2mSkipped [1mfish[0m; ~/.config/fish/conf.d not found[0m
        ❌ [31mNo shell config files found[0m

        ----- stderr -----
        ");
    });
}

/// Test `wt config shell install` with multiple existing config files
#[test]
fn test_configure_shell_multiple_configs() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create multiple shell config files
    let bash_config_path = temp_home.path().join(".bashrc");
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&bash_config_path, "# Existing bash config\n").unwrap();
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Added shell extension for [1mbash[0m @ [1m~/.bashrc[0m
        ✅ Created completions for [1mbash[0m @ [1m~/.local/share/bash-completion/completions/wt[0m
        ✅ Added shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m via lazy compdef (no file needed)
        💡 [2mSkipped [1mfish[0m; ~/.config/fish/conf.d not found[0m

        ✅ Configured 3 shells
        💡 [2mRestart shell or run: source ~/.zshrc[0m

        ----- stderr -----
        ");
    });

    // Verify both files were modified
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    assert!(
        bash_content.contains("eval \"$(command wt config shell init bash)\""),
        "Bash config should be updated"
    );

    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        zsh_content.contains("eval \"$(command wt config shell init zsh)\""),
        "Zsh config should be updated"
    );
}

/// Test `wt config shell install` shows both shells needing updates and already configured shells
#[test]
fn test_configure_shell_mixed_states() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create bash config with wt already configured
    let bash_config_path = temp_home.path().join(".bashrc");
    fs::write(
        &bash_config_path,
        "# Existing config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init bash)\"; fi\n",
    )
    .unwrap();

    // Create zsh config without wt
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Already configured shell extension for [1mbash[0m @ [1m~/.bashrc[0m
        ✅ Created completions for [1mbash[0m @ [1m~/.local/share/bash-completion/completions/wt[0m
        ✅ Added shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m via lazy compdef (no file needed)
        💡 [2mSkipped [1mfish[0m; ~/.config/fish/conf.d not found[0m

        ✅ Configured 2 shells
        💡 [2mRestart shell or run: source ~/.zshrc[0m

        ----- stderr -----
        ");
    });

    // Verify bash was not modified (already configured)
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    let bash_wt_count = bash_content.matches("wt config shell init").count();
    assert_eq!(
        bash_wt_count, 1,
        "Bash should still have exactly one wt config shell init line"
    );

    // Verify zsh was modified
    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        zsh_content.contains("eval \"$(command wt config shell init zsh)\""),
        "Zsh config should be updated"
    );
}

/// Test `wt config shell uninstall` removes shell integration
#[test]
fn test_uninstall_shell() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file with wt integration
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Removed shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m were via lazy compdef (no file to remove)
        💡 [2mNo bash integration in ~/.bashrc[0m
        💡 [2mNo fish integration in ~/.config/fish/conf.d/wt.fish[0m

        ✅ Removed integration from 1 shell
        💡 [2mRestart shell to complete uninstall[0m

        ----- stderr -----
        ");
    });

    // Verify the file no longer contains the integration
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        !content.contains("wt config shell init"),
        "Integration should be removed"
    );
    assert!(
        content.contains("# Existing config"),
        "Other content should be preserved"
    );
}

/// Test `wt config shell uninstall` with multiple shells
#[test]
fn test_uninstall_shell_multiple() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create multiple shell configs with wt integration
    let bash_config_path = temp_home.path().join(".bashrc");
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &bash_config_path,
        "# Bash config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init bash)\"; fi\n",
    )
    .unwrap();
    fs::write(
        &zshrc_path,
        "# Zsh config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Removed shell extension for [1mbash[0m @ [1m~/.bashrc[0m
        ✅ Removed shell extension for [1mzsh[0m @ [1m~/.zshrc[0m
        ⚪ Completions for [1mzsh[0m were via lazy compdef (no file to remove)
        💡 [2mNo fish integration in ~/.config/fish/conf.d/wt.fish[0m

        ✅ Removed integration from 2 shells
        💡 [2mRestart shell to complete uninstall[0m

        ----- stderr -----
        ");
    });

    // Verify both files no longer contain the integration
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    assert!(
        !bash_content.contains("wt config shell init"),
        "Bash integration should be removed"
    );

    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        !zsh_content.contains("wt config shell init"),
        "Zsh integration should be removed"
    );
}

/// Test `wt config shell uninstall` when not installed
#[test]
fn test_uninstall_shell_not_found() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a fake .zshrc file without wt integration
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        🟡 No shell integration found in ~/.zshrc

        ----- stderr -----
        ");
    });
}

/// Test `wt config shell uninstall` for Fish (deletes file)
#[test]
fn test_uninstall_shell_fish() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fish conf.d directory with wt.fish
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let fish_config = conf_d.join("wt.fish");
    fs::write(
        &fish_config,
        "if type -q wt; command wt config shell init fish | source; end\n",
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("fish")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        ✅ Removed shell extension for [1mfish[0m @ [1m~/.config/fish/conf.d/wt.fish[0m

        ✅ Removed integration from 1 shell
        💡 [2mRestart shell to complete uninstall[0m

        ----- stderr -----
        ");
    });

    // Verify the fish config file was deleted
    assert!(!fish_config.exists(), "Fish config file should be deleted");
}

/// Test install and then uninstall roundtrip
#[test]
fn test_install_uninstall_roundtrip() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create initial config file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing config\nexport PATH=$HOME/bin:$PATH\n",
    )
    .unwrap();

    // First install
    {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        let output = cmd.output().expect("Failed to execute command");
        assert!(output.status.success(), "Install should succeed");
    }

    // Verify installed
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("wt config shell init zsh"));

    // Then uninstall
    {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("zsh")
            .arg("--force")
            .current_dir(repo.root_path());

        let output = cmd.output().expect("Failed to execute command");
        assert!(output.status.success(), "Uninstall should succeed");
    }

    // Verify uninstalled but other content preserved
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        !content.contains("wt config shell init"),
        "Integration should be removed"
    );
    assert!(
        content.contains("# Existing config"),
        "Comment should be preserved"
    );
    assert!(
        content.contains("export PATH=$HOME/bin:$PATH"),
        "PATH export should be preserved"
    );
}
