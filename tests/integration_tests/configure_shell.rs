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
        // Force compinit warning for deterministic tests across environments
        cmd.env("WT_ASSUME_NO_COMPINIT", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        🟡 [33mCompletions won't work; add to ~/.zshrc before the wt line:[39m
        [107m [0m  [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m[0m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mSkipped [90mbash[39m; ~/.bashrc not found[22m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m

        ✅ [32mConfigured 1 shell[39m
        💡 [2mRestart shell or run: source ~/.zshrc[22m
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
        // Force compinit warning for deterministic tests across environments
        cmd.env("WT_ASSUME_NO_COMPINIT", "1");
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

        ----- stderr -----
        🟡 [33mCompletions won't work; add to ~/.zshrc before the wt line:[39m
        [107m [0m  [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m[0m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m

        ✅ [32mConfigured 1 shell[39m
        💡 [2mRestart shell or run: source ~/.zshrc[22m
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

        ----- stderr -----
        ⚪ Already configured shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m
        ⚪ All shells already configured
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

        ----- stderr -----
        ✅ [32mCreated shell extension for [1mfish[22m @ [1m~/.config/fish/conf.d/wt.fish[22m[39m
        ✅ [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        ✅ [32mConfigured 1 shell[39m
        💡 [2mRestart shell to activate[22m
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

/// Test `wt config shell install` when fish extension already exists
/// Fish completions are now inline in the init script, so no separate file is needed
#[test]
fn test_configure_shell_fish_extension_exists() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fish conf.d directory with wt.fish (extension exists)
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let fish_config = conf_d.join("wt.fish");
    fs::write(
        &fish_config,
        "if type -q wt; command wt config shell init fish | source; end",
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
            .arg("install")
            .arg("fish")
            .arg("--force")
            .current_dir(repo.root_path());

        // Fish completions are inline in the init script, so when extension exists,
        // it should say "All shells already configured"
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ⚪ Already configured shell extension for [1mfish[22m @ [1m~/.config/fish/conf.d/wt.fish[22m
        ✅ [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        ✅ [32mConfigured 1 shell[39m
        ");
    });

    // Fish completions should be in a separate file with WORKTRUNK_BIN fallback
    let completions_file = temp_home.path().join(".config/fish/completions/wt.fish");
    assert!(
        completions_file.exists(),
        "Fish completions file should be created"
    );
    let contents = std::fs::read_to_string(&completions_file).unwrap();
    assert!(
        contents.contains(r#"test -n \"\$WORKTRUNK_BIN\""#),
        "Fish completions should check WORKTRUNK_BIN is non-empty with fallback"
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

        ----- stderr -----
        💡 [2mSkipped [90mbash[39m; ~/.bashrc not found[22m
        💡 [2mSkipped [90mzsh[39m; ~/.zshrc not found[22m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m
        ❌ [31mNo shell config files found[39m
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
        // Force compinit warning for deterministic tests across environments
        cmd.env("WT_ASSUME_NO_COMPINIT", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        🟡 [33mCompletions won't work; add to ~/.zshrc before the wt line:[39m
        [107m [0m  [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m[0m
        ✅ [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m

        ✅ [32mConfigured 2 shells[39m
        💡 [2mRestart shell or run: source ~/.zshrc[22m
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
        // Force compinit warning for deterministic tests across environments
        cmd.env("WT_ASSUME_NO_COMPINIT", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        🟡 [33mCompletions won't work; add to ~/.zshrc before the wt line:[39m
        [107m [0m  [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m[0m
        ⚪ Already configured shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m

        ✅ [32mConfigured 1 shell[39m
        💡 [2mRestart shell or run: source ~/.zshrc[22m
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

        ----- stderr -----
        ✅ [32mRemoved shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mNo [90mbash[39m shell extension & completions in ~/.bashrc[22m
        💡 [2mNo [90mfish[39m shell extension in ~/.config/fish/conf.d/wt.fish[22m
        💡 [2mNo [90mfish[39m completions in ~/.config/fish/completions/wt.fish[22m

        ✅ [32mRemoved integration from 1 shell[39m
        💡 [2mRestart shell to complete uninstall[22m
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

        ----- stderr -----
        ✅ [32mRemoved shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        ✅ [32mRemoved shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mNo [90mfish[39m shell extension in ~/.config/fish/conf.d/wt.fish[22m
        💡 [2mNo [90mfish[39m completions in ~/.config/fish/completions/wt.fish[22m

        ✅ [32mRemoved integration from 2 shells[39m
        💡 [2mRestart shell to complete uninstall[22m
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

        ----- stderr -----
        🟡 [33mNo shell extension & completions found in ~/.zshrc[39m
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

        ----- stderr -----
        ✅ [32mRemoved shell extension for [1mfish[22m @ [1m~/.config/fish/conf.d/wt.fish[22m[39m

        ✅ [32mRemoved integration from 1 shell[39m
        💡 [2mRestart shell to complete uninstall[22m
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

/// Test that compinit warning does NOT show when .zshrc has compinit enabled
#[test]
fn test_configure_shell_no_warning_when_compinit_enabled() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a .zshrc that enables compinit - detection should find it
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing config\nautoload -Uz compinit && compinit\n",
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.env("ZDOTDIR", temp_home.path()); // Point zsh to our test home for config
        cmd.env("WT_ASSUME_COMPINIT", "1"); // Bypass zsh subprocess check (unreliable on CI)
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

        ----- stderr -----
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m

        ✅ [32mConfigured 1 shell[39m
        💡 [2mRestart shell or run: source ~/.zshrc[22m
        ");
    });
}

/// Test that compinit warning does NOT show when $SHELL is bash (not a zsh user)
/// Even when installing all shells, we don't warn bash users about zsh compinit
#[test]
fn test_configure_shell_no_warning_for_bash_user() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create config files for both shells (no compinit in zshrc)
    let zshrc_path = temp_home.path().join(".zshrc");
    let bashrc_path = temp_home.path().join(".bashrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();
    fs::write(&bashrc_path, "# Existing bash config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/bash"); // User's primary shell is bash
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - user is a bash user, not zsh
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ✅ [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m

        ✅ [32mConfigured 2 shells[39m
        💡 [2mRestart shell or run: source ~/.bashrc[22m
        ");
    });
}

/// Test that compinit warning does NOT show when installing fish (even if SHELL=zsh)
/// Only `install zsh` or `install` (all) should trigger zsh-specific warnings
#[test]
fn test_configure_shell_no_warning_for_fish_install() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fish conf.d directory
    let fish_conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&fish_conf_d).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh"); // User is zsh user, but installing fish
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish") // Specifically installing fish, not zsh
            .arg("--force")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - we're installing fish, not zsh
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ✅ [32mCreated shell extension for [1mfish[22m @ [1m~/.config/fish/conf.d/wt.fish[22m[39m
        ✅ [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        ✅ [32mConfigured 1 shell[39m
        ");
    });
}

/// Test that compinit warning does NOT show when zsh is already configured
#[test]
fn test_configure_shell_no_warning_when_already_configured() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create a .zshrc that ALREADY has wt integration (no compinit)
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

        // Should NOT show compinit warning - zsh is AlreadyExists, not newly added
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ⚪ Already configured shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m
        ⚪ All shells already configured
        ");
    });
}

/// Test that compinit warning does NOT show when $SHELL is unset
#[test]
fn test_configure_shell_no_warning_when_shell_unset() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create zsh and bash config files (no compinit)
    let zshrc_path = temp_home.path().join(".zshrc");
    let bashrc_path = temp_home.path().join(".bashrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();
    fs::write(&bashrc_path, "# Existing bash config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env_remove("SHELL"); // Explicitly unset SHELL
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--force")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - can't determine user's shell
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ✅ [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        ✅ [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        💡 [2mSkipped [90mfish[39m; ~/.config/fish/conf.d not found[22m

        ✅ [32mConfigured 2 shells[39m
        ");
    });
}
