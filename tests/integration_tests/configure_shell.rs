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
        cmd.env("ZDOTDIR", temp_home.path()); // Point zsh to our test home for config
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
    use crate::common::{TestRepo, configure_pty_command, open_pty, repo, temp_home};
    use insta::assert_snapshot;
    use insta_cmd::get_cargo_bin;
    use portable_pty::CommandBuilder;
    use rstest::rstest;
    use std::fs;
    use std::io::{Read, Write};
    use tempfile::TempDir;

    /// Execute shell install command in a PTY with interactive input
    fn exec_install_in_pty(temp_home: &TempDir, repo: &TestRepo, input: &str) -> (String, i32) {
        let pair = open_pty();

        let mut cmd = CommandBuilder::new(get_cargo_bin("wt"));
        cmd.arg("-C");
        cmd.arg(repo.root_path());
        cmd.arg("config");
        cmd.arg("shell");
        cmd.arg("install");
        cmd.cwd(repo.root_path());

        configure_pty_command(&mut cmd);
        cmd.env("HOME", temp_home.path());
        cmd.env("SHELL", "/bin/zsh");
        // Skip the compinit probe and force the advisory to appear. The probe spawns
        // `zsh -ic` which triggers global zshrc configs that can produce "insecure
        // directories" warnings on some CI environments. These warnings go to /dev/tty
        // and leak into PTY output despite our ZSH_DISABLE_COMPFIX suppression.
        // Using MISSING=1 skips the probe while still showing the compinit advisory.
        cmd.env("WORKTRUNK_TEST_COMPINIT_MISSING", "1");

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();

        // Write input to the PTY (simulating user typing)
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
        drop(writer);

        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let exit_status = child.wait().unwrap();
        let exit_code = exit_status.exit_code() as i32;

        (buf, exit_code)
    }

    /// Normalize output for snapshot testing
    fn normalize_output(output: &str, temp_home: &TempDir) -> String {
        // PTYs use \r\n line endings, normalize to \n
        let output = output.replace("\r\n", "\n");

        // Remove ^D control sequence that appears on macOS
        let output = regex::Regex::new(r"\^D\x08+")
            .unwrap()
            .replace_all(&output, "");

        // Remove echoed user input at end of prompt line (PTY echo timing varies).
        // The prompt ends with [y/N/?][22m  and then the echoed input appears.
        // Normalize to just the prompt without echoed input.
        let output = regex::Regex::new(r"(\[y/N/\?\]\x1b\[22m) [yn]")
            .unwrap()
            .replace_all(&output, "$1 ");

        // Remove standalone echoed input lines (just y or n on their own line)
        // that appear due to PTY timing differences
        let output = regex::Regex::new(r"^[yn]\n").unwrap().replace(&output, "");

        // Normalize blank lines caused by PTY echo timing variations.
        // The newline from user input can appear at different positions relative
        // to the prompt text depending on the system. Collapse consecutive newlines
        // to a single newline, then strip any leading newline.
        let output = regex::Regex::new(r"\n{2,}")
            .unwrap()
            .replace_all(&output, "\n");
        let output = output.trim_start_matches('\n');

        // Replace temp home path with ~/
        let home_path = temp_home.path().to_string_lossy();
        output.replace(&*home_path, "~").to_string()
    }

    /// Test that `wt config shell install` shows preview with gutter-formatted config lines
    #[rstest]
    fn test_install_preview_with_gutter(repo: TestRepo, temp_home: TempDir) {
        // Create zsh config file
        let zshrc_path = temp_home.path().join(".zshrc");
        fs::write(&zshrc_path, "# Existing config\n").unwrap();

        let (output, exit_code) = exec_install_in_pty(&temp_home, &repo, "y\n");
        let normalized = normalize_output(&output, &temp_home);

        assert_eq!(exit_code, 0);
        assert_snapshot!(normalized);
    }

    /// Test that declining install shows preview but doesn't modify files
    #[rstest]
    fn test_install_preview_declined(repo: TestRepo, temp_home: TempDir) {
        let zshrc_path = temp_home.path().join(".zshrc");
        fs::write(&zshrc_path, "# Existing config\n").unwrap();

        let (output, exit_code) = exec_install_in_pty(&temp_home, &repo, "n\n");
        let normalized = normalize_output(&output, &temp_home);

        // User declined, so exit code is 1
        assert_eq!(exit_code, 1);
        assert_snapshot!(normalized);

        // Verify file was not modified
        let content = fs::read_to_string(&zshrc_path).unwrap();
        assert!(
            !content.contains("wt config shell init"),
            "File should not be modified when user declines"
        );
    }
}
