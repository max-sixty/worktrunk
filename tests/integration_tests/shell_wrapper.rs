//! Shell wrapper integration tests
//!
//! Tests that verify the complete shell integration path - commands executed through
//! the actual shell wrapper (wt_exec in bash/zsh/fish).
//!
//! These tests ensure that:
//! - Directives are never leaked to users
//! - Output is properly formatted for humans
//! - Shell integration works end-to-end as users experience it
//!
//! ## Why Manual PTY Execution + File Snapshots (Not insta_cmd)?
//!
//! These tests use PTY execution because testing shell wrappers requires real TTY behavior
//! (streaming output, ANSI codes, signal handling). `insta_cmd` uses `std::process::Command`
//! which doesn't provide a TTY to child processes.
//!
//! Output normalization uses insta's `add_filter()` API via `shell_wrapper_settings()`,
//! which is consistent with how other tests in the codebase handle path and hash
//! normalization. The filters handle:
//! - PTY-specific artifacts (CRLF, ^D control sequences, ANSI resets)
//! - Temporary directory paths
//! - Commit hashes (non-deterministic in PTY tests due to timing/environment)
//! - Project root paths

// All shell integration tests and infrastructure gated by feature flag
// Supports both Unix (bash/zsh/fish) and Windows (PowerShell)
#![cfg(feature = "shell-integration-tests")]

// =============================================================================
// Imports
// =============================================================================

// Shared imports (both platforms)
use crate::common::TestRepo;
use insta_cmd::get_cargo_bin;
use std::process::Command;

// Unix-only imports
#[cfg(unix)]
use {
    crate::common::{add_pty_filters, add_pty_tmpdir_filters, canonicalize, wait_for_file_content},
    insta::assert_snapshot,
    std::{fs, path::PathBuf, sync::LazyLock},
    worktrunk::shell,
};

/// Output from executing a command through a shell wrapper
#[derive(Debug)]
struct ShellOutput {
    /// Combined stdout and stderr as user would see
    combined: String,
    /// Exit code from the command
    exit_code: i32,
}

/// Regex for detecting bash job control messages
/// Matches patterns like "[1] 12345" (job start) and "[1]+ Done" (job completion)
#[cfg(unix)]
static JOB_CONTROL_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[\d+\][+-]?\s+(Done|\d+)").unwrap());

impl ShellOutput {
    /// Check if output contains no directive leaks
    fn assert_no_directive_leaks(&self) {
        assert!(
            !self.combined.contains("__WORKTRUNK_CD__"),
            "Output contains leaked __WORKTRUNK_CD__ directive:\n{}",
            self.combined
        );
        assert!(
            !self.combined.contains("__WORKTRUNK_EXEC__"),
            "Output contains leaked __WORKTRUNK_EXEC__ directive:\n{}",
            self.combined
        );
    }

    /// Check if output contains no bash job control messages
    ///
    /// Job control messages like "[1] 12345" (job start) and "[1]+ Done ..." (job completion)
    /// should not appear in user-facing output. These are internal shell artifacts from
    /// background process management that leak implementation details.
    #[cfg(unix)]
    fn assert_no_job_control_messages(&self) {
        assert!(
            !JOB_CONTROL_REGEX.is_match(&self.combined),
            "Output contains job control messages (e.g., '[1] 12345' or '[1]+ Done'):\n{}",
            self.combined
        );
    }

    /// Assert command exited successfully (exit code 0)
    #[cfg(unix)]
    fn assert_success(&self) {
        assert_eq!(
            self.exit_code, 0,
            "Expected exit code 0, got {}.\nOutput:\n{}",
            self.exit_code, self.combined
        );
    }
}

/// Create insta settings configured for shell wrapper snapshot tests.
///
/// This sets up filters for:
/// - PTY-specific artifacts (CRLF, ^D control sequences, ANSI resets)
/// - Temporary directory paths
/// - Project root paths (for fixture references)
#[cfg(unix)]
fn shell_wrapper_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Add common PTY filters (CRLF, ^D, leading ANSI resets)
    add_pty_filters(&mut settings);

    // Add temp directory path filters
    add_pty_tmpdir_filters(&mut settings, "[TMPDIR]");

    // Normalize project root paths (for fixture references like tests/fixtures/)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        settings.add_filter(&regex::escape(&manifest_dir), "[PROJECT_ROOT]");
    }

    settings
}

/// Generate a shell wrapper script using the actual `wt config shell init` command
fn generate_wrapper(repo: &TestRepo, shell: &str) -> String {
    let wt_bin = get_cargo_bin("wt");

    let mut cmd = Command::new(&wt_bin);
    cmd.arg("config").arg("shell").arg("init").arg(shell);

    // Configure environment
    repo.configure_wt_cmd(&mut cmd);

    let output = cmd.output().unwrap_or_else(|e| {
        panic!(
            "Failed to run wt config shell init {}: {} (binary: {})",
            shell,
            e,
            wt_bin.display()
        )
    });

    if !output.status.success() {
        panic!(
            "wt config shell init {} failed with exit code: {:?}\nOutput:\n{}",
            shell,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .unwrap_or_else(|_| panic!("wt config shell init {} produced invalid UTF-8", shell))
}

/// Generate shell completions script for the given shell
///
/// Note: Fish completions are custom (use $WORKTRUNK_BIN to bypass shell wrapper).
/// Bash and Zsh use inline lazy loading in the init script.
#[cfg(unix)]
fn generate_completions(_repo: &TestRepo, shell: &str) -> String {
    match shell {
        "fish" => {
            // Fish uses a custom completion that bypasses the shell wrapper
            r#"# worktrunk completions for fish - uses $WORKTRUNK_BIN to bypass shell wrapper
complete --keep-order --exclusive --command wt --arguments "(COMPLETE=fish \$WORKTRUNK_BIN -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"
"#.to_string()
        }
        _ => {
            // Bash and Zsh use inline lazy loading in the init script
            String::new()
        }
    }
}

/// Quote a shell argument if it contains special characters
fn quote_arg(arg: &str) -> String {
    if arg.contains(' ') || arg.contains(';') || arg.contains('\'') {
        shell_quote(arg)
    } else {
        arg.to_string()
    }
}

/// Always quote a string for shell use, properly escaping single quotes.
/// Handles paths like `/path/to/worktrunk.'∅'/target/debug/wt`
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote a path for PowerShell (escape backticks and single quotes)
fn powershell_quote(s: &str) -> String {
    // PowerShell string escaping: use single quotes and escape embedded single quotes by doubling
    format!("'{}'", s.replace('\'', "''"))
}

/// Build a shell script that sources the wrapper and runs a command
fn build_shell_script(shell: &str, repo: &TestRepo, subcommand: &str, args: &[&str]) -> String {
    let wt_bin = get_cargo_bin("wt");
    let wrapper_script = generate_wrapper(repo, shell);
    let mut script = String::new();

    // Set environment variables - syntax varies by shell
    // Don't use 'set -e' in bash/zsh - we want to capture failures and their exit codes.
    // This is tested by test_wrapper_handles_command_failure which verifies
    // that command failures return proper exit codes rather than aborting the script.
    // Properly quote paths to handle special characters like single quotes
    let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
    let config_path_quoted = shell_quote(&repo.test_config_path().display().to_string());

    match shell {
        "fish" => {
            script.push_str(&format!("set -x WORKTRUNK_BIN {}\n", wt_bin_quoted));
            script.push_str(&format!(
                "set -x WORKTRUNK_CONFIG_PATH {}\n",
                config_path_quoted
            ));
            script.push_str("set -x CLICOLOR_FORCE 1\n");
        }
        "zsh" => {
            // For zsh, initialize the completion system first
            // This allows static completions (which call compdef) to work in isolated mode
            // We run with --no-rcs to prevent user rc files from touching /dev/tty,
            // but compinit is safe since it only sets up completion functions
            script.push_str("autoload -Uz compinit && compinit -i 2>/dev/null\n");

            script.push_str(&format!("export WORKTRUNK_BIN={}\n", wt_bin_quoted));
            script.push_str(&format!(
                "export WORKTRUNK_CONFIG_PATH={}\n",
                config_path_quoted
            ));
            script.push_str("export CLICOLOR_FORCE=1\n");
        }
        "powershell" | "pwsh" => {
            // PowerShell uses $env: for environment variables
            let wt_bin_ps = powershell_quote(&wt_bin.display().to_string());
            let config_path_ps = powershell_quote(&repo.test_config_path().display().to_string());
            script.push_str(&format!("$env:WORKTRUNK_BIN = {}\n", wt_bin_ps));
            script.push_str(&format!(
                "$env:WORKTRUNK_CONFIG_PATH = {}\n",
                config_path_ps
            ));
            script.push_str("$env:CLICOLOR_FORCE = '1'\n");
        }
        _ => {
            // bash
            script.push_str(&format!("export WORKTRUNK_BIN={}\n", wt_bin_quoted));
            script.push_str(&format!(
                "export WORKTRUNK_CONFIG_PATH={}\n",
                config_path_quoted
            ));
            script.push_str("export CLICOLOR_FORCE=1\n");
        }
    }

    // Source the wrapper (PowerShell uses . for sourcing inline code via Invoke-Expression)
    match shell {
        "powershell" | "pwsh" => {
            // The wrapper_script is PowerShell code; we include it directly
            script.push_str(&wrapper_script);
            script.push('\n');
        }
        _ => {
            script.push_str(&wrapper_script);
            script.push('\n');
        }
    }

    // Build the command
    script.push_str("wt ");
    script.push_str(subcommand);
    for arg in args {
        script.push(' ');
        match shell {
            "powershell" | "pwsh" => {
                // PowerShell argument quoting
                if arg.contains(' ') || arg.contains(';') || arg.contains('\'') {
                    script.push_str(&powershell_quote(arg));
                } else {
                    script.push_str(arg);
                }
            }
            _ => {
                script.push_str(&quote_arg(arg));
            }
        }
    }
    script.push('\n');

    // Merge stderr to stdout to simulate real terminal behavior
    // In a real terminal, both streams interleave naturally by the OS.
    // The .output() method captures them separately, so we merge them here
    // to preserve temporal locality (output appears when operations complete, not batched at the end)
    match shell {
        "fish" => {
            // Fish uses begin...end for grouping
            // Note: This exposes a Fish wrapper buffering bug where child output appears out of order
            // (see templates/fish.fish - psub causes buffering). Tests document current behavior.
            format!("begin\n{}\nend 2>&1", script)
        }
        "bash" => {
            // For bash, we don't use a subshell wrapper because it would isolate job control messages.
            // Instead, we use exec to redirect stderr to stdout, then run the script.
            // This ensures job control messages (like "[1] 12345" and "[1]+ Done") are captured,
            // allowing tests to catch these leaks.
            format!("exec 2>&1\n{}", script)
        }
        "powershell" | "pwsh" => {
            // PowerShell: use & to run script block with merged output
            // Wrap the script in a script block and redirect stderr (2) to stdout (1)
            format!("& {{\n{}\n}} 2>&1", script)
        }
        _ => {
            // zsh uses parentheses for subshell grouping
            format!("( {} ) 2>&1", script)
        }
    }
}

/// Normalize line endings (CRLF -> LF)
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Execute a command in a PTY with interactive input support
///
/// This is similar to `exec_in_pty` but allows sending input during execution.
/// The PTY will automatically echo the input (like a real terminal), so you'll
/// see both the prompts and the input in the captured output.
///
/// # Arguments
/// * `shell` - The shell to use (e.g., "bash", "zsh")
/// * `script` - The script to execute
/// * `working_dir` - Working directory for the command
/// * `env_vars` - Environment variables to set
/// * `inputs` - A slice of strings to send as input (e.g., `&["y\n", "feature\n"]`)
///
/// # Example
/// ```no_run
/// let (output, exit_code) = exec_in_pty_interactive(
///     "bash",
///     "wt switch --create",
///     repo.root_path(),
///     &[("CLICOLOR_FORCE", "1")],
///     &["y\n"],  // Send 'y' and newline when prompted
/// );
/// // The output will show: "Allow? [y/N] y"
/// ```
/// Get the actual shell binary name (handles powershell -> pwsh mapping)
fn get_shell_binary(shell: &str) -> &str {
    match shell {
        "powershell" => "pwsh",
        _ => shell,
    }
}

#[cfg(test)]
fn exec_in_pty_interactive(
    shell: &str,
    script: &str,
    working_dir: &std::path::Path,
    env_vars: &[(&str, &str)],
    inputs: &[&str],
) -> (String, i32) {
    use portable_pty::CommandBuilder;
    use std::io::{Read, Write};

    let pair = crate::common::open_pty();

    let shell_binary = get_shell_binary(shell);
    let mut cmd = CommandBuilder::new(shell_binary);

    // Clear inherited environment for test isolation
    cmd.env_clear();

    // Set minimal required environment for shells to function
    let home_dir = home::home_dir().unwrap().to_string_lossy().to_string();
    cmd.env("HOME", &home_dir);
    // Windows: Also set USERPROFILE for PowerShell
    #[cfg(windows)]
    cmd.env("USERPROFILE", &home_dir);

    // Use platform-appropriate default PATH
    #[cfg(unix)]
    let default_path = "/usr/bin:/bin";
    #[cfg(windows)]
    let default_path = std::env::var("PATH").unwrap_or_default();

    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| default_path.to_string()),
    );
    cmd.env("USER", "testuser");
    cmd.env("SHELL", shell_binary);

    // Run in interactive mode to simulate real user environment.
    // This ensures tests catch job control message leaks like "[1] 12345" and "[1]+ Done".
    // Interactive shells have job control enabled by default.
    match shell {
        "zsh" => {
            // Isolate from user rc files
            cmd.env("ZDOTDIR", "/dev/null");
            cmd.arg("-i");
            cmd.arg("--no-rcs");
            cmd.arg("-o");
            cmd.arg("NO_GLOBAL_RCS");
            cmd.arg("-o");
            cmd.arg("NO_RCS");
            cmd.arg("-c");
            cmd.arg(script);
        }
        "bash" => {
            cmd.arg("-i");
            cmd.arg("-c");
            cmd.arg(script);
        }
        "powershell" | "pwsh" => {
            // PowerShell uses -NoProfile to skip user profile
            cmd.arg("-NoProfile");
            cmd.arg("-Command");
            cmd.arg(script);
        }
        _ => {
            // fish and other shells
            cmd.arg("-c");
            cmd.arg(script);
        }
    }

    cmd.cwd(working_dir);

    // Add test-specific environment variables (convert &str tuples to String tuples)
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    // Pass through LLVM coverage env vars for subprocess coverage collection
    crate::common::pass_coverage_env_to_pty_cmd(&mut cmd);

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Clone the reader for capturing output
    let mut reader = pair.master.try_clone_reader().unwrap();

    // Write input synchronously if we have any (matches approval_pty.rs approach)
    if !inputs.is_empty() {
        let mut writer = pair.master.take_writer().unwrap();
        for input in inputs {
            writer.write_all(input.as_bytes()).unwrap();
            writer.flush().unwrap();
        }
        drop(writer); // Explicitly drop writer so PTY sees EOF
    }

    // Read everything the "terminal" would display (including echoed input)
    let mut buf = String::new();
    reader.read_to_string(&mut buf).unwrap(); // Blocks until child exits & PTY closes

    let status = child.wait().unwrap();

    (normalize_newlines(&buf), status.exit_code() as i32)
}

