//! Tests for the shell integration first-run prompt
//!
//! These tests verify that `prompt_shell_integration` behaves correctly across scenarios:
//! - Skips when shell integration is active (WORKTRUNK_DIRECTIVE_FILE set)
//! - Skips when already prompted (config flag true)
//! - Skips when already installed (config line exists in shell config)
//! - Shows hint when not a TTY (non-interactive)
//! - Prompts and respects user's choice in interactive mode

use crate::common::{TestRepo, repo};
use rstest::rstest;
use std::fs;
use worktrunk::config::WorktrunkConfig;

/// Test that switch with active shell integration doesn't trigger prompt
///
/// When WORKTRUNK_DIRECTIVE_FILE is set (shell integration active), we should:
/// 1. Never call prompt_shell_integration()
/// 2. Have zero overhead from the prompt feature
#[rstest]
fn test_switch_with_active_shell_integration_no_prompt(repo: TestRepo) {
    // Create a worktree first
    let create_output = repo
        .wt_command()
        .args(["switch", "--create", "feature"])
        .output()
        .unwrap();
    assert!(
        create_output.status.success(),
        "First switch should succeed: {}",
        String::from_utf8_lossy(&create_output.stderr)
    );

    // Now switch with shell integration "active" (directive file set)
    // The directive file must exist (shell wrapper creates it before calling wt)
    let directive_file = repo.root_path().join("directive.txt");
    fs::write(&directive_file, "").unwrap();
    let mut cmd = repo.wt_command();
    cmd.env("WORKTRUNK_DIRECTIVE_FILE", &directive_file);

    let output = cmd.args(["switch", "feature"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Switch should succeed.\nstderr: {stderr}\nstdout: {stdout}"
    );

    // The directive file should have cd command (shell integration active)
    let directive = fs::read_to_string(&directive_file).unwrap_or_default();
    assert!(
        directive.contains("cd "),
        "Directive should contain cd command when shell integration active"
    );

    // No install prompt in output (would contain "Install shell integration")
    assert!(
        !stderr.contains("Install shell integration"),
        "Should not show install prompt when shell integration active: {stderr}"
    );
}

/// Test that already-prompted flag prevents prompt
#[rstest]
fn test_switch_with_skip_prompt_flag(repo: TestRepo) {
    // Set the skip flag in config
    let config_path = repo.test_config_path();
    let mut config = WorktrunkConfig::default();
    config.skip_shell_integration_prompt = true;
    config.save_to(config_path).unwrap();

    let output = repo
        .wt_command()
        .args(["switch", "--create", "feature"])
        .output()
        .unwrap();

    assert!(output.status.success(), "Switch should succeed");

    // No install prompt in output
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Install shell integration"),
        "Should not show install prompt when already prompted: {stderr}"
    );
}

