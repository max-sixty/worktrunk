use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_home_snapshot_settings, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

#[rstest]
fn test_configure_shell_with_yes(repo: TestRepo, temp_home: TempDir) {
    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mSkipped [90mbash[39m; [90m~/.bashrc[39m not found[22m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m

        [32m✓[39m [32mConfigured 1 shell[39m
        [33m▲[39m [33mCompletions require compinit; add to ~/.zshrc before the wt line:[39m
        [107m [0m [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt config shell init zsh)\""));
}

#[rstest]
fn test_configure_shell_specific_shell(repo: TestRepo, temp_home: TempDir) {
    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
        [33m▲[39m [33mCompletions require compinit; add to ~/.zshrc before the wt line:[39m
        [107m [0m [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });

    // Verify the file was modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(content.contains("eval \"$(command wt config shell init zsh)\""));
}

#[rstest]
fn test_configure_shell_already_exists(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m Already configured shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m
        [32m✓[39m [32mAll shells already configured[39m
        ");
    });

    // Verify the file was not modified (no duplicate)
    let content = fs::read_to_string(&zshrc_path).unwrap();
    let count = content.matches("wt config shell init").count();
    assert_eq!(count, 1, "Should only have one wt config shell init line");
}

#[rstest]
fn test_configure_shell_fish(repo: TestRepo, temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mCreated shell extension for [1mfish[22m @ [1m~/.config/fish/functions/wt.fish[22m[39m
        [32m✓[39m [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });

    // Verify the fish conf.d file was created
    let fish_config = temp_home.path().join(".config/fish/functions/wt.fish");
    assert!(fish_config.exists());

    let content = fs::read_to_string(&fish_config).unwrap();
    assert!(
        content.contains("function wt"),
        "Should contain function definition: {}",
        content
    );
}

/// Test install dry-run shows preview with gutter-formatted config content
#[rstest]
fn test_configure_shell_fish_dry_run(repo: TestRepo, temp_home: TempDir) {
    // Create fish functions directory (but no wt.fish - so it will be "created")
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        // Dry-run should show "Will create" and the actual content in gutter
        assert_cmd_snapshot!(cmd);
    });

    // Verify no files were actually created
    let fish_config = functions.join("wt.fish");
    assert!(
        !fish_config.exists(),
        "Dry-run should not create files: {:?}",
        fish_config
    );
}

/// Test that installing when extension exists shows "Already configured"
#[rstest]
fn test_configure_shell_fish_extension_exists(repo: TestRepo, temp_home: TempDir) {
    // Create fish functions directory with wt.fish (extension exists at new canonical location)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Fish shell extension exists but completions are in a separate file.
        // Shell extension shows as "Already configured", completions show as "Created".
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m Already configured shell extension for [1mfish[22m @ [1m~/.config/fish/functions/wt.fish[22m
        [32m✓[39m [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
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

#[rstest]
fn test_configure_shell_fish_all_already_configured(repo: TestRepo, temp_home: TempDir) {
    // Create fish functions directory with wt.fish (extension exists at new canonical location)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    // Also create completions file
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    let completions_file = completions_d.join("wt.fish");
    fs::write(&completions_file, "# existing completions").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Both extension and completions already exist
        assert_cmd_snapshot!(cmd);
    });
}

/// Test that installing fish shell integration cleans up legacy conf.d file
///
/// Before issue #566, fish integration was installed to conf.d/wt.fish.
/// Now it installs to functions/wt.fish. This test ensures we clean up the old location.
#[rstest]
fn test_configure_shell_fish_legacy_conf_d_cleanup(repo: TestRepo, temp_home: TempDir) {
    // Create legacy conf.d file (old location)
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    // Use realistic content with worktrunk marker so it's detected as worktrunk-managed
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should create new file and clean up legacy
        assert_cmd_snapshot!(cmd);
    });

    // Verify new location exists
    let new_file = temp_home.path().join(".config/fish/functions/wt.fish");
    assert!(
        new_file.exists(),
        "Should create functions/wt.fish: {:?}",
        new_file
    );

    // Verify legacy location was cleaned up
    assert!(
        !legacy_file.exists(),
        "Should remove legacy conf.d/wt.fish: {:?}",
        legacy_file
    );
}

/// Test that legacy cleanup happens even when new file already exists with correct content
///
/// This handles the case where:
/// 1. User had old conf.d/wt.fish (pre-#566)
/// 2. User manually created functions/wt.fish with correct content
/// 3. User runs `wt config shell install fish`
///
/// The legacy file should still be cleaned up even though install reports "Already configured"
#[rstest]
fn test_configure_shell_fish_legacy_cleanup_even_when_already_exists(
    repo: TestRepo,
    temp_home: TempDir,
) {
    // Create functions/wt.fish with the EXACT content that install would create
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let new_file = functions.join("wt.fish");
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&new_file, format!("{}\n", wrapper_content)).unwrap();

    // Also create completions (so it reports "all already configured")
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    fs::write(completions_d.join("wt.fish"), "# completions").unwrap();

    // Create legacy conf.d file (old location that should be cleaned up)
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    // Use realistic content with worktrunk marker so it's detected as worktrunk-managed
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should report "Already configured" but still clean up legacy
        assert_cmd_snapshot!(cmd);
    });

    // The key assertion: legacy file should be removed even though new file already existed
    assert!(
        !legacy_file.exists(),
        "Should remove legacy conf.d/wt.fish even when functions/wt.fish already exists: {:?}",
        legacy_file
    );

    // New file should still exist
    assert!(
        new_file.exists(),
        "Should preserve existing functions/wt.fish: {:?}",
        new_file
    );
}

/// Test that uninstalling fish shell integration also cleans up legacy conf.d file
///
/// If a user has the old conf.d/wt.fish file, uninstall should remove it too.
#[rstest]
fn test_uninstall_shell_fish_legacy_conf_d_cleanup(repo: TestRepo, temp_home: TempDir) {
    // Create both new location (functions) and legacy location (conf.d)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let new_file = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&new_file, format!("{}\n", wrapper_content)).unwrap();

    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    // Legacy content from main branch
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    // Also create completions
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    let completions_file = completions_d.join("wt.fish");
    fs::write(&completions_file, "# completions").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should remove both new and legacy files
        assert_cmd_snapshot!(cmd);
    });

    // Verify both locations were cleaned up
    assert!(
        !new_file.exists(),
        "Should remove functions/wt.fish: {:?}",
        new_file
    );
    assert!(
        !legacy_file.exists(),
        "Should remove legacy conf.d/wt.fish: {:?}",
        legacy_file
    );
    assert!(
        !completions_file.exists(),
        "Should remove completions/wt.fish: {:?}",
        completions_file
    );
}

/// Test that --dry-run does NOT delete legacy fish conf.d file
///
/// Regression test: Previously, --dry-run could delete the legacy file because
/// cleanup ran before the dry_run check. This must never happen.
#[rstest]
fn test_configure_shell_fish_dry_run_does_not_delete_legacy(repo: TestRepo, temp_home: TempDir) {
    // Create functions/wt.fish with correct content (already configured)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let new_file = functions.join("wt.fish");
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&new_file, format!("{}\n", wrapper_content)).unwrap();

    // Create completions (so it reports "all already configured")
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    fs::write(completions_d.join("wt.fish"), "# completions").unwrap();

    // Create legacy conf.d file that should NOT be deleted in dry-run mode
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // CRITICAL: Legacy file must still exist after --dry-run
    assert!(
        legacy_file.exists(),
        "--dry-run must NOT delete legacy conf.d/wt.fish: {:?}",
        legacy_file
    );

    // New file should still exist too
    assert!(
        new_file.exists(),
        "functions/wt.fish should be preserved: {:?}",
        new_file
    );
}

/// Test that detection finds fish integration in legacy conf.d location
///
/// `wt config show` should detect shell integration whether it's in the
/// old conf.d location or the new functions location.
#[rstest]
fn test_config_show_detects_fish_legacy_conf_d(mut repo: TestRepo, temp_home: TempDir) {
    // Create ONLY the legacy conf.d file (simulating user who installed before #566)
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    // Write content that matches our detection pattern (old-style init sourcing)
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    // Mock claude as not found (consistent across environments)
    repo.setup_mock_ci_tools_unauthenticated();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config").arg("show").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test config show when functions/ exists but wt.fish doesn't, with legacy conf.d
///
/// This tests a different code path than test_config_show_detects_fish_legacy_conf_d:
/// - That test: functions/ doesn't exist -> fish is "skipped"
/// - This test: functions/ exists but empty -> fish is "configured" with WouldCreate
///
/// Both should show the migration hint for the legacy conf.d location.
#[rstest]
fn test_config_show_fish_legacy_with_functions_dir(mut repo: TestRepo, temp_home: TempDir) {
    // Create functions/ directory (empty - no wt.fish)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();

    // Create legacy conf.d file
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let legacy_file = conf_d.join("wt.fish");
    fs::write(&legacy_file, "wt config shell init fish | source").unwrap();

    // Mock claude as not found
    repo.setup_mock_ci_tools_unauthenticated();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config").arg("show").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_configure_shell_no_files(repo: TestRepo, temp_home: TempDir) {
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        [2m↳[22m [2mSkipped [90mbash[39m; [90m~/.bashrc[39m not found[22m
        [2m↳[22m [2mSkipped [90mzsh[39m; [90m~/.zshrc[39m not found[22m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m
        [31m✗[39m [31mNo shell config files found[39m
        ");
    });
}

#[rstest]
fn test_configure_shell_multiple_configs(repo: TestRepo, temp_home: TempDir) {
    // Create multiple shell config files
    let bash_config_path = temp_home.path().join(".bashrc");
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&bash_config_path, "# Existing bash config\n").unwrap();
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m

        [32m✓[39m [32mConfigured 2 shells[39m
        [33m▲[39m [33mCompletions require compinit; add to ~/.zshrc before the wt line:[39m
        [107m [0m [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
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

#[rstest]
fn test_configure_shell_mixed_states(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m Already configured shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m

        [32m✓[39m [32mConfigured 1 shell[39m
        [33m▲[39m [33mCompletions require compinit; add to ~/.zshrc before the wt line:[39m
        [107m [0m [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
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

#[rstest]
fn test_uninstall_shell(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mRemoved shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mNo [90mbash[39m shell extension & completions in ~/.bashrc[22m
        [2m↳[22m [2mNo [90mfish[39m shell extension in ~/.config/fish/functions/wt.fish[22m
        [2m↳[22m [2mNo [90mnu[39m shell extension in ~/.config/nushell/vendor/autoload/wt.nu[22m
        [2m↳[22m [2mNo [90mfish[39m completions in ~/.config/fish/completions/wt.fish[22m

        [32m✓[39m [32mRemoved integration from 1 shell[39m
        [2m↳[22m [2mRestart shell to complete uninstall[22m
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

#[rstest]
fn test_uninstall_shell_multiple(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mRemoved shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        [32m✓[39m [32mRemoved shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mNo [90mfish[39m shell extension in ~/.config/fish/functions/wt.fish[22m
        [2m↳[22m [2mNo [90mnu[39m shell extension in ~/.config/nushell/vendor/autoload/wt.nu[22m
        [2m↳[22m [2mNo [90mfish[39m completions in ~/.config/fish/completions/wt.fish[22m

        [32m✓[39m [32mRemoved integration from 2 shells[39m
        [2m↳[22m [2mRestart shell to complete uninstall[22m
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

#[rstest]
fn test_uninstall_shell_not_found(repo: TestRepo, temp_home: TempDir) {
    // Create a fake .zshrc file without wt integration
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("zsh")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [33m▲[39m [33mNo shell extension & completions found in ~/.zshrc[39m
        ");
    });
}

#[rstest]
fn test_uninstall_shell_fish(repo: TestRepo, temp_home: TempDir) {
    // Create fish functions directory with wt.fish (new canonical location)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("fish")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mRemoved shell extension for [1mfish[22m @ [1m~/.config/fish/functions/wt.fish[22m[39m

        [32m✓[39m [32mRemoved integration from 1 shell[39m
        [2m↳[22m [2mRestart shell to complete uninstall[22m
        ");
    });

    // Verify the fish config file was deleted
    assert!(!fish_config.exists());
}

#[rstest]
fn test_install_uninstall_roundtrip(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--yes")
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("zsh")
            .arg("--yes")
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

#[rstest]
fn test_install_uninstall_no_blank_line_accumulation(repo: TestRepo, temp_home: TempDir) {
    // Create initial config file matching the user's real zshrc structure
    let zshrc_path = temp_home.path().join(".zshrc");
    let initial_content =
        "[ -f ~/.fzf.zsh ] && source ~/.fzf.zsh\n\nautoload -Uz compinit && compinit\n";
    fs::write(&zshrc_path, initial_content).unwrap();

    // Install
    {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.args(["config", "shell", "install", "zsh", "--yes"]);
        cmd.current_dir(repo.root_path());
        let output = cmd.output().expect("Failed to execute command");
        assert!(output.status.success(), "Install should succeed");
    }

    let after_install = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        after_install.contains("wt config shell init zsh"),
        "Integration should be added"
    );

    // Uninstall
    {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.args(["config", "shell", "uninstall", "zsh", "--yes"]);
        cmd.current_dir(repo.root_path());
        let output = cmd.output().expect("Failed to execute command");
        assert!(output.status.success(), "Uninstall should succeed");
    }

    let after_uninstall = fs::read_to_string(&zshrc_path).unwrap();
    assert_eq!(
        initial_content, after_uninstall,
        "Uninstall should restore original content"
    );

    // Re-install
    {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.args(["config", "shell", "install", "zsh", "--yes"]);
        cmd.current_dir(repo.root_path());
        let output = cmd.output().expect("Failed to execute command");
        assert!(output.status.success(), "Re-install should succeed");
    }

    let after_reinstall = fs::read_to_string(&zshrc_path).unwrap();

    // Key assertion: re-install should produce the same result as initial install
    // (no accumulation of blank lines)
    assert_eq!(
        after_install, after_reinstall,
        "Re-install should produce same result as initial install.\n\
         After first install:\n{after_install}\n---\n\
         After uninstall:\n{after_uninstall}\n---\n\
         After re-install:\n{after_reinstall}"
    );
}

#[rstest]
fn test_configure_shell_no_warning_when_compinit_enabled(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Canonicalize to handle macOS /var -> /private/var symlinks
        cmd.env("ZDOTDIR", crate::common::canonicalize(temp_home.path()).unwrap_or_else(|_| temp_home.path().to_path_buf()));
        cmd.env("WORKTRUNK_TEST_COMPINIT_CONFIGURED", "1"); // Bypass zsh subprocess check (unreliable on CI)
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });
}

/// Even when installing all shells, we don't warn bash users about zsh compinit
#[rstest]
fn test_configure_shell_no_warning_for_bash_user(repo: TestRepo, temp_home: TempDir) {
    // Create config files for both shells (no compinit in zshrc)
    let zshrc_path = temp_home.path().join(".zshrc");
    let bashrc_path = temp_home.path().join(".bashrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();
    fs::write(&bashrc_path, "# Existing bash config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/bash"); // User's primary shell is bash
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - user is a bash user, not zsh
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m

        [32m✓[39m [32mConfigured 2 shells[39m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });
}

/// Test that explicitly targeting a shell creates the config file when it doesn't exist
#[rstest]
fn test_configure_shell_create_zshrc_when_missing(repo: TestRepo, temp_home: TempDir) {
    // Don't create .zshrc - it doesn't exist
    let zshrc_path = temp_home.path().join(".zshrc");
    assert!(!zshrc_path.exists(), "zshrc should not exist before test");

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Force compinit warning for deterministic tests across environments
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh") // Explicitly target zsh
            .arg("--yes")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mCreated shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
        [33m▲[39m [33mCompletions require compinit; add to ~/.zshrc before the wt line:[39m
        [107m [0m [2m[0m[2m[34mautoload[0m[2m [0m[2m[36m-Uz[0m[2m compinit [0m[2m[36m&&[0m[2m [0m[2m[34mcompinit[0m[2m
        [2m↳[22m [2mRestart shell to activate shell integration[22m
        ");
    });

    // Verify the file was created with correct content
    assert!(zshrc_path.exists(), "zshrc should exist after install");
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        content.contains("eval \"$(command wt config shell init zsh)\""),
        "Created file should contain wt integration: {}",
        content
    );
}

/// Only `install zsh` or `install` (all) should trigger zsh-specific warnings
#[rstest]
fn test_configure_shell_no_warning_for_fish_install(repo: TestRepo, temp_home: TempDir) {
    // Create fish conf.d directory
    let fish_conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&fish_conf_d).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh"); // User is zsh user, but installing fish
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("fish") // Specifically installing fish, not zsh
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - we're installing fish, not zsh
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mCreated shell extension for [1mfish[22m @ [1m~/.config/fish/functions/wt.fish[22m[39m
        [32m✓[39m [32mCreated completions for [1mfish[22m @ [1m~/.config/fish/completions/wt.fish[22m[39m

        [32m✓[39m [32mConfigured 1 shell[39m
        ");
    });
}

#[rstest]
fn test_configure_shell_no_warning_when_already_configured(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - zsh is AlreadyExists, not newly added
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m Already configured shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m
        [32m✓[39m [32mAll shells already configured[39m
        ");
    });
}

#[rstest]
fn test_configure_shell_no_warning_when_shell_unset(repo: TestRepo, temp_home: TempDir) {
    // Create zsh and bash config files (no compinit)
    let zshrc_path = temp_home.path().join(".zshrc");
    let bashrc_path = temp_home.path().join(".bashrc");
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();
    fs::write(&bashrc_path, "# Existing bash config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env_remove("SHELL"); // Explicitly unset SHELL
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--yes")
            .current_dir(repo.root_path());

        // Should NOT show compinit warning - can't determine user's shell
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mAdded shell extension & completions for [1mbash[22m @ [1m~/.bashrc[22m[39m
        [32m✓[39m [32mAdded shell extension & completions for [1mzsh[22m @ [1m~/.zshrc[22m[39m
        [2m↳[22m [2mSkipped [90mfish[39m; [90m~/.config/fish/functions[39m not found[22m
        [2m↳[22m [2mSkipped [90mnu[39m; [90m~/.config/nushell/vendor/autoload[39m not found[22m

        [32m✓[39m [32mConfigured 2 shells[39m
        ");
    });
}

#[rstest]
fn test_configure_shell_dry_run(repo: TestRepo, temp_home: TempDir) {
    // Create a fake .zshrc file
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&zshrc_path, "# Existing config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify the file was NOT modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        !content.contains("wt config shell init"),
        "File should not be modified with --dry-run"
    );
    assert_eq!(content, "# Existing config\n", "File should be unchanged");
}

#[rstest]
fn test_configure_shell_dry_run_multiple(repo: TestRepo, temp_home: TempDir) {
    // Create multiple shell config files
    let bash_config_path = temp_home.path().join(".bashrc");
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(&bash_config_path, "# Existing bash config\n").unwrap();
    fs::write(&zshrc_path, "# Existing zsh config\n").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify no files were modified
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    assert!(
        !bash_content.contains("wt config shell init"),
        "Bash config should not be modified with --dry-run"
    );
    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        !zsh_content.contains("wt config shell init"),
        "Zsh config should not be modified with --dry-run"
    );
}

#[rstest]
fn test_configure_shell_dry_run_already_configured(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("install")
            .arg("zsh")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        // Already configured - nothing to preview
        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_uninstall_shell_dry_run(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("zsh")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify the file was NOT modified
    let content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        content.contains("wt config shell init"),
        "File should not be modified with --dry-run"
    );
}

/// Test dry-run with fish in legacy conf.d location (shows deprecated message)
#[rstest]
fn test_uninstall_shell_dry_run_fish(repo: TestRepo, temp_home: TempDir) {
    // Create fish conf.d directory with wt.fish and completions
    let conf_d = temp_home.path().join(".config/fish/conf.d");
    fs::create_dir_all(&conf_d).unwrap();
    let fish_config = conf_d.join("wt.fish");
    fs::write(
        &fish_config,
        "if type -q wt; command wt config shell init fish | source; end\n",
    )
    .unwrap();

    // Create completions file
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    let completions_file = completions_d.join("wt.fish");
    fs::write(&completions_file, "# fish completions").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("fish")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify files were NOT modified
    assert!(fish_config.exists(), "Fish config should still exist");
    assert!(
        completions_file.exists(),
        "Fish completions should still exist"
    );
}

/// Test dry-run with fish in canonical functions/ location (shows normal message)
#[rstest]
fn test_uninstall_shell_dry_run_fish_canonical(repo: TestRepo, temp_home: TempDir) {
    // Create fish functions directory with wt.fish (canonical location)
    let functions = temp_home.path().join(".config/fish/functions");
    fs::create_dir_all(&functions).unwrap();
    let fish_config = functions.join("wt.fish");
    // Write the exact wrapper content that install would create
    let init =
        worktrunk::shell::ShellInit::with_prefix(worktrunk::shell::Shell::Fish, "wt".to_string());
    let wrapper_content = init.generate_fish_wrapper().unwrap();
    fs::write(&fish_config, format!("{}\n", wrapper_content)).unwrap();

    // Create completions file
    let completions_d = temp_home.path().join(".config/fish/completions");
    fs::create_dir_all(&completions_d).unwrap();
    let completions_file = completions_d.join("wt.fish");
    fs::write(&completions_file, "# fish completions").unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/fish");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("fish")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify files were NOT modified
    assert!(fish_config.exists(), "Fish config should still exist");
    assert!(
        completions_file.exists(),
        "Fish completions should still exist"
    );
}

#[rstest]
fn test_uninstall_shell_dry_run_multiple(repo: TestRepo, temp_home: TempDir) {
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
        repo.configure_wt_cmd(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        cmd.arg("config")
            .arg("shell")
            .arg("uninstall")
            .arg("--dry-run")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify no files were modified
    let bash_content = fs::read_to_string(&bash_config_path).unwrap();
    assert!(
        bash_content.contains("wt config shell init"),
        "Bash config should not be modified with --dry-run"
    );
    let zsh_content = fs::read_to_string(&zshrc_path).unwrap();
    assert!(
        zsh_content.contains("wt config shell init"),
        "Zsh config should not be modified with --dry-run"
    );
}

// PTY-based tests for interactive install preview
#[cfg(all(unix, feature = "shell-integration-tests"))]
mod pty_tests {
    use crate::common::pty::exec_cmd_in_pty_prompted;
    use crate::common::{
        TestRepo, add_pty_filters, configure_pty_command, repo, temp_home, wt_bin,
    };
    use insta::assert_snapshot;
    use portable_pty::CommandBuilder;
    use rstest::rstest;
    use std::fs;
    use tempfile::TempDir;

    /// Execute shell install command in a PTY, waiting for prompt before input
    fn exec_install_in_pty(temp_home: &TempDir, repo: &TestRepo, input: &str) -> (String, i32) {
        let mut cmd = CommandBuilder::new(wt_bin());
        cmd.arg("-C");
        cmd.arg(repo.root_path());
        cmd.arg("config");
        cmd.arg("shell");
        cmd.arg("install");
        cmd.cwd(repo.root_path());

        configure_pty_command(&mut cmd);
        cmd.env("HOME", temp_home.path());
        cmd.env("XDG_CONFIG_HOME", temp_home.path().join(".config"));
        cmd.env("WORKTRUNK_TEST_NUSHELL_ENV", "0");
        cmd.env("SHELL", "/bin/zsh");
        // Skip the compinit probe and force the advisory to appear. The probe spawns
        // `zsh -ic` which triggers global zshrc configs that can produce "insecure
        // directories" warnings on some CI environments. These warnings go to /dev/tty
        // and leak into PTY output despite our ZSH_DISABLE_COMPFIX suppression.
        // Using MISSING=1 skips the probe while still showing the compinit advisory.
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");

        exec_cmd_in_pty_prompted(cmd, &[input], "[y/N")
    }

    /// Create insta settings for install PTY tests.
    fn install_pty_settings(temp_home: &TempDir) -> insta::Settings {
        let mut settings = insta::Settings::clone_current();

        // Add PTY filters (CRLF, ^D, leading ANSI resets)
        add_pty_filters(&mut settings);

        // Remove echoed user input at end of prompt line (PTY echo timing varies).
        // The prompt ends with [y/N/?] (possibly with ANSI codes) and then the echoed input appears.
        settings.add_filter(r"(\[y/N/\?\](?:\x1b\[22m)?) [yn]", "$1 ");

        // Remove standalone echoed input lines (just y or n on their own line)
        settings.add_filter(r"^[yn]\n", "");

        // Replace temp home path with ~/
        settings.add_filter(&regex::escape(&temp_home.path().to_string_lossy()), "~");

        settings
    }

    /// Test that `wt config shell install` shows preview with gutter-formatted config lines
    #[rstest]
    fn test_install_preview_with_gutter(repo: TestRepo, temp_home: TempDir) {
        // Create zsh config file
        let zshrc_path = temp_home.path().join(".zshrc");
        fs::write(&zshrc_path, "# Existing config\n").unwrap();

        let (output, exit_code) = exec_install_in_pty(&temp_home, &repo, "y\n");

        assert_eq!(exit_code, 0);
        install_pty_settings(&temp_home).bind(|| {
            assert_snapshot!(output.trim_start_matches('\n'));
        });
    }

    /// Test that declining install shows preview but doesn't modify files
    #[rstest]
    fn test_install_preview_declined(repo: TestRepo, temp_home: TempDir) {
        let zshrc_path = temp_home.path().join(".zshrc");
        fs::write(&zshrc_path, "# Existing config\n").unwrap();

        let (output, exit_code) = exec_install_in_pty(&temp_home, &repo, "n\n");

        // User declined, so exit code is 1
        assert_eq!(exit_code, 1);
        install_pty_settings(&temp_home).bind(|| {
            assert_snapshot!(output.trim_start_matches('\n'));
        });

        // Verify file was not modified
        let content = fs::read_to_string(&zshrc_path).unwrap();
        assert!(
            !content.contains("wt config shell init"),
            "File should not be modified when user declines"
        );
    }
}

/// Test installing nushell shell integration
///
/// Runs `install nu --yes` and verifies the wrapper file was created.
/// This covers the nushell-specific wrapper generation path in configure_shell.
///
/// set_temp_home_env sets XDG_CONFIG_HOME to home/.config, and the `nu` binary
/// isn't available in tests, so nushell_config_dir falls back to XDG_CONFIG_HOME/nushell.
#[rstest]
fn test_configure_shell_nushell(repo: TestRepo, temp_home: TempDir) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("SHELL", "/bin/nu");
    cmd.arg("config")
        .arg("shell")
        .arg("install")
        .arg("nu")
        .arg("--yes")
        .current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to execute command");
    assert!(
        output.status.success(),
        "Install should succeed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Created shell extension for") && stderr.contains("nu"),
        "Output should show nushell was created:\n{}",
        stderr
    );

    // set_temp_home_env sets XDG_CONFIG_HOME → home/.config, so the nushell
    // vendor autoload path is deterministic (nu binary not available in tests).
    let home = std::fs::canonicalize(temp_home.path()).unwrap();
    let nu_config = home
        .join(".config")
        .join("nushell")
        .join("vendor")
        .join("autoload")
        .join("wt.nu");
    assert!(
        nu_config.exists(),
        "wt.nu should be created at {:?}",
        nu_config
    );

    let content = fs::read_to_string(&nu_config).unwrap();
    assert!(
        content.contains("def --env --wrapped wt"),
        "Should contain nushell function definition: {}",
        content
    );
}

/// Test uninstalling nushell shell integration
///
/// Installs nushell integration first, then uninstalls it.
/// This covers the nushell-specific uninstall block in configure_shell.
#[rstest]
fn test_uninstall_shell_nushell(repo: TestRepo, temp_home: TempDir) {
    let home = std::fs::canonicalize(temp_home.path()).unwrap();
    let nu_config = home
        .join(".config")
        .join("nushell")
        .join("vendor")
        .join("autoload")
        .join("wt.nu");

    // First install to create the wrapper file
    let mut install_cmd = wt_command();
    repo.configure_wt_cmd(&mut install_cmd);
    set_temp_home_env(&mut install_cmd, temp_home.path());
    install_cmd.env("SHELL", "/bin/nu");
    install_cmd
        .args(["config", "shell", "install", "nu", "--yes"])
        .current_dir(repo.root_path());

    let install_output = install_cmd.output().expect("Failed to execute install");
    assert!(
        install_output.status.success(),
        "Install should succeed:\nstderr: {}",
        String::from_utf8_lossy(&install_output.stderr)
    );
    assert!(
        nu_config.exists(),
        "wt.nu should exist after install at {:?}",
        nu_config
    );

    // Now uninstall
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.env("SHELL", "/bin/nu");
    cmd.args(["config", "shell", "uninstall", "nu", "--yes"])
        .current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to execute uninstall");
    assert!(
        output.status.success(),
        "Uninstall should succeed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Removed shell extension for") && stderr.contains("nu"),
        "Output should show nushell was removed:\n{}",
        stderr
    );

    // Verify the nushell config file was deleted
    assert!(
        !nu_config.exists(),
        "wt.nu should be deleted after uninstall: {:?}",
        nu_config
    );
}

/// Test that nushell uninstall cleans up config files at all candidate locations.
///
/// Exercises the fix where uninstall iterates all nushell config candidates,
/// not just the first. Simulates the scenario where the file was installed at
/// a location that is no longer the primary candidate (e.g., `nu` reported
/// a different path during install than what we'd pick now).
#[rstest]
fn test_uninstall_nushell_cleans_all_candidate_locations(repo: TestRepo, temp_home: TempDir) {
    let home = std::fs::canonicalize(temp_home.path()).unwrap();

    // Install nushell integration normally (goes to XDG_CONFIG_HOME/nushell)
    let mut install_cmd = wt_command();
    repo.configure_wt_cmd(&mut install_cmd);
    set_temp_home_env(&mut install_cmd, temp_home.path());
    install_cmd.env("SHELL", "/bin/nu");
    install_cmd
        .args(["config", "shell", "install", "nu", "--yes"])
        .current_dir(repo.root_path());

    let install_output = install_cmd.output().expect("Failed to execute install");
    assert!(
        install_output.status.success(),
        "Install should succeed:\nstderr: {}",
        String::from_utf8_lossy(&install_output.stderr)
    );

    let primary_config = home
        .join(".config")
        .join("nushell")
        .join("vendor")
        .join("autoload")
        .join("wt.nu");
    assert!(primary_config.exists(), "Primary config should exist");

    // Copy the config to a secondary candidate location (~/.config is the XDG default,
    // but also manually create one at a different path to simulate install at a
    // non-primary location). Use a custom XDG_CONFIG_HOME to make a second candidate
    // be the primary during uninstall.
    let secondary_dir = home
        .join("custom-config")
        .join("nushell")
        .join("vendor")
        .join("autoload");
    fs::create_dir_all(&secondary_dir).unwrap();
    let secondary_config = secondary_dir.join("wt.nu");
    fs::copy(&primary_config, &secondary_config).unwrap();

    // Uninstall with XDG_CONFIG_HOME pointing to the custom location.
    // The custom path becomes the first candidate, but the original at ~/.config/nushell
    // should also be cleaned up since uninstall checks all candidates.
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    // Override XDG_CONFIG_HOME to point to custom dir, making it the primary candidate
    cmd.env("HOME", &home);
    cmd.env("USERPROFILE", &home);
    cmd.env("XDG_CONFIG_HOME", home.join("custom-config"));
    cmd.env("APPDATA", home.join("custom-config"));
    cmd.env("SHELL", "/bin/nu");
    cmd.args(["config", "shell", "uninstall", "nu", "--yes"])
        .current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to execute uninstall");
    assert!(
        output.status.success(),
        "Uninstall should succeed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Both locations should be cleaned up
    assert!(
        !primary_config.exists(),
        "Primary config at ~/.config/nushell should be deleted: {primary_config:?}"
    );
    assert!(
        !secondary_config.exists(),
        "Secondary config at custom XDG path should be deleted: {secondary_config:?}"
    );
}

/// Test that WORKTRUNK_TEST_POWERSHELL_ENV=1 triggers PowerShell auto-detection.
/// This simulates the Windows behavior where we detect PowerShell when SHELL is not set.
#[rstest]
#[cfg_attr(
    windows,
    ignore = "Windows uses Documents folder which can't be easily overridden"
)]
fn test_powershell_env_detection(repo: TestRepo, temp_home: TempDir) {
    // Create the PowerShell config directory (Unix: ~/.config/powershell)
    // Note: On Windows, PowerShell uses Documents/ which dirs::document_dir() returns.
    // This test only runs on Unix where we can control the path via HOME.
    let powershell_dir = temp_home.path().join(".config/powershell");
    fs::create_dir_all(&powershell_dir).unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    // Force PowerShell detection via test env var
    cmd.env("WORKTRUNK_TEST_POWERSHELL_ENV", "1");
    // Set SHELL to something non-PowerShell to ensure we're testing the override
    cmd.env("SHELL", "/bin/bash");
    cmd.arg("config")
        .arg("shell")
        .arg("install")
        .arg("--yes")
        .current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to execute command");
    assert!(output.status.success(), "Command should succeed");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Check that PowerShell was configured (not skipped)
    assert!(
        stderr.contains("Created shell extension for") && stderr.contains("powershell"),
        "Output should show PowerShell was created:\n{}",
        stderr
    );

    // Verify the PowerShell profile was created
    let profile_path = powershell_dir.join("Microsoft.PowerShell_profile.ps1");
    assert!(
        profile_path.exists(),
        "PowerShell profile should be created at {:?}",
        profile_path
    );

    let content = fs::read_to_string(&profile_path).unwrap();
    assert!(
        content.contains("wt config shell init powershell"),
        "Profile should contain shell init: {}",
        content
    );
}

/// Test that nushell gets auto-configured when detected, even without vendor/autoload dir.
///
/// Parallels test_powershell_env_detection: when nushell is detected on the system,
/// `wt config shell install` should create vendor/autoload/ and install the wrapper,
/// rather than skipping with "vendor/autoload not found".
#[rstest]
fn test_nushell_auto_detection_creates_vendor_autoload(repo: TestRepo, temp_home: TempDir) {
    // Don't create vendor/autoload - the whole point is that it doesn't exist yet
    // but nushell IS detected on the system

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    set_temp_home_env(&mut cmd, temp_home.path());
    // Force nushell detection via test env var (parallels WORKTRUNK_TEST_POWERSHELL_ENV)
    cmd.env("WORKTRUNK_TEST_NUSHELL_ENV", "1");
    cmd.env("SHELL", "/bin/zsh");
    cmd.arg("config")
        .arg("shell")
        .arg("install")
        .arg("--yes")
        .current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to execute command");
    assert!(
        output.status.success(),
        "Command should succeed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Nushell should be configured, not skipped
    assert!(
        stderr.contains("Created shell extension for") && stderr.contains("nu"),
        "Nushell should be auto-configured when detected:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Skipped nu"),
        "Nushell should not be skipped when detected:\n{}",
        stderr
    );

    // Verify the nushell wrapper was created with vendor/autoload/ directory
    let home = std::fs::canonicalize(temp_home.path()).unwrap();
    let nu_config = home
        .join(".config")
        .join("nushell")
        .join("vendor")
        .join("autoload")
        .join("wt.nu");
    assert!(
        nu_config.exists(),
        "wt.nu should be created at {:?}",
        nu_config
    );

    let content = fs::read_to_string(&nu_config).unwrap();
    assert!(
        content.contains("def --env --wrapped wt"),
        "Should contain nushell function definition: {}",
        content
    );
}