/// Execute bash in true interactive mode by writing commands to the PTY
///
/// Unlike `exec_in_pty_interactive` which uses `bash -i -c "script"`, this function
/// starts bash without `-c` and writes commands directly to the PTY. This captures
/// job control notifications (`[1]+ Done`) that only appear at prompt-time in bash.
///
/// The setup_script is written to a temp file and sourced. Then final_cmd is run
/// directly at the prompt (where job notifications appear).
#[cfg(all(test, unix))]
fn exec_bash_truly_interactive(
    setup_script: &str,
    final_cmd: &str,
    working_dir: &std::path::Path,
    env_vars: &[(&str, &str)],
) -> (String, i32) {
    use portable_pty::CommandBuilder;
    use std::io::{Read, Write};
    use std::thread;
    use std::time::Duration;

    // Write setup script to a temp file
    let tmp_dir = tempfile::tempdir().unwrap();
    let script_path = tmp_dir.path().join("setup.sh");
    fs::write(&script_path, setup_script).unwrap();

    let pair = crate::common::open_pty();

    // Spawn bash in true interactive mode using env to pass flags
    // (portable_pty's CommandBuilder can have issues with flag parsing)
    let mut cmd = CommandBuilder::new("env");
    cmd.arg("bash");
    cmd.arg("--norc");
    cmd.arg("--noprofile");
    cmd.arg("-i");

    // Clear inherited environment for test isolation
    cmd.env_clear();

    // Set minimal required environment for shells to function
    cmd.env(
        "HOME",
        home::home_dir().unwrap().to_string_lossy().to_string(),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    cmd.env("USER", "testuser");
    cmd.env("SHELL", "bash");

    // Simple prompt to make output cleaner ($ followed by space)
    cmd.env("PS1", "$ ");
    cmd.cwd(working_dir);

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    // Pass through LLVM coverage env vars for subprocess coverage collection
    crate::common::pass_coverage_env_to_pty_cmd(&mut cmd);

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Get both reader and writer
    let reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Give bash time to start up. Unlike async operations, bash startup is deterministic
    // and fast (<50ms typical), so a fixed sleep is acceptable here. We use 200ms for CI margin.
    thread::sleep(Duration::from_millis(200));

    // Write setup and command (but not exit yet)
    let commands = format!("source '{}'\n{}\n", script_path.display(), final_cmd);
    writer.write_all(commands.as_bytes()).unwrap();
    writer.flush().unwrap();

    // Wait for the command to complete and bash to show job notifications.
    // The `[1]+ Done` message appears when bash prepares to show the next prompt.
    // Without this delay, bash might receive `exit` before it reports job completion.
    thread::sleep(Duration::from_millis(500));

    // Now send exit
    writer.write_all(b"exit\n").unwrap();
    writer.flush().unwrap();
    drop(writer); // Close writer after sending all commands

    // Read output in a thread. This is necessary because bash outputs the `[1]+ Done`
    // notification between command completion and the next prompt, and we need to
    // capture that output while waiting for the child to exit.
    let reader_thread = thread::spawn(move || {
        let mut reader = reader;
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        buf
    });

    // Wait for bash to exit
    let status = child.wait().unwrap();

    // Get the captured output
    let buf = reader_thread.join().unwrap();

    (normalize_newlines(&buf), status.exit_code() as i32)
}

/// Execute a command through a shell wrapper
///
/// This simulates what actually happens when users run `wt switch`, etc. in their shell:
/// 1. The `wt` function is defined (from shell integration)
/// 2. It calls `wt_exec` which sets WORKTRUNK_DIRECTIVE_FILE and runs the binary
/// 3. The wrapper sources the directive file after wt exits (for cd, exec commands)
/// 4. Users see stdout/stderr output in real-time
///
/// Now uses PTY interactive mode for consistent behavior and potential input echoing.
///
/// Returns ShellOutput with combined output and exit code
fn exec_through_wrapper(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
) -> ShellOutput {
    exec_through_wrapper_from(shell, repo, subcommand, args, repo.root_path())
}

fn exec_through_wrapper_from(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    working_dir: &std::path::Path,
) -> ShellOutput {
    // Delegate to interactive version with no input
    // This provides consistent PTY behavior across all tests
    exec_through_wrapper_interactive(shell, repo, subcommand, args, working_dir, &[])
}

/// Execute a command through a shell wrapper with interactive input support
///
/// This is similar to `exec_through_wrapper_from` but allows sending input during execution
/// (e.g., approval responses). The PTY will automatically echo the input, so you'll see
/// both the prompts and the responses in the captured output.
///
/// # Arguments
/// * `shell` - The shell to use (e.g., "bash", "zsh", "fish")
/// * `repo` - The test repository
/// * `subcommand` - The wt subcommand (e.g., "merge", "switch")
/// * `args` - Arguments to the subcommand (without --yes)
/// * `working_dir` - Working directory for the command
/// * `inputs` - Input strings to send (e.g., `&["y\n"]` for approval prompts)
///
/// # Example
/// ```no_run
/// // Test merge with approval prompt visible in output
/// let output = exec_through_wrapper_interactive(
///     "bash",
///     &repo,
///     "merge",
///     &["main"],
///     repo.root_path(),
///     &["y\n"],  // Approve the merge
/// );
/// // Output will show: "❓ Allow and remember? [y/N] y"
/// ```
#[cfg(test)]
fn exec_through_wrapper_interactive(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    inputs: &[&str],
) -> ShellOutput {
    exec_through_wrapper_with_env(shell, repo, subcommand, args, working_dir, inputs, &[])
}

/// Execute a command through a shell wrapper with custom environment variables
///
/// Like `exec_through_wrapper_interactive` but allows additional env vars to be set.
/// Useful for tests that need custom PATH (e.g., for mock binaries).
#[cfg(test)]
fn exec_through_wrapper_with_env(
    shell: &str,
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    inputs: &[&str],
    extra_env: &[(&str, &str)],
) -> ShellOutput {
    let script = build_shell_script(shell, repo, subcommand, args);

    let config_path = repo.test_config_path().to_string_lossy().to_string();

    let mut env_vars = build_test_env_vars(&config_path);
    env_vars.push(("CLICOLOR_FORCE", "1"));
    // Add extra env vars (these can override defaults if needed)
    env_vars.extend(extra_env.iter().copied());

    let (combined, exit_code) =
        exec_in_pty_interactive(shell, &script, working_dir, &env_vars, inputs);

    ShellOutput {
        combined,
        exit_code,
    }
}

/// Standard test environment variables (static parts that don't depend on test state)
///
/// These are used by tests that build custom scripts and call `exec_in_pty_interactive` directly.
/// For tests using `exec_through_wrapper*`, these are already applied.
const STANDARD_TEST_ENV: &[(&str, &str)] = &[
    ("TERM", "xterm"),
    ("GIT_AUTHOR_NAME", "Test User"),
    ("GIT_AUTHOR_EMAIL", "test@example.com"),
    ("GIT_COMMITTER_NAME", "Test User"),
    ("GIT_COMMITTER_EMAIL", "test@example.com"),
    ("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z"),
    ("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z"),
    ("LANG", "C"),
    ("LC_ALL", "C"),
    ("SOURCE_DATE_EPOCH", "1735776000"),
];

/// Build standard test env vars with config path
///
/// Returns a Vec containing STANDARD_TEST_ENV plus WORKTRUNK_CONFIG_PATH.
/// The caller must keep `config_path` alive for the duration of the returned Vec's use.
#[cfg(test)]
fn build_test_env_vars(config_path: &str) -> Vec<(&str, &str)> {
    let mut env_vars: Vec<(&str, &str)> = vec![("WORKTRUNK_CONFIG_PATH", config_path)];
    env_vars.extend_from_slice(STANDARD_TEST_ENV);
    env_vars
}

// =============================================================================
// Unix Shell Tests (bash/zsh/fish)
// =============================================================================
//
// All Unix shell integration tests are in this module, gated by #[cfg(unix)].
// This includes tests for bash, zsh, and fish shells.
//
// Shared infrastructure (exec_through_wrapper, ShellOutput, etc.) is defined
// above and works on both platforms.

#[cfg(unix)]
mod unix_tests {
    use super::*;
    use crate::common::repo;
    use rstest::rstest;

    // ========================================================================
    // Cross-Shell Error Handling Tests
    // ========================================================================
    //
    // These tests use parametrized testing to verify consistent behavior
    // across all supported shells (bash, zsh, fish).
    //
    // Note: Zsh tests run in isolated mode (--no-rcs, ZDOTDIR=/dev/null) to prevent
    // user startup files from touching /dev/tty, which would cause SIGTTIN/TTOU/TSTP
    // signals. This isolation ensures tests are deterministic across all environments.
    //
    // SNAPSHOT CONSOLIDATION:
    // Tests use `insta::allow_duplicates!` to share a single snapshot across all shells
    // when output is deterministic and identical. This reduces snapshot count from 3×N to N.
    //
    // Trade-off: If future changes introduce shell-specific output differences, all three
    // shells will fail with "doesn't match snapshot" rather than showing which specific
    // shell differs. For tests with non-deterministic output (PTY buffering causes varying
    // order), we keep shell-specific snapshots.
    //
    // TODO: Consider adding a test assertion that compares bash/zsh/fish outputs are
    // byte-identical before the snapshot check, so we can identify which shell diverged.

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_handles_command_failure(#[case] shell: &str, mut repo: TestRepo) {
        // Create a worktree that already exists
        repo.add_worktree("existing");

        // Try to create it again - should fail
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "existing"]);

        // Shell-agnostic assertions: these must be true for ALL shells
        assert_eq!(
            output.exit_code, 1,
            "{}: Command should fail with exit code 1",
            shell
        );
        output.assert_no_directive_leaks();
        assert!(
            output.combined.contains("already exists"),
            "{}: Error message should mention 'already exists'.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("command_failure", &output.combined);
            }
        });
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_switch_create(#[case] shell: &str, repo: TestRepo) {
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "feature"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();
        output.assert_no_job_control_messages();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should show success message",
            shell
        );

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("switch_create", &output.combined);
            }
        });
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_remove(#[case] shell: &str, mut repo: TestRepo) {
        // Create a worktree to remove
        repo.add_worktree("to-remove");

        let output = exec_through_wrapper(shell, &repo, "remove", &["to-remove"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("remove", &output.combined);
            }
        });
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_step_for_each(#[case] shell: &str, mut repo: TestRepo) {
        // Remove fixture worktrees so we can create our own feature-a and feature-b
        repo.remove_fixture_worktrees();

        repo.commit("Initial commit");

        // Create additional worktrees
        repo.add_worktree("feature-a");
        repo.add_worktree("feature-b");

        // Run for-each with echo to test stdout handling
        let output = exec_through_wrapper(
            shell,
            &repo,
            "step",
            &["for-each", "--", "echo", "Branch: {{ branch }}"],
        );

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();
        output.assert_no_job_control_messages();

        // Verify output contains branch names (stdout redirected to stderr)
        assert!(
            output.combined.contains("Branch: main"),
            "{}: Should show main branch output.\nOutput:\n{}",
            shell,
            output.combined
        );
        assert!(
            output.combined.contains("Branch: feature-a"),
            "{}: Should show feature-a branch output.\nOutput:\n{}",
            shell,
            output.combined
        );
        assert!(
            output.combined.contains("Branch: feature-b"),
            "{}: Should show feature-b branch output.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Verify summary message
        assert!(
            output.combined.contains("Completed in 3 worktrees"),
            "{}: Should show completion summary.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("step_for_each", &output.combined);
            }
        });
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_merge(#[case] shell: &str, mut repo: TestRepo) {
        // Create a feature branch
        repo.add_worktree("feature");

        let output = exec_through_wrapper(shell, &repo, "merge", &["main"]);

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("merge", &output.combined);
            }
        });
    }

    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_switch_with_execute(#[case] shell: &str, repo: TestRepo) {
        // Use --yes to skip approval prompt in tests
        let output = exec_through_wrapper(
            shell,
            &repo,
            "switch",
            &[
                "--create",
                "test-exec",
                "--execute",
                "echo executed",
                "--yes",
            ],
        );

        // Shell-agnostic assertions
        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("executed"),
            "{}: Execute command output missing",
            shell
        );

        // Consolidated snapshot - output should be identical across all shells
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("switch_with_execute", &output.combined);
            }
        });
    }

    /// Test that --execute command exit codes are propagated
    /// Verifies that when wt succeeds but the --execute command fails,
    /// the wrapper returns the command's exit code, not wt's.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_execute_exit_code_propagation(#[case] shell: &str, repo: TestRepo) {
        // Use --yes to skip approval prompt in tests
        // wt should succeed (creates worktree), but the execute command should fail with exit 42
        let output = exec_through_wrapper(
            shell,
            &repo,
            "switch",
            &[
                "--create",
                "test-exit-code",
                "--execute",
                "exit 42",
                "--yes",
            ],
        );

        // Shell-agnostic assertions
        assert_eq!(
            output.exit_code, 42,
            "{}: Should propagate execute command's exit code (42), not wt's (0)",
            shell
        );
        output.assert_no_directive_leaks();

        // Should still show wt's success message (worktree was created)
        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should show wt's success message even though execute command failed",
            shell
        );
    }

    /// Test switch --create with post-create (blocking) and post-start (background)
    /// Note: bash and fish disabled due to flaky PTY buffering race conditions
    ///
    /// TODO: Fix timing/race condition in bash where "Building project..." output appears
    /// before the command display, causing snapshot mismatch (appears on line 7 instead of line 9).
    /// This is a non-deterministic PTY output ordering issue.
    #[rstest]
    // #[case("bash")] // TODO: Flaky PTY output ordering - command output appears before command display
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_switch_with_hooks(#[case] shell: &str, repo: TestRepo) {
        // Create project config with both post-create and post-start hooks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"# Blocking commands that run before worktree is ready
[post-create]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"

# Background commands that run in parallel
[post-start]
server = "echo 'Starting dev server on port 3000'"
watch = "echo 'Watching for file changes'"
"#,
        )
        .unwrap();

        repo.commit("Add hooks");

        // Pre-approve the commands in user config
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = [
    "echo 'Installing dependencies...'",
    "echo 'Building project...'",
    "echo 'Starting dev server on port 3000'",
    "echo 'Watching for file changes'",
]
"#,
        )
        .unwrap();

        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "feature-hooks"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        shell_wrapper_settings().bind(|| {
            assert_snapshot!(format!("switch_with_hooks_{}", shell), &output.combined);
        });
    }

    /// Test merge with successful pre-merge validation
    /// Note: fish disabled due to flaky PTY buffering race conditions
    /// TODO: bash variant occasionally fails on Ubuntu CI with snapshot mismatches due to PTY timing
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_pre_merge_success(#[case] shell: &str, mut repo: TestRepo) {
        // Create project config with pre-merge validation
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"[pre-merge]
format = "echo '✓ Code formatting check passed'"
lint = "echo '✓ Linting passed - no warnings'"
test = "echo '✓ All 47 tests passed in 2.3s'"
"#,
        )
        .unwrap();

        repo.commit("Add pre-merge validation");
        let feature_wt = repo.add_feature();

        // Pre-approve commands
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = [
    "echo '✓ Code formatting check passed'",
    "echo '✓ Linting passed - no warnings'",
    "echo '✓ All 47 tests passed in 2.3s'",
]
"#,
        )
        .unwrap();

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--yes"], &feature_wt);

        assert_eq!(output.exit_code, 0, "{}: Merge should succeed", shell);
        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        shell_wrapper_settings().bind(|| {
            assert_snapshot!(
                format!("merge_with_pre_merge_success_{}", shell),
                &output.combined
            );
        });
    }

    /// Test merge with failing pre-merge that aborts the merge
    /// Note: fish disabled due to flaky PTY buffering race conditions
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_pre_merge_failure(#[case] shell: &str, mut repo: TestRepo) {
        // Create project config with failing pre-merge validation
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"[pre-merge]
format = "echo '✓ Code formatting check passed'"
test = "echo '✗ Test suite failed: 3 tests failing' && exit 1"
"#,
        )
        .unwrap();

        repo.commit("Add failing pre-merge validation");

        // Create feature worktree with a commit
        let feature_wt = repo.add_worktree_with_commit(
            "feature-fail",
            "feature.txt",
            "feature content",
            "Add feature",
        );

        // Pre-approve the commands
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = [
    "echo '✓ Code formatting check passed'",
    "echo '✗ Test suite failed: 3 tests failing' && exit 1",
]
"#,
        )
        .unwrap();

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--yes"], &feature_wt);

        output.assert_no_directive_leaks();

        // Shell-specific snapshot - output ordering varies due to PTY buffering
        shell_wrapper_settings().bind(|| {
            assert_snapshot!(
                format!("merge_with_pre_merge_failure_{}", shell),
                &output.combined
            );
        });
    }

    /// Test merge with pre-merge commands that output to both stdout and stderr
    /// Verifies that interleaved stdout/stderr appears in correct temporal order
    /// Note: fish disabled due to flaky PTY buffering race conditions
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    // #[case("fish")] // TODO: Fish shell has non-deterministic PTY output ordering
    fn test_wrapper_merge_with_mixed_stdout_stderr(#[case] shell: &str, mut repo: TestRepo) {
        // Copy the fixture script to the test repo to avoid path issues with special characters
        // (CARGO_MANIFEST_DIR may contain single quotes like worktrunk.'∅' which break shell parsing)
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let script_content = fs::read(fixtures_dir.join("mixed-output.sh")).unwrap();
        let script_path = repo.root_path().join("mixed-output.sh");
        fs::write(&script_path, &script_content).unwrap();
        // Make the script executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Create project config with pre-merge commands that output to both stdout and stderr
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            format!(
                r#"[pre-merge]
check1 = "{} check1 3"
check2 = "{} check2 3"
"#,
                script_path.display(),
                script_path.display()
            ),
        )
        .unwrap();

        repo.commit("Add pre-merge validation with mixed output");
        let feature_wt = repo.add_feature();

        // Pre-approve commands
        fs::write(
            repo.test_config_path(),
            format!(
                r#"worktree-path = "../{{{{ repo }}}}.{{{{ branch }}}}"

[projects."../origin"]
approved-commands = [
    "{} check1 3",
    "{} check2 3",
]
"#,
                script_path.display(),
                script_path.display()
            ),
        )
        .unwrap();

        // Run merge from the feature worktree
        let output =
            exec_through_wrapper_from(shell, &repo, "merge", &["main", "--yes"], &feature_wt);

        assert_eq!(output.exit_code, 0, "{}: Merge should succeed", shell);
        output.assert_no_directive_leaks();

        // Verify output shows proper temporal ordering:
        // header1 → all check1 output (interleaved stdout/stderr) → header2 → all check2 output
        // This ensures that stdout/stderr from child processes properly stream through
        // to the terminal in real-time, maintaining correct ordering
        shell_wrapper_settings().bind(|| {
            assert_snapshot!(
                format!("merge_with_mixed_stdout_stderr_{}", shell),
                &output.combined
            );
        });
    }

    // ========================================================================
    // Bash Shell Wrapper Tests
    // ========================================================================

    #[rstest]
    fn test_switch_with_post_start_command_no_directive_leak(repo: TestRepo) {
        // Configure a post-start command in the project config (this is where the bug manifests)
        // The println! in handle_post_start_commands causes directive leaks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'test command executed'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'test command executed'"]
"#,
        )
        .unwrap();

        let output =
            exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-with-hooks"]);

        // The critical assertion: directives must never appear in user-facing output
        // This is where the bug occurs - "🔄 Starting (background):" is printed with println!
        // which causes it to concatenate with the directive
        output.assert_no_directive_leaks();
        output.assert_no_job_control_messages();

        output.assert_success();

        // Normalize paths in output for snapshot testing
        // Snapshot the output
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_switch_with_execute_through_wrapper(repo: TestRepo) {
        // Use --yes to skip approval prompt in tests
        let output = exec_through_wrapper(
            "bash",
            &repo,
            "switch",
            &[
                "--create",
                "test-exec",
                "--execute",
                "echo executed",
                "--yes",
            ],
        );

        // No directives should leak
        output.assert_no_directive_leaks();
        output.assert_success();

        // The executed command output should appear
        assert!(
            output.combined.contains("executed"),
            "Execute command output missing"
        );

        // Normalize paths in output for snapshot testing
        // Snapshot the output
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_bash_shell_integration_hint_suppressed(repo: TestRepo) {
        // When running through the shell wrapper, the "To enable automatic cd" hint
        // should NOT appear because the user already has shell integration
        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "bash-test"]);

        // Critical: shell integration hint must be suppressed when shell integration is active
        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should not appear when running through wrapper. Output:\n{}",
            output.combined
        );

        // Should still have the success message
        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "Success message missing"
        );

        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_readme_example_simple_switch(repo: TestRepo) {
        // Create worktree through shell wrapper (suppresses hint)
        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "fix-auth"]);

        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should be suppressed"
        );

        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_readme_example_switch_back(repo: TestRepo) {
        // Create worktrees (fix-auth is where we are after step 2, feature-api exists from earlier)
        exec_through_wrapper("bash", &repo, "switch", &["--create", "fix-auth"]);
        // Create feature-api from main (simulating it already existed)
        exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-api"]);

        // Switch to feature-api from fix-auth (showing navigation between worktrees)
        let fix_auth_path = repo.root_path().parent().unwrap().join("repo.fix-auth");
        let output =
            exec_through_wrapper_from("bash", &repo, "switch", &["feature-api"], &fix_auth_path);

        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should be suppressed"
        );

        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_readme_example_remove(repo: TestRepo) {
        // Create worktrees
        exec_through_wrapper("bash", &repo, "switch", &["--create", "fix-auth"]);
        exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-api"]);

        // Remove feature-api from within it (current worktree removal)
        let feature_api_path = repo.root_path().parent().unwrap().join("repo.feature-api");
        let output = exec_through_wrapper_from("bash", &repo, "remove", &[], &feature_api_path);

        assert!(
            !output.combined.contains("To enable automatic cd"),
            "Shell integration hint should be suppressed"
        );

        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_wrapper_preserves_progress_messages(repo: TestRepo) {
        // Configure a post-start background command that will trigger progress output
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'background task'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'background task'"]
"#,
        )
        .unwrap();

        let output = exec_through_wrapper("bash", &repo, "switch", &["--create", "feature-bg"]);

        // No directives should leak
        output.assert_no_directive_leaks();

        output.assert_success();

        // Snapshot verifies progress messages appear to users
        // (catches the bug where progress() was incorrectly suppressed)
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    // ============================================================================
    // Fish Shell Wrapper Tests
    // ============================================================================
    //
    // These tests verify that the Fish shell wrapper correctly:
    // 1. Captures stdout (shell script) via command substitution and evals it
    // 2. Streams stderr (progress, success, hints) to terminal in real-time
    // 3. Never leaks shell script commands to users
    // 4. Preserves exit codes from both wt and executed commands
    //
    // Fish uses `string collect` to join command substitution output into
    // a single string before eval (fish splits on newlines by default).

    #[rstest]
    fn test_fish_wrapper_preserves_progress_messages(repo: TestRepo) {
        // Configure a post-start background command that will trigger progress output
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'fish background task'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command in user config
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'fish background task'"]
"#,
        )
        .unwrap();

        let output = exec_through_wrapper("fish", &repo, "switch", &["--create", "fish-bg"]);

        // No directives should leak
        output.assert_no_directive_leaks();

        output.assert_success();

        // Snapshot verifies progress messages appear to users through Fish wrapper
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_fish_multiline_command_execution(repo: TestRepo) {
        // Test that Fish wrapper handles multi-line commands correctly
        // This tests Fish's NUL-byte parsing with embedded newlines
        // Use actual newlines in the command string
        let multiline_cmd = "echo 'line 1'; echo 'line 2'; echo 'line 3'";

        // Use --yes to skip approval prompt in tests
        let output = exec_through_wrapper(
            "fish",
            &repo,
            "switch",
            &[
                "--create",
                "fish-multiline",
                "--execute",
                multiline_cmd,
                "--yes",
            ],
        );

        // No directives should leak
        output.assert_no_directive_leaks();

        output.assert_success();

        // All three lines should be executed and visible
        assert!(output.combined.contains("line 1"), "First line missing");
        assert!(output.combined.contains("line 2"), "Second line missing");
        assert!(output.combined.contains("line 3"), "Third line missing");

        // Normalize paths in output for snapshot testing
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    #[rstest]
    fn test_fish_wrapper_handles_empty_chunks(repo: TestRepo) {
        // Test edge case: command that produces minimal output
        // This verifies Fish's `test -n "$chunk"` check works correctly
        let output = exec_through_wrapper("fish", &repo, "switch", &["--create", "fish-minimal"]);

        // No directives should leak even with minimal output
        output.assert_no_directive_leaks();

        output.assert_success();

        // Should still show success message
        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "Success message missing from minimal output"
        );

        // Normalize paths in output for snapshot testing
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    // ========================================================================
    // --source Flag Error Passthrough Tests
    // ========================================================================
    //
    // These tests verify that actual error messages pass through correctly
    // when using the --source flag (instead of being hidden with generic
    // wrapper error messages like "Error: cargo build failed").

    // This test runs `cargo run` inside a PTY which can take longer than the
    // default 60s timeout when cargo checks/compiles dependencies. Extended
    // timeout configured in .config/nextest.toml.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_source_flag_forwards_errors(#[case] shell: &str, repo: TestRepo) {
        use std::env;

        // Get the worktrunk source directory (where this test is running from)
        // This is the directory that contains Cargo.toml with the workspace
        let worktrunk_source = canonicalize(&env::current_dir().unwrap()).unwrap();

        // Build a shell script that runs from the worktrunk source directory
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, shell);
        let mut script = String::new();

        // Set environment variables (use shell_quote to handle paths with special chars)
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());
        match shell {
            "fish" => {
                script.push_str(&format!("set -x WORKTRUNK_BIN {}\n", wt_bin_quoted));
                script.push_str(&format!("set -x WORKTRUNK_CONFIG_PATH {}\n", config_quoted));
                script.push_str("set -x CLICOLOR_FORCE 1\n");
            }
            "zsh" => {
                script.push_str("autoload -Uz compinit && compinit -i 2>/dev/null\n");
                script.push_str(&format!("export WORKTRUNK_BIN={}\n", wt_bin_quoted));
                script.push_str(&format!("export WORKTRUNK_CONFIG_PATH={}\n", config_quoted));
                script.push_str("export CLICOLOR_FORCE=1\n");
            }
            _ => {
                // bash
                script.push_str(&format!("export WORKTRUNK_BIN={}\n", wt_bin_quoted));
                script.push_str(&format!("export WORKTRUNK_CONFIG_PATH={}\n", config_quoted));
                script.push_str("export CLICOLOR_FORCE=1\n");
            }
        }

        // Source the wrapper
        script.push_str(&wrapper_script);
        script.push('\n');

        // Try to run wt --source with an invalid subcommand
        // The --source flag triggers cargo build (which succeeds)
        // Then it tries to run 'wt foo' which should fail with "unrecognized subcommand"
        script.push_str("wt --source foo\n");

        // Wrap in subshell to merge stderr
        let final_script = match shell {
            "fish" => format!("begin\n{}\nend 2>&1", script),
            _ => format!("( {} ) 2>&1", script),
        };

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("CLICOLOR_FORCE", "1"),
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("GIT_AUTHOR_NAME", "Test User"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test User"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z"),
            ("LANG", "C"),
            ("LC_ALL", "C"),
            ("SOURCE_DATE_EPOCH", "1735776000"),
        ];

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, &worktrunk_source, &env_vars, &[]);
        let output = ShellOutput {
            combined,
            exit_code,
        };

        // Shell-agnostic assertions
        assert_ne!(output.exit_code, 0, "{}: Command should fail", shell);

        // CRITICAL: Should see wt's actual error message about unrecognized subcommand
        assert!(
            output.combined.contains("unrecognized subcommand"),
            "{}: Should show actual wt error message 'unrecognized subcommand'.\nOutput:\n{}",
            shell,
            output.combined
        );

        // CRITICAL: Should NOT see the old generic wrapper error message
        assert!(
            !output.combined.contains("Error: cargo build failed"),
            "{}: Should not contain old generic error message",
            shell
        );

        // Consolidated snapshot - output should be identical across shells
        // (wt error messages are deterministic)
        shell_wrapper_settings().bind(|| {
            insta::allow_duplicates! {
                assert_snapshot!("source_flag_error_passthrough", &output.combined);
            }
        });
    }

    // ========================================================================
    // Job Control Notification Tests
    // ========================================================================
    //
    // These tests verify that job control notifications ([1] 12345, [1] + done)
    // don't leak into user output. Zsh suppresses these with NO_MONITOR,
    // bash shows them at the next prompt (less intrusive).

    /// Test that zsh doesn't show job control notifications inline
    /// The NO_MONITOR option should suppress [1] 12345 and [1] + done messages
    #[rstest]
    fn test_zsh_no_job_control_notifications(repo: TestRepo) {
        // Configure a post-start command that will trigger background job
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'background job'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'background job'"]
"#,
        )
        .unwrap();

        let output = exec_through_wrapper("zsh", &repo, "switch", &["--create", "zsh-job-test"]);

        output.assert_success();
        output.assert_no_directive_leaks();

        // Critical: zsh should NOT show job control notifications
        // These patterns indicate job control messages leaked through
        assert!(
            !output.combined.contains("[1]"),
            "Zsh should suppress job control notifications with NO_MONITOR.\nOutput:\n{}",
            output.combined
        );
        assert!(
            !output.combined.contains("+ done"),
            "Zsh should suppress job completion notifications.\nOutput:\n{}",
            output.combined
        );
    }

    /// Test that bash job control messages are suppressed in true interactive mode
    ///
    /// Bash shows `[1]+ Done` notifications at prompt-time, not during script execution.
    /// To detect if they leak, we use `exec_bash_truly_interactive` which runs bash without
    /// `-c` and writes commands to the PTY, triggering prompts where notifications appear.
    ///
    /// The shell wrapper suppresses these via two mechanisms (see bash.sh/zsh.zsh templates):
    /// - START notifications (`[1] 12345`): stderr redirection around `&`
    /// - DONE notifications (`[1]+ Done`): `set +m` before backgrounding
    #[rstest]
    fn test_bash_job_control_suppression(repo: TestRepo) {
        // Configure a post-start command that will trigger background job
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'bash background'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'bash background'"]
"#,
        )
        .unwrap();

        // Build the setup script that defines the wt function
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "bash");
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());
        let setup_script = format!(
            "export WORKTRUNK_BIN={}\n\
             export WORKTRUNK_CONFIG_PATH={}\n\
             export CLICOLOR_FORCE=1\n\
             {}",
            wt_bin_quoted, config_quoted, wrapper_script
        );

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("CLICOLOR_FORCE", "1"),
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("GIT_AUTHOR_NAME", "Test User"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test User"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ];

        // Run wt at the prompt (where job notifications appear)
        let (output, exit_code) = exec_bash_truly_interactive(
            &setup_script,
            "wt switch --create bash-job-test",
            repo.root_path(),
            &env_vars,
        );

        assert_eq!(exit_code, 0, "Command should succeed.\nOutput:\n{}", output);

        // Verify the command completed successfully
        assert!(
            output.contains("Created branch") && output.contains("and worktree"),
            "Should show success message.\nOutput:\n{}",
            output
        );

        // Verify no job control messages leak through.
        // The shell wrapper suppresses both START notifications (`[1] 12345` via stderr
        // redirection) and DONE notifications (`[1]+ Done` via `set +m`).
        // This test uses true interactive mode to ensure we'd see them if they leaked.
        assert!(
            !JOB_CONTROL_REGEX.is_match(&output),
            "Output contains job control messages (e.g., '[1] 12345' or '[1]+ Done'):\n{}",
            output
        );
    }

    // ========================================================================
    // Completion Functionality Tests
    // ========================================================================

    /// Test that bash completions are properly registered
    /// Note: Completions are inline in the wrapper script (lazy loading)
    #[rstest]
    fn test_bash_completions_registered(repo: TestRepo) {
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "bash");

        // Script that sources wrapper and checks if completion is registered
        // (completions are inline in the wrapper via lazy loading)
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());
        let script = format!(
            r#"
            export WORKTRUNK_BIN={}
            export WORKTRUNK_CONFIG_PATH={}
            {}
            # Check if wt completion is registered
            complete -p wt 2>/dev/null && echo "__COMPLETION_REGISTERED__" || echo "__NO_COMPLETION__"
            "#,
            wt_bin_quoted, config_quoted, wrapper_script
        );

        let final_script = format!("( {} ) 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> =
            vec![("WORKTRUNK_CONFIG_PATH", &config_path), ("TERM", "xterm")];

        let (combined, exit_code) =
            exec_in_pty_interactive("bash", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0);
        assert!(
            combined.contains("__COMPLETION_REGISTERED__"),
            "Bash completions should be registered after sourcing wrapper.\nOutput:\n{}",
            combined
        );
    }

    /// Test that fish completions are properly registered
    #[rstest]
    fn test_fish_completions_registered(repo: TestRepo) {
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "fish");
        let completions_script = generate_completions(&repo, "fish");

        // Script that sources wrapper, completions, and checks if completion is registered
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());
        let script = format!(
            r#"
            set -x WORKTRUNK_BIN {}
            set -x WORKTRUNK_CONFIG_PATH {}
            {}
            {}
            # Check if wt completions are registered
            if complete -c wt 2>/dev/null | grep -q .
                echo "__COMPLETION_REGISTERED__"
            else
                echo "__NO_COMPLETION__"
            end
            "#,
            wt_bin_quoted, config_quoted, wrapper_script, completions_script
        );

        let final_script = format!("begin\n{}\nend 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> =
            vec![("WORKTRUNK_CONFIG_PATH", &config_path), ("TERM", "xterm")];

        let (combined, exit_code) =
            exec_in_pty_interactive("fish", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0);
        assert!(
            combined.contains("__COMPLETION_REGISTERED__"),
            "Fish completions should be registered after sourcing wrapper.\nOutput:\n{}",
            combined
        );
    }

    /// Test that zsh wrapper function is properly defined
    /// Note: Completions are inline in the wrapper script (lazy loading via compdef)
    #[rstest]
    fn test_zsh_wrapper_function_registered(repo: TestRepo) {
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, "zsh");

        // Use a marker file to avoid PTY output race conditions.
        // PTY buffer flushing is unreliable on CI, so we write to a file and poll for it.
        let marker_file = repo.root_path().join(".wrapper_test_marker");
        let marker_path = marker_file.to_string_lossy().to_string();

        // Script that sources wrapper and checks if wt function exists
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());
        let marker_quoted = shell_quote(&marker_path);
        let script = format!(
            r#"
            export WORKTRUNK_BIN={wt_bin}
            export WORKTRUNK_CONFIG_PATH={config}
            {wrapper}
            # Check if wt wrapper function is defined and write result to marker file
            if (( $+functions[wt] )); then
                echo "__WRAPPER_REGISTERED__" > {marker}
            else
                echo "__NO_WRAPPER__" > {marker}
            fi
            "#,
            wt_bin = wt_bin_quoted,
            config = config_quoted,
            wrapper = wrapper_script,
            marker = marker_quoted,
        );

        let final_script = format!("( {} ) 2>&1", script);
        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars: Vec<(&str, &str)> = vec![
            ("WORKTRUNK_CONFIG_PATH", &config_path),
            ("TERM", "xterm"),
            ("ZDOTDIR", "/dev/null"),
        ];

        let (_combined, exit_code) =
            exec_in_pty_interactive("zsh", &final_script, repo.root_path(), &env_vars, &[]);

        assert_eq!(exit_code, 0);

        // Poll for marker file instead of relying on PTY output
        wait_for_file_content(&marker_file);
        let result = std::fs::read_to_string(&marker_file).unwrap();
        assert!(
            result.contains("__WRAPPER_REGISTERED__"),
            "Zsh wrapper function should be registered after sourcing.\nMarker file content:\n{}",
            result
        );
    }

    // ========================================================================
    // Special Characters in Branch Names Tests
    // ========================================================================

    /// Test that branch names with special characters work correctly
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_branch_name_with_slashes(#[case] shell: &str, repo: TestRepo) {
        // Branch name with slashes (common git convention)
        let output =
            exec_through_wrapper(shell, &repo, "switch", &["--create", "feature/test-branch"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should create worktree for branch with slashes",
            shell
        );
    }

    /// Test that branch names with dashes and underscores work
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_branch_name_with_dashes_underscores(#[case] shell: &str, repo: TestRepo) {
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "fix-bug_123"]);

        assert_eq!(output.exit_code, 0, "{}: Command should succeed", shell);
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should create worktree for branch with dashes/underscores",
            shell
        );
    }

    // ========================================================================
    // WORKTRUNK_BIN Fallback Tests
    // ========================================================================

    /// Test that shell integration works when wt is not in PATH but WORKTRUNK_BIN is set
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_worktrunk_bin_fallback(#[case] shell: &str, repo: TestRepo) {
        let wt_bin = get_cargo_bin("wt");
        let wrapper_script = generate_wrapper(&repo, shell);

        // Use shell_quote to handle paths with special chars (like single quotes)
        let wt_bin_quoted = shell_quote(&wt_bin.display().to_string());
        let config_quoted = shell_quote(&repo.test_config_path().display().to_string());

        // Script that explicitly removes wt from PATH but sets WORKTRUNK_BIN
        let script = match shell {
            "zsh" => format!(
                r#"
                autoload -Uz compinit && compinit -i 2>/dev/null
                # Clear PATH to ensure wt is not found via PATH
                export PATH="/usr/bin:/bin"
                export WORKTRUNK_BIN={}
                export WORKTRUNK_CONFIG_PATH={}
                export CLICOLOR_FORCE=1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin_quoted, config_quoted, wrapper_script
            ),
            "fish" => format!(
                r#"
                # Clear PATH to ensure wt is not found via PATH
                set -x PATH /usr/bin /bin
                set -x WORKTRUNK_BIN {}
                set -x WORKTRUNK_CONFIG_PATH {}
                set -x CLICOLOR_FORCE 1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin_quoted, config_quoted, wrapper_script
            ),
            _ => format!(
                r#"
                # Clear PATH to ensure wt is not found via PATH
                export PATH="/usr/bin:/bin"
                export WORKTRUNK_BIN={}
                export WORKTRUNK_CONFIG_PATH={}
                export CLICOLOR_FORCE=1
                {}
                wt switch --create fallback-test
                echo "__PWD__ $PWD"
                "#,
                wt_bin_quoted, config_quoted, wrapper_script
            ),
        };

        let final_script = match shell {
            "fish" => format!("begin\n{}\nend 2>&1", script),
            _ => format!("( {} ) 2>&1", script),
        };

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars = build_test_env_vars(&config_path);

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, repo.root_path(), &env_vars, &[]);

        let output = ShellOutput {
            combined,
            exit_code,
        };

        assert_eq!(
            output.exit_code, 0,
            "{}: Command should succeed with WORKTRUNK_BIN fallback",
            shell
        );
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should create worktree using WORKTRUNK_BIN fallback.\nOutput:\n{}",
            shell,
            output.combined
        );

        // Verify we actually cd'd to the new worktree
        assert!(
            output.combined.contains("fallback-test"),
            "{}: Should be in the new worktree directory.\nOutput:\n{}",
            shell,
            output.combined
        );
    }

    /// Test that fish wrapper shows clear error when wt binary is not available
    ///
    /// This tests the scenario where:
    /// 1. User has shell integration installed (functions/wt.fish exists)
    /// 2. But wt binary is not in PATH
    /// 3. And WORKTRUNK_BIN is not set
    ///
    /// The fish function should show "wt: command not found" and exit 127.
    /// This is fish-specific because bash/zsh have an outer guard that prevents
    /// the function from being defined when wt isn't available.
    #[rstest]
    #[case("fish")]
    fn test_fish_binary_not_found_clear_error(#[case] shell: &str, repo: TestRepo) {
        let wrapper_script = generate_wrapper(&repo, shell);

        // Script that clears PATH and does NOT set WORKTRUNK_BIN
        // This simulates having the fish function installed but wt not available
        let script = format!(
            r#"
            # Clear PATH to ensure wt is not found via PATH
            set -x PATH /usr/bin /bin
            # Explicitly unset WORKTRUNK_BIN to ensure it's not set
            set -e WORKTRUNK_BIN
            set -x CLICOLOR_FORCE 1
            {}
            wt --version
            echo "exit_code: $status"
            "#,
            wrapper_script
        );

        let final_script = format!("begin\n{}\nend 2>&1", script);

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars = build_test_env_vars(&config_path);

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, repo.root_path(), &env_vars, &[]);

        let output = ShellOutput {
            combined,
            exit_code,
        };

        // The function should show a clear error message
        assert!(
            output.combined.contains("wt: command not found"),
            "Fish wrapper should show 'wt: command not found' when binary is missing.\nOutput:\n{}",
            output.combined
        );

        // And return exit code 127 (standard "command not found" exit code)
        assert!(
            output.combined.contains("exit_code: 127"),
            "Fish wrapper should return exit code 127 when binary is missing.\nOutput:\n{}",
            output.combined
        );
    }

    /// Test that fish WRAPPER (bootstrap) handles missing binary gracefully
    ///
    /// This tests the WRAPPER file (fish_wrapper.fish) that gets installed to
    /// ~/.config/fish/functions/wt.fish. Unlike the full function (tested above),
    /// the wrapper tries to SOURCE the full function from the binary at runtime.
    ///
    /// When wt isn't in PATH:
    /// - `command wt config shell init fish` fails
    /// - The wrapper should return 127, NOT infinite loop
    ///
    /// This is different from test_fish_binary_not_found_clear_error which tests
    /// the FULL function (which has its own WORKTRUNK_BIN check).
    #[rstest]
    #[case("fish")]
    fn test_fish_wrapper_binary_not_found_no_infinite_loop(#[case] shell: &str, repo: TestRepo) {
        // Use the WRAPPER template (not the full function from generate_wrapper)
        let init = shell::ShellInit::with_prefix(shell::Shell::Fish, "wt".to_string());
        let wrapper_content = init.generate_fish_wrapper().unwrap();

        // Script that clears PATH so wt isn't found, then calls wt
        let script = format!(
            r#"
            # Clear PATH to ensure wt is not found
            set -x PATH /usr/bin /bin
            set -x CLICOLOR_FORCE 1
            {}
            wt --version
            echo "exit_code: $status"
            "#,
            wrapper_content
        );

        let final_script = format!("begin\n{}\nend 2>&1", script);

        let config_path = repo.test_config_path().to_string_lossy().to_string();
        let env_vars = build_test_env_vars(&config_path);

        let (combined, exit_code) =
            exec_in_pty_interactive(shell, &final_script, repo.root_path(), &env_vars, &[]);

        let output = ShellOutput {
            combined,
            exit_code,
        };

        // Should NOT show signs of infinite recursion (stack overflow, repeated function calls)
        // One occurrence of "in function 'wt'" is normal (fish's error trace).
        // Infinite recursion would show this MANY times.
        let function_call_count = output.combined.matches("in function 'wt'").count();
        assert!(
            function_call_count <= 1,
            "Fish wrapper should not infinite loop (found {} recursive calls).\nOutput:\n{}",
            function_call_count,
            output.combined
        );

        // Should return exit code 127 (command not found)
        assert!(
            output.combined.contains("exit_code: 127"),
            "Fish wrapper should return exit code 127 when binary is missing.\nOutput:\n{}",
            output.combined
        );

        // Should show fish's error message about unknown command
        assert!(
            output.combined.contains("Unknown command")
                || output.combined.contains("command not found"),
            "Fish wrapper should show error when binary is missing.\nOutput:\n{}",
            output.combined
        );
    }

    // ========================================================================
    // Interrupt/Cleanup Tests
    // ========================================================================

    /// Test that shell integration completes without leaving zombie processes
    /// Note: Temp directory cleanup is verified implicitly by successful test completion.
    /// We can't check for specific temp files because tests run in parallel.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_shell_completes_cleanly(#[case] shell: &str, repo: TestRepo) {
        // Configure a post-start command to exercise the background job code path
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("wt.toml"),
            r#"post-start = "echo 'cleanup test'""#,
        )
        .unwrap();

        repo.commit("Add post-start command");

        // Pre-approve the command
        fs::write(
            repo.test_config_path(),
            r#"[projects."../origin"]
approved-commands = ["echo 'cleanup test'"]
"#,
        )
        .unwrap();

        // Run a command that exercises the full FIFO/background job code path
        let output = exec_through_wrapper(shell, &repo, "switch", &["--create", "cleanup-test"]);

        // Verify command completed successfully
        // If cleanup failed (e.g., FIFO not removed, zombie process),
        // the command would hang or fail
        assert_eq!(
            output.exit_code, 0,
            "{}: Command should complete cleanly",
            shell
        );
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "{}: Should complete successfully",
            shell
        );
    }

    // ========================================================================
    // README Example Tests (PTY-based for interleaved output)
    // ========================================================================
    //
    // These tests generate snapshots for README.md examples. They use PTY execution
    // to capture stdout/stderr interleaved in the order users see them.
    //
    // See tests/CLAUDE.md for background on why PTY-based tests are needed for README examples.

    /// README example: Pre-merge hooks with squash and LLM commit message
    ///
    /// This test demonstrates:
    /// - Multiple commits being squashed with LLM commit message
    /// - Pre-merge hooks (test, lint) running before merge
    ///
    /// Source: tests/snapshots/shell_wrapper__tests__readme_example_hooks_pre_merge.snap
    #[rstest]
    fn test_readme_example_hooks_pre_merge(mut repo: TestRepo) {
        // Create project config with pre-merge hooks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();

        // Create mock commands for realistic output
        let bin_dir = repo.root_path().join(".bin");
        fs::create_dir_all(&bin_dir).unwrap();

        // Mock pytest command
        let pytest_script = r#"#!/bin/sh
cat << 'EOF'

============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================

EOF
exit 0
"#;
        fs::write(bin_dir.join("pytest"), pytest_script).unwrap();

        // Mock ruff command
        let ruff_script = r#"#!/bin/sh
if [ "$1" = "check" ]; then
    echo ""
    echo "All checks passed!"
    echo ""
    exit 0
else
    echo "ruff: unknown command '$1'"
    exit 1
fi
"#;
        fs::write(bin_dir.join("ruff"), ruff_script).unwrap();

        // Mock llm command for commit message
        let llm_script = r#"#!/bin/sh
cat > /dev/null
cat << 'EOF'
feat(api): Add user authentication endpoints

Implement login and token refresh endpoints with JWT validation.
Includes comprehensive test coverage and input validation.
EOF
"#;
        fs::write(bin_dir.join("llm"), llm_script).unwrap();

        // Mock uv command for running pytest and ruff
        let uv_script = r#"#!/bin/sh
if [ "$1" = "run" ] && [ "$2" = "pytest" ]; then
    exec pytest
elif [ "$1" = "run" ] && [ "$2" = "ruff" ]; then
    shift 2
    exec ruff "$@"
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
        fs::write(bin_dir.join("uv"), uv_script).unwrap();

        // Make scripts executable (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in &["pytest", "ruff", "llm", "uv"] {
                let mut perms = fs::metadata(bin_dir.join(script)).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(bin_dir.join(script), perms).unwrap();
            }
        }

        let config_content = r#"
