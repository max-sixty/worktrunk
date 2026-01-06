//! Shell probe integration tests.
//!
//! Tests the `probe_shell_integration` function which spawns interactive shells
//! to determine how a command resolves (function, alias, or binary).
#![cfg(all(unix, feature = "shell-integration-tests"))]

use crate::common::{
    TestRepo, repo,
    shell::{execute_shell_script, generate_init_code, path_export_syntax, wt_bin_dir},
};
use rstest::rstest;
use worktrunk::shell::{Shell, ShellProbeResult, probe_shell_integration};

// =============================================================================
// UNIT TESTS - Parsing logic (always run, no shell required)
// =============================================================================

#[test]
fn test_probe_result_is_function() {
    assert!(ShellProbeResult::Function.is_function());
    assert!(
        !ShellProbeResult::Alias {
            target: "wt.exe".to_string()
        }
        .is_function()
    );
    assert!(
        !ShellProbeResult::Binary {
            path: "/usr/bin/wt".to_string()
        }
        .is_function()
    );
    assert!(!ShellProbeResult::NotFound.is_function());
}

// =============================================================================
// INTEGRATION TESTS - Shell probing (require shells)
// =============================================================================

/// Test that probing a shell with shell integration configured returns Function.
///
/// This test sources the wt shell init code and then uses `type` to verify
/// that `wt` is now a function.
#[rstest]
#[case::bash("bash", Shell::Bash)]
#[case::zsh("zsh", Shell::Zsh)]
fn test_probe_detects_function_after_init(
    #[case] shell_name: &str,
    #[case] _shell: Shell,
    repo: TestRepo,
) {
    let init_code = generate_init_code(&repo, shell_name);
    let bin_path = wt_bin_dir();

    // Include init code directly in the script (execute_shell_script uses --norc)
    let script = format!(
        r#"
        {}
        {}
        type wt 2>&1
        "#,
        path_export_syntax(shell_name, &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, shell_name, &script);

    // The output should indicate wt is a function
    assert!(
        output.contains("function"),
        "Expected 'function' in type output for {} after init, got:\n{}",
        shell_name,
        output
    );
}

/// Test that probing a shell without shell integration returns Binary or NotFound.
#[rstest]
#[case::bash("bash", Shell::Bash)]
#[case::zsh("zsh", Shell::Zsh)]
fn test_probe_detects_binary_without_init(#[case] shell_name: &str, #[case] shell: Shell) {
    // Probe the shell for a command that definitely exists as a binary (ls)
    // This tests the probe mechanism without needing wt to be installed
    let result = probe_shell_integration(shell, "ls");

    // ls should be a binary
    assert!(
        matches!(result, ShellProbeResult::Binary { .. }),
        "Expected ls to be detected as Binary in {}, got: {:?}",
        shell_name,
        result
    );
}

/// Test that probing for a non-existent command returns NotFound.
#[rstest]
#[case::bash("bash", Shell::Bash)]
#[case::zsh("zsh", Shell::Zsh)]
fn test_probe_detects_not_found(#[case] shell_name: &str, #[case] shell: Shell) {
    // Probe for a command that definitely doesn't exist
    let result = probe_shell_integration(shell, "__wt_nonexistent_command_12345__");

    assert!(
        matches!(result, ShellProbeResult::NotFound),
        "Expected NotFound for nonexistent command in {}, got: {:?}",
        shell_name,
        result
    );
}

/// Test that aliases are detected correctly.
///
/// This creates an alias inline and verifies the type command shows it.
#[rstest]
#[case::bash("bash", Shell::Bash)]
fn test_probe_detects_alias(#[case] shell_name: &str, #[case] _shell: Shell, repo: TestRepo) {
    // Define alias inline in the script (execute_shell_script uses --norc)
    // Need to enable expand_aliases since bash doesn't expand aliases in non-interactive mode
    let script = r#"
        shopt -s expand_aliases
        alias my_test_alias='/usr/bin/ls'
        type my_test_alias 2>&1
    "#;

    let output = execute_shell_script(&repo, shell_name, script);

    // Verify the output shows alias
    assert!(
        output.contains("alias") || output.contains("aliased"),
        "Expected 'alias' in type output for {} with alias, got:\n{}",
        shell_name,
        output
    );
}

/// Test the Issue #348 scenario: alias to binary bypasses function.
///
/// This simulates the exact problem from the GitHub issue:
/// - User has `eval "$(git-wt.exe config shell init bash)"`
/// - User has `alias gwt="git-wt.exe"`
/// - The function is named `git-wt`, but the alias points to the binary
#[rstest]
fn test_issue_348_alias_bypasses_function(repo: TestRepo) {
    let bin_path = wt_bin_dir();
    let init_code = generate_init_code(&repo, "bash");

    // Create a script that mimics Issue #348:
    // - Shell integration for wt (creates function named `wt`)
    // - Alias wt_alias pointing to the binary (not the function)
    //
    // Note: We use wt instead of git-wt for this test since that's what our
    // test infrastructure provides
    //
    // Need to enable expand_aliases since bash doesn't expand aliases in non-interactive mode
    let script = format!(
        r#"
shopt -s expand_aliases
{path_export}
{init_code}
# Issue #348 pattern: alias to binary bypasses the shell function
alias wt_alias="{bin}/wt"

echo "=== Testing wt ==="
type wt 2>&1

echo "=== Testing wt_alias ==="
type wt_alias 2>&1
"#,
        path_export = path_export_syntax("bash", &bin_path),
        init_code = init_code,
        bin = bin_path,
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Verify wt is a function
    assert!(
        output.contains("wt is a function"),
        "wt should be a function, got:\n{}",
        output
    );

    // Verify wt_alias is an alias (to the binary)
    assert!(
        output.contains("wt_alias is aliased") || output.contains("wt_alias is an alias"),
        "wt_alias should be an alias, got:\n{}",
        output
    );

    // The key insight: wt_alias points to the BINARY, not the function.
    // Users experiencing Issue #348 are calling the binary directly via their alias,
    // which means shell integration (WORKTRUNK_DIRECTIVE_FILE) is never set.
    assert!(
        output.contains("/wt"),
        "wt_alias should point to the binary path, got:\n{}",
        output
    );
}

/// Test that fish shell probing works.
#[rstest]
fn test_probe_fish_binary() {
    // Probe for ls in fish
    let result = probe_shell_integration(Shell::Fish, "ls");

    // ls should be a binary (or function in fish since fish has lots of function wrappers)
    assert!(
        matches!(
            result,
            ShellProbeResult::Binary { .. } | ShellProbeResult::Function
        ),
        "Expected ls to be Binary or Function in fish, got: {:?}",
        result
    );
}

/// Test that fish shell probing detects not found.
#[rstest]
fn test_probe_fish_not_found() {
    let result = probe_shell_integration(Shell::Fish, "__wt_nonexistent_12345__");

    assert!(
        matches!(result, ShellProbeResult::NotFound),
        "Expected NotFound in fish, got: {:?}",
        result
    );
}

// =============================================================================
// END-TO-END TESTS - wt config show with shell probe output
// =============================================================================
//
// These tests verify the shell probe behavior in `wt config show`.
// They use the standard command-based testing approach (not PTY shell scripts)
// to ensure deterministic behavior.

use crate::common::{set_temp_home_env, wt_command};
use std::fs;
use std::process::Stdio;
use tempfile::TempDir;

/// Test that `wt config show` shows "Restart shell" hint when:
/// - Shell config EXISTS (has eval line)
/// - Shell probe shows BINARY (current shell hasn't loaded it yet)
///
/// This is the main use case: user configured shell but hasn't restarted.
#[rstest]
fn test_config_show_restart_hint_when_config_exists(repo: TestRepo) {
    let temp_home = TempDir::new().unwrap();

    // Create global config (required)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .bashrc WITH eval line (shell integration configured)
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Shell integration configured
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init bash)"; fi
"#,
    )
    .unwrap();

    // Run wt config show (NOT through shell function, so probe will show binary)
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config").arg("show").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    // Set SHELL to bash so current_shell() detects bash, matching our .bashrc config
    cmd.env("SHELL", "/bin/bash");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = cmd.output().expect("Failed to execute command");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should show binary warning (probe runs because not active)
    assert!(
        stderr.contains("is binary at") && stderr.contains("not function"),
        "Expected binary warning in output:\n{}",
        stderr
    );

    // Should show restart hint because config EXISTS
    assert!(
        stderr.contains("Restart shell to activate"),
        "Expected 'Restart shell to activate' hint when config exists:\n{}",
        stderr
    );
}

