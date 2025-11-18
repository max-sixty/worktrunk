//! PTY-based tests for interactive approval prompts
//!
//! These tests verify the approval workflow in a real PTY environment where stdin is a TTY.
//! This allows testing the actual interactive prompt behavior that users experience.
//!
//! Note: These tests are separate from `approval_ui.rs` because they require PTY setup
//! to simulate interactive terminals. The non-PTY tests in `approval_ui.rs` verify the
//! error case (non-TTY environments).

use crate::common::TestRepo;
use insta::assert_snapshot;
use insta_cmd::get_cargo_bin;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;

/// Execute a command in a PTY with interactive input
///
/// Returns (combined_output, exit_code)
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 48,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    // Spawn the command inside the PTY
    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Set minimal environment
    cmd.env_clear();
    cmd.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn command in PTY");
    drop(pair.slave); // Close slave in parent

    // Get reader and writer for the PTY master
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("Failed to clone PTY reader");
    let mut writer = pair.master.take_writer().expect("Failed to get PTY writer");

    // Write input to the PTY (simulating user typing)
    writer
        .write_all(input.as_bytes())
        .expect("Failed to write input to PTY");
    writer.flush().expect("Failed to flush PTY writer");
    drop(writer); // Close writer so command sees EOF

    // Read all output
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .expect("Failed to read PTY output");

    // Wait for child to exit
    let exit_status = child.wait().expect("Failed to wait for child");
    let exit_code = exit_status.exit_code() as i32;

    (buf, exit_code)
}

/// Normalize output for snapshot testing
fn normalize_output(output: &str) -> String {
    // Remove repository paths
    let output = regex::Regex::new(r"/[^\s]+\.tmp[^\s/]+")
        .unwrap()
        .replace_all(output, "[REPO]");

    // Remove config paths
    let output = regex::Regex::new(r"/var/folders/[^\s]+/test-config\.toml")
        .unwrap()
        .replace_all(&output, "[CONFIG]");

    output.to_string()
}

#[test]
fn test_approval_prompt_accept() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test command'""#);
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-approve"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0, "Command should succeed when approved");
    assert_snapshot!("approval_prompt_accept", normalized);
}

#[test]
fn test_approval_prompt_decline() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test command'""#);
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-decline"],
        repo.root_path(),
        &env_vars,
        "n\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0, "Command should succeed even when declined");
    assert_snapshot!("approval_prompt_decline", normalized);
}

#[test]
fn test_approval_prompt_multiple_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    );
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-multi"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0);
    assert_snapshot!("approval_prompt_multiple_commands", normalized);
}

#[test]
fn test_approval_prompt_permission_error() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(r#"post-create-command = "echo 'test command'""#);
    repo.commit("Add config");

    // Create config file and make it read-only to trigger permission error when saving approval
    let config_path = repo.test_config_path();
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        // Create the config file first
        fs::write(config_path, "# read-only config\n").unwrap();

        // Make it read-only
        let mut perms = fs::metadata(config_path).unwrap().permissions();
        perms.set_mode(0o444); // Read-only
        fs::set_permissions(config_path, perms).unwrap();
    }

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-permission"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(
        exit_code, 0,
        "Command should succeed even when saving approval fails"
    );
    assert!(
        normalized.contains("Failed to save command approval"),
        "Should show permission error warning"
    );
    assert!(
        normalized.contains("You will be prompted again next time"),
        "Should show hint about being prompted again"
    );
    assert!(
        normalized.contains("test command"),
        "Command should still execute despite save failure"
    );
    assert_snapshot!("approval_prompt_permission_error", normalized);
}

#[test]
fn test_approval_prompt_named_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(
        r#"[post-create-command]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"
test = "echo 'Running tests...'"
"#,
    );
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-named"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0, "Command should succeed when approved");
    assert!(
        normalized.contains("install") && normalized.contains("Installing dependencies"),
        "Should show command name 'install' and execute it"
    );
    assert!(
        normalized.contains("build") && normalized.contains("Building project"),
        "Should show command name 'build' and execute it"
    );
    assert!(
        normalized.contains("test") && normalized.contains("Running tests"),
        "Should show command name 'test' and execute it"
    );
    assert_snapshot!("approval_prompt_named_commands", normalized);
}

#[test]
fn test_approval_prompt_mixed_approved_unapproved_accept() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
        project_id
    ));

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-mixed-accept"],
        repo.root_path(),
        &env_vars,
        "y\n",
    );

    let normalized = normalize_output(&output);
    assert_eq!(exit_code, 0, "Command should succeed when approved");

    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        normalized.contains("execute")
            && normalized.contains("2")
            && normalized.contains("command"),
        "Should show 2 unapproved commands in prompt"
    );
    assert!(
        normalized.contains("First command"),
        "Should execute first command"
    );
    assert!(
        normalized.contains("Second command"),
        "Should execute pre-approved second command"
    );
    assert!(
        normalized.contains("Third command"),
        "Should execute third command"
    );
    assert_snapshot!(
        "approval_prompt_mixed_approved_unapproved_accept",
        normalized
    );
}

#[test]
fn test_approval_prompt_mixed_approved_unapproved_decline() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.write_project_config(
        r#"post-create-command = [
    "echo 'First command'",
    "echo 'Second command'",
    "echo 'Third command'"
]"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    let project_id = repo.root_path().file_name().unwrap().to_str().unwrap();
    repo.write_test_config(&format!(
        r#"[projects."{}"]
approved-commands = ["echo 'Second command'"]
"#,
        project_id
    ));

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_in_pty_with_input(
        get_cargo_bin("wt").to_str().unwrap(),
        &["switch", "--create", "test-mixed-decline"],
        repo.root_path(),
        &env_vars,
        "n\n",
    );

    let normalized = normalize_output(&output);

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when declined (worktree still created)"
    );
    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        normalized.contains("execute")
            && normalized.contains("2")
            && normalized.contains("command"),
        "Should show only 2 unapproved commands in prompt (not 3)"
    );
    // When declined, ALL commands are skipped (including pre-approved ones)
    assert!(
        normalized.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    // Commands appear in the prompt, but should not be executed
    // Check for "Running post-create" which indicates execution
    assert!(
        !normalized.contains("Running post-create"),
        "Should NOT execute any commands when declined"
    );
    assert!(
        normalized.contains("Created new worktree"),
        "Should still create worktree even when commands declined"
    );
    assert_snapshot!(
        "approval_prompt_mixed_approved_unapproved_decline",
        normalized
    );
}