[pre-merge]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
"#;

        fs::write(config_dir.join("wt.toml"), config_content).unwrap();

        // Commit the config
        repo.run_git(&["add", ".config/wt.toml", ".bin"]);
        repo.run_git(&["commit", "-m", "Add pre-merge hooks"]);

        // Create a feature worktree and make multiple commits
        let feature_wt = repo.add_worktree("feature-auth");

        // First commit - create initial auth.py with login endpoint
        fs::create_dir_all(feature_wt.join("api")).unwrap();
        let auth_py_v1 = r#"# Authentication API endpoints
from typing import Dict, Optional
import jwt
from datetime import datetime, timedelta, timezone

def login(username: str, password: str) -> Optional[Dict]:
    """Authenticate user and return JWT token."""
    # Validate credentials (stub)
    if not username or not password:
        return None

    # Generate JWT token
    payload = {
        'sub': username,
        'exp': datetime.now(timezone.utc) + timedelta(hours=1)
    }
    token = jwt.encode(payload, 'secret', algorithm='HS256')
    return {'token': token, 'expires_in': 3600}
"#;
        std::fs::write(feature_wt.join("api/auth.py"), auth_py_v1).unwrap();
        repo.run_git_in(&feature_wt, &["add", "api/auth.py"]);
        repo.run_git_in(&feature_wt, &["commit", "-m", "Add login endpoint"]);

        // Second commit - add tests
        fs::create_dir_all(feature_wt.join("tests")).unwrap();
        let test_auth_py = r#"# Authentication endpoint tests