/// Test that non-TTY stdin shows hint but doesn't prompt
///
/// When stdin is not a TTY (e.g., piped input), we should:
/// - Skip the prompt (can't interact)
/// - Always show the hint (not just first run)
/// - NOT mark as prompted (hints are not prompts)
#[rstest]
fn test_switch_non_tty_shows_hint(repo: TestRepo) {
    use std::process::Stdio;

    // Run with piped stdin (not a TTY)
    let output = repo
        .wt_command()
        .args(["switch", "--create", "feature"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success(), "Switch should succeed");

    // Verify the switch succeeded without prompting
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Created new worktree for"),
        "Should create worktree: {stderr}"
    );

    // Should show hint
    assert!(
        stderr.contains("wt config shell install"),
        "Should show install hint: {stderr}"
    );

    // Config should NOT have skip_shell_integration_prompt set (hints are not prompts)
    let config_content = fs::read_to_string(repo.test_config_path()).unwrap_or_default();
    assert!(
        !config_content.contains("skip-shell-integration-prompt"),
        "Should not mark as prompted for non-TTY (hints are not prompts): {config_content}"
    );

    // Second non-TTY run should also show hint
    let output2 = repo
        .wt_command()
        .args(["switch", "--create", "feature2"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();

    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr2.contains("wt config shell install"),
        "Should show hint on every non-TTY run: {stderr2}"
    );
}

/// Test that unsupported shells show appropriate message
///
/// When SHELL is set to an unsupported shell (like tcsh), we should:
/// - Show a hint that the shell is not supported
/// - List the supported shells
#[rstest]
fn test_switch_unsupported_shell_shows_hint(repo: TestRepo) {
    use std::process::Stdio;

    // Run with an unsupported shell
    let mut cmd = repo.wt_command();
    cmd.env("SHELL", "/bin/tcsh");

    let output = cmd
        .args(["switch", "--create", "feature"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success(), "Switch should succeed");

    // Should show unsupported shell message
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not yet supported for tcsh"),
        "Should show unsupported shell message: {stderr}"
    );
    assert!(
        stderr.contains("bash, zsh, fish, PowerShell"),
        "Should list supported shells: {stderr}"
    );
}

/// Test that unset SHELL shows install hint
///
/// When SHELL is not set (unusual Unix setup or Windows), we should:
/// - Show the standard install hint
#[rstest]
fn test_switch_no_shell_env_shows_hint(repo: TestRepo) {
    use std::process::Stdio;

    // Run without SHELL set
    let mut cmd = repo.wt_command();
    cmd.env_remove("SHELL");

    let output = cmd
        .args(["switch", "--create", "feature"])
        .stdin(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success(), "Switch should succeed");

    // Should show install hint
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wt config shell install"),
        "Should show install hint when SHELL not set: {stderr}"
    );
}

// PTY-based tests for interactive scenarios
#[cfg(all(unix, feature = "shell-integration-tests"))]
mod pty_tests {
    use super::*;
    use crate::common::{configure_pty_command, open_pty, setup_snapshot_settings};
    use insta::assert_snapshot;
    use insta_cmd::get_cargo_bin;
    use portable_pty::CommandBuilder;
    use std::io::{Read, Write};
    use std::path::Path;
    use tempfile::TempDir;

    /// Execute a command in a PTY with interactive input
    fn exec_in_pty_with_input(
        command: &str,
        args: &[&str],
        working_dir: &Path,
        env_vars: &[(String, String)],
        home_dir: Option<&Path>,
        input: &str,
    ) -> (String, i32) {
        let pair = open_pty();

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(working_dir);

        // Set up isolated environment with coverage passthrough
        configure_pty_command(&mut cmd);

        // Add test-specific environment variables
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        // Set HOME to isolated temp directory if provided
        // (must be after configure_pty_command which sets HOME to real home)
        if let Some(home) = home_dir {
            cmd.env("HOME", home.to_string_lossy().to_string());
            cmd.env(
                "XDG_CONFIG_HOME",
                home.join(".config").to_string_lossy().to_string(),
            );
        }

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();

        // Write input to the PTY (simulating user typing)
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
        drop(writer);

        // Read all output
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let exit_status = child.wait().unwrap();
        let exit_code = exit_status.exit_code() as i32;

        (buf, exit_code)
    }

    /// Normalize output for snapshot testing
    fn normalize_output(output: &str, home_dir: &Path) -> String {
        // Remove platform-specific PTY control sequences
        let output = regex::Regex::new(r"\^D\x08+")
            .unwrap()
            .replace_all(output, "");

        // Remove repository paths
        let output = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]+")
            .unwrap()
            .replace_all(&output, "[REPO]");

        // Remove home directory paths
        let home_str = home_dir.display().to_string();
        let output = output.replace(&home_str, "[HOME]");