/// Test that `wt config show` does NOT show "Restart shell" hint when:
/// - Shell config does NOT exist
/// - Shell probe shows BINARY
///
/// "Restart shell" is misleading when there's nothing to load.
#[rstest]
fn test_config_show_no_restart_hint_when_no_config(repo: TestRepo) {
    let temp_home = TempDir::new().unwrap();

    // Create global config (required)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // No .bashrc created - shell integration not configured

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config").arg("show").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = cmd.output().expect("Failed to execute command");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should show binary warning
    assert!(
        stderr.contains("is binary at") && stderr.contains("not function"),
        "Expected binary warning in output:\n{}",
        stderr
    );

    // Should NOT show restart hint because config doesn't exist
    assert!(
        !stderr.contains("Restart shell to activate"),
        "Should NOT show 'Restart shell to activate' when config doesn't exist:\n{}",
        stderr
    );
}

/// Test that `wt config show` skips probe when shell integration is active.
///
/// When WORKTRUNK_DIRECTIVE_FILE is set, shell integration is already working.
#[rstest]
fn test_config_show_skips_probe_when_active(repo: TestRepo) {
    let temp_home = TempDir::new().unwrap();
    let directive_file = temp_home.path().join("wt-directive");

    // Create global config (required)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config").arg("show").current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    // Simulate shell integration being active
    cmd.env("WORKTRUNK_DIRECTIVE_FILE", &directive_file);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = cmd.output().expect("Failed to execute command");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should show "Shell integration active"
    assert!(
        stderr.contains("Shell integration active"),
        "Expected 'Shell integration active' in output:\n{}",
        stderr
    );

    // Should NOT show any shell probe output (skipped because active)
    assert!(
        !stderr.contains("Shell probe:"),
        "Should NOT show shell probe when integration is active:\n{}",
        stderr
    );
}