import pytest
from api.auth import login

def test_login_success():
    result = login('user', 'pass')
    assert result and 'token' in result

def test_login_invalid_password():
    result = login('user', '')
    assert result is None

def test_token_validation():
    assert login('valid_user', 'valid_pass')['expires_in'] == 3600
"#;
        std::fs::write(feature_wt.join("tests/test_auth.py"), test_auth_py).unwrap();
        repo.run_git_in(&feature_wt, &["add", "tests/test_auth.py"]);
        repo.run_git_in(&feature_wt, &["commit", "-m", "Add authentication tests"]);

        // Third commit - add refresh endpoint
        let auth_py_v2 = r#"# Authentication API endpoints
from typing import Dict, Optional
import jwt
from datetime import datetime, timedelta, timezone

def login(username: str, password: str) -> Optional[Dict]:
    """Authenticate user and return JWT token."""
    # Validate credentials (stub)
    if not username or not password:
        return None

    # Generate JWT token
    payload = {
        'sub': username,
        'exp': datetime.now(timezone.utc) + timedelta(hours=1)
    }
    token = jwt.encode(payload, 'secret', algorithm='HS256')
    return {'token': token, 'expires_in': 3600}

def refresh_token(token: str) -> Optional[Dict]:
    """Refresh an existing JWT token."""
    try:
        payload = jwt.decode(token, 'secret', algorithms=['HS256'])
        new_payload = {
            'sub': payload['sub'],
            'exp': datetime.now(timezone.utc) + timedelta(hours=1)
        }
        new_token = jwt.encode(new_payload, 'secret', algorithm='HS256')
        return {'token': new_token, 'expires_in': 3600}
    except jwt.InvalidTokenError:
        return None
