//! Security tests for shell script injection vulnerabilities
//!
//! # Attack Surface Analysis
//!
//! Worktrunk uses a shell script protocol for shell integration. Commands with `--internal`:
//! - Stream all user-visible output (progress, errors, hints) to stderr in real-time
//! - Emit a shell script to stdout at the end (cd commands, exec commands)
//!
//! The shell wrapper captures stdout via command substitution and evals it:
//! ```bash
//! script="$(wt --internal ... 2>&2)" && eval "$script"
//! ```
//!
//! ## Vulnerability: Shell Injection
//!
//! If external content (branch names, file paths, git output) can inject malicious shell
//! code into stdout, the shell wrapper will execute it. This is analogous to SQL injection
//! or command injection vulnerabilities.
//!
//! ## Attack Vectors
//!
//! ### 1. Path Injection via Single Quote Escaping (HIGH RISK if not escaped)
//!
//! Paths are emitted in single quotes: `cd '/path/to/worktree'`
//! Single quotes prevent most shell metacharacter expansion, but embedded single quotes
//! could break out of the quoting if not properly escaped.
//!
//! **Example attack (if unescaped):**
//! ```bash
//! # Create directory with malicious name
//! mkdir "test'; rm -rf /; echo '"
//! wt switch --internal branch  # If cd emits: cd 'test'; rm -rf /; echo ''
//! ```
//!
//! **Protection:** All paths are escaped using `replace('\'', "'\\''")` pattern,
//! which is the standard POSIX approach for embedding single quotes in single-quoted strings.
//!
//! ### 2. Execute Command Injection (LOW RISK - user-controlled)
//!
//! The `--execute` flag lets users specify shell commands to run. This is intentionally
//! user-controlled and not an injection vector (users can already run arbitrary commands).
//!
//! ### 3. Branch Name in Output (NO RISK)
//!
//! Branch names appear in stderr messages, not in the stdout shell script.
//! The shell wrapper only evals stdout, so stderr content cannot be executed.
//!
//! ## Current Protections
//!
//! **Shell script protocol with proper escaping:**
//!
//! 1. **Channel separation**: User messages go to stderr, shell script goes to stdout
//!    - Shell wrapper only evals stdout: `script="$(... 2>&2)" && eval "$script"`
//!    - Malicious content in stderr cannot be executed
//!
//! 2. **Path escaping**: All paths use single quotes with `'\''` escape pattern
//!    ```rust
//!    let escaped = path_str.replace('\'', "'\\''");
//!    writeln!(stdout, "cd '{}'", escaped)?;
//!    ```
//!    This handles all shell metacharacters: `$`, `` ` ``, `;`, `&`, `|`, spaces, etc.
//!
//! 3. **Git layer**: Git REJECTS invalid characters in ref names
//!
//! 4. **Filesystem layer**: OS enforces valid path characters
//!
//! ## Vulnerabilities We Test
//!
//! This test suite verifies that user-controlled content CANNOT inject shell commands:
//!
//! 1. ✅ Branch names with shell metacharacters
//! 2. ✅ Branch names with single quotes
//! 3. ✅ Paths with special characters
//! 4. ✅ Git output with shell commands
//!
//! ## Security Model
//!
//! The new shell script protocol is simpler and more secure than the previous NUL-delimited
//! directive protocol:
//!
//! 1. **Simpler parsing**: No NUL byte parsing in shell, just command substitution + eval
//! 2. **Channel separation**: Messages on stderr, script on stdout (not interleaved)
//! 3. **Standard escaping**: Uses well-understood POSIX single-quote escaping
//! 4. **Smaller attack surface**: Only cd and exec commands in stdout, nothing else
//!
//! ### Testing Limitations
//!
//! These tests verify that:
//! - Path escaping is correct for shell metacharacters
//! - Branch names with special characters don't break quoting
//!
//! However, they DON'T fully test shell execution security because:
//! - Tests run the Rust binary, not the shell wrapper
//! - Full end-to-end tests with malicious shell wrapper input are in `shell_wrapper.rs`
//!
//! For comprehensive security testing, see `tests/integration_tests/shell_wrapper.rs` which
//! tests the full shell integration pipeline.

use crate::common::{TestRepo, setup_snapshot_settings, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Test that Git rejects NUL bytes in commit messages
///
/// Git provides the first line of defense by refusing to create commits
/// with NUL bytes in the message.
#[test]
fn test_git_rejects_nul_in_commit_messages() {
    use std::process::Stdio;

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Try to create a commit with NUL in the message
    // We can't use Command::arg() because Rust rejects NUL bytes,
    // so we use printf piped to git commit -F -
    let malicious_message = "Fix bug\0__WORKTRUNK_EXEC__echo PWNED";

    // Create a file to commit
    std::fs::write(repo.root_path().join("test.txt"), "content").unwrap();
    let mut add_cmd = Command::new("git");
    repo.configure_git_cmd(&mut add_cmd);
    add_cmd
        .args(["add", "."])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Try to commit with NUL in message using shell redirection
    let shell_cmd = format!(
        "printf '{}' | git commit -F -",
        malicious_message.replace('\0', "\\0")
    );

    let mut cmd = Command::new("sh");
    repo.configure_git_cmd(&mut cmd);
    cmd.arg("-c")
        .arg(&shell_cmd)
        .current_dir(repo.root_path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let output = cmd.output().unwrap();

    // Git should reject this
    assert!(
        !output.status.success(),
        "Expected git to reject NUL bytes in commit message"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NUL byte") || stderr.contains("nul byte"),
        "Expected git to complain about NUL bytes, got: {}",
        stderr
    );
}

/// Test that Rust/OS prevents NUL bytes in command arguments
///
/// This verifies that the OS/Rust provides protection against NUL injection.
/// Rust's Command API uses C strings internally, which reject NUL bytes.
#[test]
fn test_rust_prevents_nul_bytes_in_args() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Rust's Command API should reject NUL bytes in arguments
    let malicious_branch = "feature\0__WORKTRUNK_EXEC__echo PWNED";

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", malicious_branch])
        .current_dir(repo.root_path());

    // Command::output() should fail with InvalidInput error
    let result = cmd.output();

    match result {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            // Good! Rust prevented the NUL byte injection
        }
        Ok(output) => {
            panic!(
                "Expected Rust to reject NUL bytes in args, but command succeeded: {:?}",
                output
            );
        }
        Err(e) => {
            panic!(
                "Expected InvalidInput error for NUL bytes, got different error: {:?}",
                e
            );
        }
    }
}