        // PTYs use \r\n line endings, normalize to \n
        output.replace("\r\n", "\n")
    }

    /// Test: Already installed (config line exists) → skip prompt, show restart hint
    ///
    /// This covers the "installed but shell not restarted" scenario where:
    /// - Shell integration is not active (no WORKTRUNK_DIRECTIVE_FILE)
    /// - But the config line is already in shell config files
    /// - We should detect this and show a restart hint (not prompt)
    /// - We should NOT mark as prompted (no interactive prompt shown)
    #[rstest]
    fn test_already_installed_skips_prompt(repo: TestRepo) {
        // Create isolated HOME with shell config that already has integration
        let temp_home = TempDir::new().unwrap();
        let bashrc = temp_home.path().join(".bashrc");
        let config_line = "if command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init bash)\"; fi";
        fs::write(&bashrc, format!("{config_line}\n")).unwrap();

        let mut env_vars = repo.test_env_vars();
        // Remove WORKTRUNK_DIRECTIVE_FILE if present (ensure shell integration not active)
        env_vars.retain(|(k, _)| k != "WORKTRUNK_DIRECTIVE_FILE");
        // Set SHELL to bash since we're testing with .bashrc
        env_vars.push(("SHELL".to_string(), "/bin/bash".to_string()));

        let (output, exit_code) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "", // No input needed - should not prompt
        );

        assert_eq!(exit_code, 0, "Switch should succeed");

        // Should NOT contain prompt (detected already installed)
        assert!(
            !output.contains("Install shell integration"),
            "Should not prompt when already installed: {output}"
        );

        // Should show restart hint
        assert!(
            output.contains("Restart") && output.contains("shell"),
            "Should show restart hint: {output}"
        );

        // Should have created the worktree
        assert!(
            output.contains("Created new worktree for"),
            "Should create worktree: {output}"
        );

        // Config should NOT have skip-shell-integration-prompt = true
        // (no interactive prompt shown, just a hint)
        let config_content = fs::read_to_string(repo.test_config_path()).unwrap_or_default();
        assert!(
            !config_content.contains("skip-shell-integration-prompt"),
            "Should NOT mark as prompted when just showing hint: {config_content}"
        );
    }

    /// Test: Not installed, user declines → mark prompted, no install
    #[rstest]
    fn test_user_declines_prompt(repo: TestRepo) {
        // Create isolated HOME with empty shell config
        let temp_home = TempDir::new().unwrap();
        let bashrc = temp_home.path().join(".bashrc");
        fs::write(&bashrc, "# empty bashrc\n").unwrap();

        let mut env_vars = repo.test_env_vars();
        env_vars.retain(|(k, _)| k != "WORKTRUNK_DIRECTIVE_FILE");
        // Set SHELL to bash since we're testing with .bashrc
        env_vars.push(("SHELL".to_string(), "/bin/bash".to_string()));

        let (output, exit_code) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "n\n", // User declines
        );

        assert_eq!(exit_code, 0, "Switch should succeed even when declining");

        let normalized = normalize_output(&output, temp_home.path());

        // Should contain the prompt
        assert!(
            normalized.contains("Install shell integration"),
            "Should show prompt: {normalized}"
        );

        // Should have created the worktree
        assert!(
            normalized.contains("Created new worktree for"),
            "Should create worktree: {normalized}"
        );

        // Config should have skip-shell-integration-prompt = true
        let config_content = fs::read_to_string(repo.test_config_path()).unwrap_or_default();
        assert!(
            config_content.contains("skip-shell-integration-prompt = true"),
            "Should mark as prompted after decline: {config_content}"
        );

        // Shell config should NOT have the integration line
        let bashrc_content = fs::read_to_string(&bashrc).unwrap();
        assert!(
            !bashrc_content.contains("eval \"$(command wt"),
            "Should not install when declined: {bashrc_content}"
        );

        // Snapshot the output
        let settings = setup_snapshot_settings(&repo);
        settings.bind(|| {
            assert_snapshot!("prompt_decline", normalized);
        });
    }

    /// Test: Not installed, user accepts → install and show success
    #[rstest]
    fn test_user_accepts_prompt(repo: TestRepo) {
        // Create isolated HOME with empty shell config
        let temp_home = TempDir::new().unwrap();
        let bashrc = temp_home.path().join(".bashrc");
        fs::write(&bashrc, "# empty bashrc\n").unwrap();

        let mut env_vars = repo.test_env_vars();
        env_vars.retain(|(k, _)| k != "WORKTRUNK_DIRECTIVE_FILE");
        // Set SHELL to bash since we're testing with .bashrc
        env_vars.push(("SHELL".to_string(), "/bin/bash".to_string()));

        let (output, exit_code) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "y\n", // User accepts
        );

        assert_eq!(exit_code, 0, "Switch should succeed");

        let normalized = normalize_output(&output, temp_home.path());

        // Should contain the prompt
        assert!(
            normalized.contains("Install shell integration"),
            "Should show prompt: {normalized}"
        );

        // Should show success message for configuration
        assert!(
            normalized.contains("Configured") && normalized.contains("bash"),
            "Should show configured message: {normalized}"
        );

        // Config should NOT have skip-shell-integration-prompt after accept
        // (only set after explicit decline - if they uninstall, they can be prompted again)
        let config_content = fs::read_to_string(repo.test_config_path()).unwrap_or_default();
        assert!(
            !config_content.contains("skip-shell-integration-prompt = true"),
            "Should not set skip flag after accept (installation itself prevents future prompts): {config_content}"
        );

        // Shell config SHOULD have the integration line
        let bashrc_content = fs::read_to_string(&bashrc).unwrap();
        assert!(
            bashrc_content.contains("eval \"$(command wt"),
            "Should install when accepted: {bashrc_content}"
        );

        // Snapshot the output
        let settings = setup_snapshot_settings(&repo);
        settings.bind(|| {
            assert_snapshot!("prompt_accept", normalized);
        });
    }

    /// Test: User requests preview with ? then declines
    #[rstest]
    fn test_user_requests_preview_then_declines(repo: TestRepo) {
        // Create isolated HOME with empty shell config
        let temp_home = TempDir::new().unwrap();
        let bashrc = temp_home.path().join(".bashrc");
        fs::write(&bashrc, "# empty bashrc\n").unwrap();

        let mut env_vars = repo.test_env_vars();
        env_vars.retain(|(k, _)| k != "WORKTRUNK_DIRECTIVE_FILE");
        // Set SHELL to bash since we're testing with .bashrc
        env_vars.push(("SHELL".to_string(), "/bin/bash".to_string()));

        let (output, exit_code) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "?\nn\n", // User requests preview, then declines
        );

        assert_eq!(exit_code, 0, "Switch should succeed");

        let normalized = normalize_output(&output, temp_home.path());

        // Should contain the prompt (shown twice - before and after preview)
        assert!(
            normalized.contains("Install shell integration"),
            "Should show prompt: {normalized}"
        );

        // Should show preview content (gutter with config line)
        assert!(
            normalized.contains("Will add") && normalized.contains("bash"),
            "Should show preview: {normalized}"
        );

        // Should show the config line in preview
        assert!(
            normalized.contains("eval") && normalized.contains("wt config shell init"),
            "Should show config line in preview: {normalized}"
        );

        // Shell config should NOT have the integration line (user declined)
        let bashrc_content = fs::read_to_string(&bashrc).unwrap();
        assert!(
            !bashrc_content.contains("eval \"$(command wt"),
            "Should not install when declined after preview: {bashrc_content}"
        );

        // Snapshot the output
        let settings = setup_snapshot_settings(&repo);
        settings.bind(|| {
            assert_snapshot!("prompt_preview_decline", normalized);
        });
    }

    /// Test: Second switch after first prompt → no prompt
    #[rstest]
    fn test_no_prompt_after_first_prompt(repo: TestRepo) {
        // Create isolated HOME with empty shell config
        let temp_home = TempDir::new().unwrap();
        let bashrc = temp_home.path().join(".bashrc");
        fs::write(&bashrc, "# empty bashrc\n").unwrap();

        let mut env_vars = repo.test_env_vars();
        env_vars.retain(|(k, _)| k != "WORKTRUNK_DIRECTIVE_FILE");
        // Set SHELL to bash since we're testing with .bashrc
        env_vars.push(("SHELL".to_string(), "/bin/bash".to_string()));

        // First switch - decline the prompt
        let (_, _) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature1"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "n\n",
        );

        // Second switch - should NOT prompt again
        let (output, exit_code) = exec_in_pty_with_input(
            get_cargo_bin("wt").to_str().unwrap(),
            &["switch", "--create", "feature2"],
            repo.root_path(),
            &env_vars,
            Some(temp_home.path()),
            "", // No input needed
        );

        assert_eq!(exit_code, 0, "Second switch should succeed");

        assert!(
            !output.contains("Install shell integration"),
            "Should not prompt on second switch: {output}"
        );
    }
}