"#;
        std::fs::write(feature_wt.join("api/auth.py"), auth_py_v2).unwrap();
        repo.run_git_in(&feature_wt, &["add", "api/auth.py"]);
        repo.run_git_in(&feature_wt, &["commit", "-m", "Add validation"]);

        // Configure LLM in worktrunk config
        let llm_path = bin_dir.join("llm");
        let worktrunk_config = format!(
            r#"worktree-path = "../repo.{{{{ branch }}}}"

[commit-generation]
command = "{}"
"#,
            llm_path.display()
        );
        fs::write(repo.test_config_path(), worktrunk_config).unwrap();

        // Set PATH with mock binaries and run merge
        let path_with_bin = format!(
            "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            bin_dir.display()
        );

        let output = exec_through_wrapper_with_env(
            "bash",
            &repo,
            "merge",
            &["main", "--yes"],
            &feature_wt,
            &[],
            &[("PATH", &path_with_bin)],
        );

        output.assert_success();
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    /// README example: Creating worktree with post-create and post-start hooks
    ///
    /// This test demonstrates:
    /// - Post-create hooks (install dependencies)
    /// - Post-start hooks (start dev server)
    ///
    /// Uses shell wrapper to avoid "To enable automatic cd" hint.
    ///
    /// Source: tests/snapshots/shell_wrapper__tests__readme_example_hooks_post_create.snap
    #[rstest]
    fn test_readme_example_hooks_post_create(repo: TestRepo) {
        // Create project config with post-create and post-start hooks
        let config_dir = repo.root_path().join(".config");
        fs::create_dir_all(&config_dir).unwrap();

        // Create mock commands for realistic output
        let bin_dir = repo.root_path().join(".bin");
        fs::create_dir_all(&bin_dir).unwrap();

        // Mock uv command that simulates dependency installation
        let uv_script = r#"#!/bin/sh
if [ "$1" = "sync" ]; then
    echo ""
    echo "  Resolved 24 packages in 145ms"
    echo "  Installed 24 packages in 1.2s"
    exit 0
elif [ "$1" = "run" ] && [ "$2" = "dev" ]; then
    echo ""
    echo "  Starting dev server on http://localhost:3000..."
    exit 0
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
        fs::write(bin_dir.join("uv"), uv_script).unwrap();

        // Make scripts executable (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(bin_dir.join("uv")).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(bin_dir.join("uv"), perms).unwrap();
        }

        let config_content = r#"