/// Test that branch names that ARE directives themselves don't get executed
///
/// This tests the case where the entire branch name is a directive
#[test]
fn test_branch_name_is_directive_not_executed() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked2";

    // Try to create this branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", malicious_branch])
        .current_dir(repo.root_path());

    let result = cmd.output().unwrap();

    if !result.status.success() {
        // Git rejected the malicious branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    // Verify the malicious file was NOT created
    assert!(
        !std::path::Path::new("/tmp/hacked2").exists(),
        "Malicious code was executed! File /tmp/hacked2 should not exist"
    );
}

/// Test that branch names with newline + directive are not executed
#[test]
fn test_branch_name_with_newline_directive_not_executed() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let malicious_branch = "feature\n__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked3";

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", malicious_branch])
        .current_dir(repo.root_path());

    let result = cmd.output().unwrap();

    if !result.status.success() {
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    assert!(
        !std::path::Path::new("/tmp/hacked3").exists(),
        "Malicious code was executed!"
    );
}

/// Test that commit messages with directives in list output don't get executed
///
/// This tests if commit messages shown in output (e.g., wt list, logs) could inject directives
#[test]
fn test_commit_message_with_directive_not_executed() {
    use crate::common::setup_snapshot_settings;

    let mut repo = TestRepo::new();

    // Create commit with malicious message (no NUL - Rust prevents those)
    let malicious_message = "Fix bug\n__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked4";
    repo.commit_with_message(malicious_message);

    // Create a worktree
    let _feature_wt = repo.add_worktree("feature");

    let mut settings = setup_snapshot_settings(&repo);
    // Filter SHAs because commit_with_message creates non-deterministic hashes
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]");

    // Run 'wt list' which might show commit messages
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        // Verify output - commit message should be escaped/sanitized
        assert_cmd_snapshot!(cmd);
    });

    // Verify the malicious file was NOT created
    assert!(
        !std::path::Path::new("/tmp/hacked4").exists(),
        "Malicious code was executed from commit message!"
    );
}

/// Test that path display with directives doesn't get executed
///
/// This tests if file paths shown in output could inject directives
#[cfg(unix)]
#[test]
fn test_path_with_directive_not_executed() {
    use crate::common::setup_snapshot_settings;

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a directory with a malicious name
    let malicious_dir = repo
        .root_path()
        .join("__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked5");
    std::fs::create_dir_all(&malicious_dir).unwrap();

    let settings = setup_snapshot_settings(&repo);

    // Run a command that might display this path
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    assert!(
        !std::path::Path::new("/tmp/hacked5").exists(),
        "Malicious code was executed from path display!"
    );
}

/// Test that CD directive in branch names is not treated as a directive
///
/// Similar to EXEC injection, but for CD directives
#[test]
fn test_branch_name_with_cd_directive_not_executed() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Branch name that IS a CD directive (no NUL - git allows this)
    let malicious_branch = "__WORKTRUNK_CD__/tmp";

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", malicious_branch])
        .current_dir(repo.root_path());

    let result = cmd.output().unwrap();

    if !result.status.success() {
        // Git rejected it - that's fine, nothing to test
        return;
    }

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        // Branch name should appear in success message, but not as a separate directive
        assert_cmd_snapshot!(cmd);
    });
}

/// Test that error messages cannot inject directives
///
/// This tests if error messages (e.g., from git) could inject directives
#[test]
fn test_error_message_with_directive_not_executed() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Try to switch to a non-existent branch with a name that looks like a directive
    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked6";

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        // Should fail with error, but not execute directive
        assert_cmd_snapshot!(cmd);
    });

    assert!(
        !std::path::Path::new("/tmp/hacked6").exists(),
        "Malicious code was executed from error message!"
    );
}

/// Test that execute flag (-x) input is properly handled
///
/// The -x flag is SUPPOSED to execute commands, so this tests that:
/// 1. Commands from -x are emitted as shell script to stdout
/// 2. User content in branch names that looks like old directives doesn't cause injection
#[test]
fn test_execute_flag_with_directive_like_branch_name() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Branch name that looks like a directive
    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked7";

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", malicious_branch])
        .current_dir(repo.root_path());

    let result = cmd.output().unwrap();

    if !result.status.success() {
        // Git rejected the branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .arg("-x")
            .arg("echo legitimate command")
            .current_dir(repo.root_path());

        // The -x command should appear in stdout as shell script
        // The branch name should NOT inject additional commands
        assert_cmd_snapshot!(cmd);
    });

    // The legitimate command would execute (we're not actually running the shell wrapper),
    // but the injected command should NOT
    assert!(
        !std::path::Path::new("/tmp/hacked7").exists(),
        "Malicious code was executed alongside legitimate -x command!"
    );
}