[post-create]
"install" = "uv sync"

[post-start]
"dev" = "uv run dev"
"#;

        fs::write(config_dir.join("wt.toml"), config_content).unwrap();

        // Commit the config
        repo.run_git(&["add", ".config/wt.toml", ".bin"]);
        repo.run_git(&["commit", "-m", "Add project hooks"]);

        // Set PATH with mock binaries and run switch --create
        let path_with_bin = format!(
            "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            bin_dir.display()
        );

        let output = exec_through_wrapper_with_env(
            "bash",
            &repo,
            "switch",
            &["--create", "feature-x", "--yes"],
            repo.root_path(),
            &[],
            &[("PATH", &path_with_bin)],
        );

        output.assert_success();
        shell_wrapper_settings().bind(|| assert_snapshot!(&output.combined));
    }

    /// README example: approval prompt for post-create commands
    /// This test captures just the prompt (before responding) to show what users see.
    ///
    /// Note: This uses direct PTY execution (not shell wrapper) because interactive prompts
    /// require direct stdin access. The shell wrapper approach detects non-interactive mode.
    /// The shell integration hint is truncated from the output.
    #[rstest]
    fn test_readme_example_approval_prompt(repo: TestRepo) {
        use portable_pty::CommandBuilder;
        use std::io::{Read, Write};

        // Remove origin so worktrunk uses directory name as project identifier
        repo.run_git(&["remote", "remove", "origin"]);

        // Create project config with named post-create commands
        repo.write_project_config(
            r#"[post-create]
install = "echo 'Installing dependencies...'"
build = "echo 'Building project...'"
test = "echo 'Running tests...'"
"#,
        );
        repo.commit("Add config");

        let pair = crate::common::open_pty();

        let cargo_bin = get_cargo_bin("wt");
        let mut cmd = CommandBuilder::new(cargo_bin);
        cmd.arg("switch");
        cmd.arg("--create");
        cmd.arg("test-approval");
        cmd.cwd(repo.root_path());

        // Set environment
        cmd.env_clear();
        cmd.env(
            "HOME",
            home::home_dir().unwrap().to_string_lossy().to_string(),
        );
        cmd.env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        );
        for (key, value) in repo.test_env_vars() {
            cmd.env(key, value);
        }

        // Pass through LLVM coverage env vars for subprocess coverage collection
        crate::common::pass_coverage_env_to_pty_cmd(&mut cmd);

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();

        // Send "n" to decline and complete the command
        writer.write_all(b"n\n").unwrap();
        writer.flush().unwrap();
        drop(writer);

        // Read all output
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        child.wait().unwrap();

        // Normalize: strip ANSI codes and control characters
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let output = ansi_regex
            .replace_all(&buf, "")
            .replace("\r\n", "\n")
            .to_string();

        // Remove ^D and backspaces (macOS PTY artifacts)
        let ctrl_d_regex = regex::Regex::new(r"\^D\x08+").unwrap();
        let output = ctrl_d_regex.replace_all(&output, "").to_string();

        // Normalize paths (local regexes since we're extracting content, not snapshotting)
        let tmpdir_regex = regex::Regex::new(
            r#"(?:/private)?/var/folders/[^/]+/[^/]+/T/\.tmp[^\s/'\x1b\)]+|/tmp/\.tmp[^\s/'\x1b\)]+"#,
        )
        .unwrap();
        let output = tmpdir_regex.replace_all(&output, "[TMPDIR]").to_string();
        let collapse_regex = regex::Regex::new(r"\[TMPDIR](?:/?\[TMPDIR])+").unwrap();
        let output = collapse_regex.replace_all(&output, "[TMPDIR]").to_string();

        assert!(
            output.contains("needs approval"),
            "Should show approval prompt"
        );
        assert!(
            output.contains("[y/N]"),
            "Should show the interactive prompt"
        );

        // Extract just the prompt portion (from "🟡" to "[y/N]")
        // This removes the echoed input at the start and anything after the prompt
        let prompt_start = output.find("🟡").unwrap_or(0);
        let prompt_end = output.find("[y/N]").map(|i| i + "[y/N]".len());
        let prompt_only = if let Some(end) = prompt_end {
            output[prompt_start..end].trim().to_string()
        } else {
            output[prompt_start..].trim().to_string()
        };

        assert_snapshot!(prompt_only);
    }

    /// Black-box test: bash completion is registered and produces correct output.
    ///
    /// This test verifies completion works WITHOUT knowing internal function names.
    /// It uses `complete -p wt` to discover whatever completion function is registered,
    /// then calls it via shell completion machinery.
    ///
    /// This catches bugs like:
    /// - Completion not registered at all
    /// - Completion function not loading (lazy loading broken)
    /// - Completion output being executed as commands (the COMPLETE mode bug)
    #[rstest]
    fn test_bash_completion_produces_correct_output(repo: TestRepo) {
        use std::io::Read;

        let wt_bin = get_cargo_bin("wt");
        let wt_bin_dir = wt_bin.parent().unwrap();

        // Generate wrapper without WORKTRUNK_BIN (simulates installed wt)
        let output = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", "bash"])
            .output()
            .unwrap();
        let wrapper_script = String::from_utf8_lossy(&output.stdout);

        // Black-box test: don't reference internal function names
        let script = format!(
            r#"
# Do NOT set WORKTRUNK_BIN - simulate real user scenario
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Step 1: Verify SOME completion is registered for 'wt' (black-box check)
if ! complete -p wt >/dev/null 2>&1; then
    echo "FAILURE: No completion registered for wt"
    exit 1
fi
echo "SUCCESS: Completion is registered for wt"

# Step 2: Get the completion function name (whatever it's called)
completion_func=$(complete -p wt 2>/dev/null | sed -n 's/.*-F \([^ ]*\).*/\1/p')
if [[ -z "$completion_func" ]]; then
    echo "FAILURE: Could not extract completion function name"
    exit 1
fi
echo "SUCCESS: Found completion function: $completion_func"

# Step 3: Set up completion environment and call the function
COMP_WORDS=(wt "")
COMP_CWORD=1
COMP_TYPE=9  # TAB
COMP_LINE="wt "
COMP_POINT=${{#COMP_LINE}}

# Call the completion function (this triggers lazy loading if needed)
"$completion_func" wt "" wt 2>&1

# Step 4: Verify we got completions (black-box: just check we got results)
if [[ "${{#COMPREPLY[@]}}" -eq 0 ]]; then
    echo "FAILURE: No completions returned"
    echo "COMPREPLY is empty"
    exit 1
fi
echo "SUCCESS: Got ${{#COMPREPLY[@]}} completions"

# Print completions
for c in "${{COMPREPLY[@]}}"; do
    echo "  - $c"
done

# Step 5: Verify expected subcommands are present
if printf '%s\n' "${{COMPREPLY[@]}}" | grep -q '^config$'; then
    echo "VERIFIED: 'config' is in completions"
else
    echo "FAILURE: 'config' not found in completions"
    exit 1
fi
if printf '%s\n' "${{COMPREPLY[@]}}" | grep -q '^list$'; then
    echo "VERIFIED: 'list' is in completions"
else
    echo "FAILURE: 'list' not found in completions"
    exit 1
fi
"#,
            wrapper_script = wrapper_script
        );

        let pair = crate::common::open_pty();

        let mut cmd = crate::common::shell_command("bash", Some(wt_bin_dir));
        cmd.arg("-c");
        cmd.arg(&script);
        cmd.cwd(repo.root_path());

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let status = child.wait().unwrap();
        let output = buf.replace("\r\n", "\n");

        // Verify no "command not found" error (the COMPLETE mode bug)
        assert!(
            !output.contains("command not found"),
            "Completion output should NOT be executed as a command.\n\
             This indicates the COMPLETE mode fix is not working.\n\
             Output: {}",
            output
        );

        assert!(
            output.contains("SUCCESS: Completion is registered"),
            "Completion should be registered.\nOutput: {}\nExit: {}",
            output,
            status.exit_code()
        );

        assert!(
            output.contains("SUCCESS: Got") && output.contains("completions"),
            "Completion should return results.\nOutput: {}\nExit: {}",
            output,
            status.exit_code()
        );

        assert!(
            output.contains("VERIFIED: 'config' is in completions"),
            "Expected 'config' subcommand in completions.\nOutput: {}",
            output
        );

        assert!(
            output.contains("VERIFIED: 'list' is in completions"),
            "Expected 'list' subcommand in completions.\nOutput: {}",
            output
        );
    }

    /// Black-box test: zsh completion is registered and produces correct output.
    ///
    /// This test verifies completion works WITHOUT knowing internal function names.
    /// It checks that a completion is registered for 'wt' and that calling the
    /// wt command with COMPLETE=zsh produces completion candidates.
    #[rstest]
    fn test_zsh_completion_produces_correct_output(repo: TestRepo) {
        use std::io::Read;

        let wt_bin = get_cargo_bin("wt");
        let wt_bin_dir = wt_bin.parent().unwrap();

        // Generate wrapper without WORKTRUNK_BIN (simulates installed wt)
        let output = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", "zsh"])
            .output()
            .unwrap();
        let wrapper_script = String::from_utf8_lossy(&output.stdout);

        // Black-box test: don't reference internal function names
        let script = format!(
            r#"
autoload -Uz compinit && compinit -i 2>/dev/null

# Do NOT set WORKTRUNK_BIN - simulate real user scenario
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Step 1: Verify SOME completion is registered for 'wt' (black-box check)
# In zsh, $_comps[wt] contains the completion function if registered
if (( $+_comps[wt] )); then
    echo "SUCCESS: Completion is registered for wt"
else
    echo "FAILURE: No completion registered for wt"
    exit 1
fi

# Step 2: Test that COMPLETE mode works through our shell function
# This is the key test - the wt() shell function must detect COMPLETE
# and call the binary directly, not through wt_exec which would eval the output
words=(wt "")
CURRENT=2
_CLAP_COMPLETE_INDEX=1
_CLAP_IFS=$'\n'

# Call wt with COMPLETE=zsh - this goes through our shell function
completions=$(COMPLETE=zsh _CLAP_IFS="$_CLAP_IFS" _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" wt -- "${{words[@]}}" 2>&1)

if [[ -z "$completions" ]]; then
    echo "FAILURE: No completions returned"
    exit 1
fi
echo "SUCCESS: Got completions"

# Print first few completions
echo "$completions" | head -10 | while read line; do
    echo "  - $line"
done

# Step 3: Verify expected subcommands are present
if echo "$completions" | grep -q 'config'; then
    echo "VERIFIED: 'config' is in completions"
else
    echo "FAILURE: 'config' not found in completions"
    exit 1
fi
"#,
            wrapper_script = wrapper_script
        );

        let pair = crate::common::open_pty();

        let mut cmd = crate::common::shell_command("zsh", Some(wt_bin_dir));
        cmd.arg("-c");
        cmd.arg(&script);
        cmd.cwd(repo.root_path());

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let status = child.wait().unwrap();
        let output = buf.replace("\r\n", "\n");

        // Verify no "command not found" error (the COMPLETE mode bug)
        assert!(
            !output.contains("command not found"),
            "Completion output should NOT be executed as a command.\n\
             Output: {}",
            output
        );

        assert!(
            output.contains("SUCCESS: Completion is registered"),
            "Completion should be registered.\nOutput: {}\nExit: {}",
            output,
            status.exit_code()
        );

        assert!(
            output.contains("SUCCESS: Got completions"),
            "Completion should return results.\nOutput: {}\nExit: {}",
            output,
            status.exit_code()
        );

        assert!(
            output.contains("VERIFIED: 'config' is in completions"),
            "Expected 'config' subcommand in completions.\nOutput: {}",
            output
        );
    }

    /// Black-box test: zsh completion produces correct subcommands.
    ///
    /// Sources actual `wt config shell init zsh`, triggers completion, snapshots result.
    #[test]
    fn test_zsh_completion_subcommands() {
        let wt_bin = get_cargo_bin("wt");
        let init = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", "zsh"])
            .output()
            .unwrap();
        let shell_integration = String::from_utf8_lossy(&init.stdout);

        // Override _describe to print completions (it normally writes to zsh's internal state)
        let script = format!(
            r#"
autoload -Uz compinit && compinit -i 2>/dev/null
_describe() {{
    while [[ "$1" == -* ]]; do shift; done; shift
    for arr in "$@"; do for item in "${{(@P)arr}}"; do echo "${{item%%:*}}"; done; done
}}
{shell_integration}
words=(wt "") CURRENT=2
_wt_lazy_complete
"#
        );

        let output = std::process::Command::new("zsh")
            .arg("-c")
            .arg(&script)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    wt_bin.parent().unwrap().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap();

        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    }

    /// Black-box test: bash completion produces correct subcommands.
    ///
    /// Sources actual `wt config shell init bash`, triggers completion, snapshots result.
    #[test]
    fn test_bash_completion_subcommands() {
        let wt_bin = get_cargo_bin("wt");
        let init = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", "bash"])
            .output()
            .unwrap();
        let shell_integration = String::from_utf8_lossy(&init.stdout);

        let script = format!(
            r#"
{shell_integration}
COMP_WORDS=(wt "") COMP_CWORD=1
_wt_lazy_complete
for c in "${{COMPREPLY[@]}}"; do echo "${{c%%	*}}"; done
"#
        );

        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    wt_bin.parent().unwrap().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap();

        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    }

    /// Black-box test: fish completion produces correct subcommands.
    ///
    /// Fish completions call binary with COMPLETE=fish (separate from init script).
    #[test]
    fn test_fish_completion_subcommands() {
        let wt_bin = get_cargo_bin("wt");

        let output = std::process::Command::new(&wt_bin)
            .args(["--", "wt", ""])
            .env("COMPLETE", "fish")
            .env("_CLAP_COMPLETE_INDEX", "1")
            .output()
            .unwrap();

        // Fish format is "value\tdescription" - extract just values
        let completions: String = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.split('\t').next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert_snapshot!(completions);
    }

    // ========================================================================
    // Stderr/Stdout Redirection Tests
    // ========================================================================
    //
    // These tests verify that output redirection works correctly through the
    // shell wrapper. When a user runs `wt --help &> file`, ALL output should
    // go to the file - nothing should leak to the terminal.
    //
    // This is particularly important for fish where command substitution `(...)`
    // doesn't propagate stderr redirects from the calling function.

    /// Test that `wt --help &> file` redirects all output to the file.
    ///
    /// This test verifies that stderr redirection works correctly through the
    /// shell wrapper. The issue being tested: in some shells (particularly fish),
    /// command substitution doesn't propagate stderr redirects, causing help
    /// output to appear on the terminal even when redirected.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_help_redirect_captures_all_output(#[case] shell: &str, repo: TestRepo) {
        use std::io::Read;

        let wt_bin = get_cargo_bin("wt");
        let wt_bin_dir = wt_bin.parent().unwrap();

        // Create a temp file for the redirect target
        let tmp_dir = tempfile::tempdir().unwrap();
        let redirect_file = tmp_dir.path().join("output.log");
        let redirect_path = redirect_file.display().to_string();

        // Generate wrapper script
        let output = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", shell])
            .output()
            .unwrap();
        let wrapper_script = String::from_utf8_lossy(&output.stdout);

        // Build shell script that:
        // 1. Sources the wrapper
        // 2. Runs `wt --help &> file`
        // 3. Echoes a marker so we know the script completed
        let script = match shell {
            "fish" => format!(
                r#"
set -x WORKTRUNK_BIN '{wt_bin}'
set -x CLICOLOR_FORCE 1

# Source the shell integration
{wrapper_script}

# Run help with redirect - ALL output should go to file
wt --help &>'{redirect_path}'

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                wrapper_script = wrapper_script,
                redirect_path = redirect_path,
            ),
            "zsh" => format!(
                r#"
autoload -Uz compinit && compinit -i 2>/dev/null
export WORKTRUNK_BIN='{wt_bin}'
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Run help with redirect - ALL output should go to file
wt --help &>'{redirect_path}'

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                wrapper_script = wrapper_script,
                redirect_path = redirect_path,
            ),
            _ => format!(
                r#"
export WORKTRUNK_BIN='{wt_bin}'
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Run help with redirect - ALL output should go to file
wt --help &>'{redirect_path}'

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                wrapper_script = wrapper_script,
                redirect_path = redirect_path,
            ),
        };

        let pair = crate::common::open_pty();

        let mut cmd = crate::common::shell_command(shell, Some(wt_bin_dir));
        cmd.arg("-c");
        cmd.arg(&script);
        cmd.cwd(repo.root_path());

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let _status = child.wait().unwrap();
        let terminal_output = buf.replace("\r\n", "\n");

        // Read the redirect file
        let file_content = fs::read_to_string(&redirect_file).unwrap_or_else(|e| {
            panic!(
                "{}: Failed to read redirect file: {}\nTerminal output:\n{}",
                shell, e, terminal_output
            )
        });

        // Verify script completed
        assert!(
            terminal_output.contains("SCRIPT_COMPLETED"),
            "{}: Script did not complete successfully.\nTerminal output:\n{}",
            shell,
            terminal_output
        );

        // Verify help content went to the file
        assert!(
            file_content.contains("Usage:") || file_content.contains("wt"),
            "{}: Help content should be in the redirect file.\nFile content:\n{}\nTerminal output:\n{}",
            shell,
            file_content,
            terminal_output
        );

        // Verify help content did NOT leak to the terminal
        // We check for specific help markers that shouldn't appear on terminal
        let help_markers = ["Usage:", "Commands:", "Options:", "USAGE:"];
        for marker in help_markers {
            if terminal_output.contains(marker) {
                panic!(
                    "{}: Help output leaked to terminal (found '{}').\n\
                     This indicates stderr redirection is not working correctly.\n\
                     Terminal output:\n{}\n\
                     File content:\n{}",
                    shell, marker, terminal_output, file_content
                );
            }
        }
    }

    /// Test that interactive `wt --help` uses a pager.
    ///
    /// This is the complement to `test_wrapper_help_redirect_captures_all_output`:
    /// - Redirect case (`&>file`): pager should be SKIPPED (output goes to file)
    /// - Interactive case (no redirect): pager should be USED
    ///
    /// We verify pager invocation by setting GIT_PAGER to a script that creates
    /// a marker file before passing through the content.
    #[rstest]
    #[case("bash")]
    #[case("zsh")]
    #[case("fish")]
    fn test_wrapper_help_interactive_uses_pager(#[case] shell: &str, repo: TestRepo) {
        use std::io::Read;

        let wt_bin = get_cargo_bin("wt");
        let wt_bin_dir = wt_bin.parent().unwrap();

        // Create temp dir for marker file and pager script
        let tmp_dir = tempfile::tempdir().unwrap();
        let marker_file = tmp_dir.path().join("pager_invoked.marker");
        let pager_script = tmp_dir.path().join("test_pager.sh");

        // Create a pager script that:
        // 1. Creates a marker file to prove it was invoked
        // 2. Passes stdin through to stdout (like cat)
        fs::write(
            &pager_script,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", marker_file.display()),
        )
        .unwrap();

        // Make script executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&pager_script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Generate wrapper script
        let output = std::process::Command::new(&wt_bin)
            .args(["config", "shell", "init", shell])
            .output()
            .unwrap();
        let wrapper_script = String::from_utf8_lossy(&output.stdout);

        // Build shell script that sources wrapper and runs help interactively
        let script = match shell {
            "fish" => format!(
                r#"
set -x WORKTRUNK_BIN '{wt_bin}'
set -x GIT_PAGER '{pager_script}'
set -x CLICOLOR_FORCE 1

# Source the shell integration
{wrapper_script}

# Run help interactively (no redirect) - pager should be invoked
wt --help

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                pager_script = pager_script.display(),
                wrapper_script = wrapper_script,
            ),
            "zsh" => format!(
                r#"
autoload -Uz compinit && compinit -i 2>/dev/null
export WORKTRUNK_BIN='{wt_bin}'
export GIT_PAGER='{pager_script}'
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Run help interactively (no redirect) - pager should be invoked
wt --help

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                pager_script = pager_script.display(),
                wrapper_script = wrapper_script,
            ),
            _ => format!(
                r#"
export WORKTRUNK_BIN='{wt_bin}'
export GIT_PAGER='{pager_script}'
export CLICOLOR_FORCE=1

# Source the shell integration
{wrapper_script}

# Run help interactively (no redirect) - pager should be invoked
wt --help

# Marker to show script completed
echo "SCRIPT_COMPLETED"
"#,
                wt_bin = wt_bin.display(),
                pager_script = pager_script.display(),
                wrapper_script = wrapper_script,
            ),
        };

        let pair = crate::common::open_pty();

        let mut cmd = crate::common::shell_command(shell, Some(wt_bin_dir));
        cmd.arg("-c");
        cmd.arg(&script);
        cmd.cwd(repo.root_path());

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        let _status = child.wait().unwrap();
        let terminal_output = buf.replace("\r\n", "\n");

        // Verify script completed
        assert!(
            terminal_output.contains("SCRIPT_COMPLETED"),
            "{}: Script did not complete successfully.\nTerminal output:\n{}",
            shell,
            terminal_output
        );

        // Verify pager was invoked (marker file should exist)
        assert!(
            marker_file.exists(),
            "{}: Pager was NOT invoked for interactive help.\n\
             The marker file was not created, indicating show_help_in_pager() \n\
             skipped the pager even though stderr is a TTY.\n\
             Terminal output:\n{}",
            shell,
            terminal_output
        );
    }
}

// =============================================================================
// Windows PowerShell Tests
// =============================================================================
//
// All Windows-specific tests are in this module, gated by #[cfg(windows)].
// This keeps platform-specific tests clearly separated.

#[cfg(windows)]
mod windows_tests {
    use super::*;
    use crate::common::repo;
    use rstest::rstest;

    /// Test that PowerShell shell integration works for switch --create
    #[rstest]
    #[ignore = "PowerShell PTY tests timeout in CI - needs investigation"]
    fn test_powershell_switch_create(repo: TestRepo) {
        let output = exec_through_wrapper("powershell", &repo, "switch", &["--create", "feature"]);

        assert_eq!(output.exit_code, 0, "PowerShell: Command should succeed");
        output.assert_no_directive_leaks();

        assert!(
            output.combined.contains("Created branch") && output.combined.contains("and worktree"),
            "PowerShell: Should show success message.\nOutput:\n{}",
            output.combined
        );
    }

    /// Test that PowerShell shell integration handles command failures correctly
    #[rstest]
    #[ignore = "PowerShell PTY tests timeout in CI - needs investigation"]
    fn test_powershell_command_failure(mut repo: TestRepo) {
        // Create a worktree that already exists
        repo.add_worktree("existing");

        // Try to create it again - should fail
        let output = exec_through_wrapper("powershell", &repo, "switch", &["--create", "existing"]);

        assert_eq!(
            output.exit_code, 1,
            "PowerShell: Command should fail with exit code 1"
        );
        output.assert_no_directive_leaks();
        assert!(
            output.combined.contains("already exists"),
            "PowerShell: Error message should mention 'already exists'.\nOutput:\n{}",
            output.combined
        );
    }

    /// Test that PowerShell shell integration works for remove
    #[rstest]
    #[ignore = "PowerShell PTY tests timeout in CI - needs investigation"]
    fn test_powershell_remove(mut repo: TestRepo) {
        // Create a worktree to remove
        repo.add_worktree("to-remove");

        let output = exec_through_wrapper("powershell", &repo, "remove", &["to-remove"]);

        assert_eq!(output.exit_code, 0, "PowerShell: Command should succeed");
        output.assert_no_directive_leaks();
    }

    /// Test that PowerShell shell integration works for wt list
    #[rstest]
    #[ignore = "PowerShell PTY tests timeout in CI - needs investigation"]
    fn test_powershell_list(repo: TestRepo) {
        let output = exec_through_wrapper("powershell", &repo, "list", &[]);

        assert_eq!(output.exit_code, 0, "PowerShell: Command should succeed");
        output.assert_no_directive_leaks();

        // Should show the main worktree
        assert!(
            output.combined.contains("main"),
            "PowerShell: Should show main branch.\nOutput:\n{}",
            output.combined
        );
    }
}
