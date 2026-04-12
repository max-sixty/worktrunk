//! Security tests for shell injection vulnerabilities
//!
//! # Security Model: Split Directive Protocol
//!
//! Worktrunk uses two directive files with different trust levels:
//!
//! - **CD file** (`WORKTRUNK_DIRECTIVE_CD_FILE`): Contains a raw path. The shell
//!   wrapper runs `cd -- "$(< file)"`. Because the file is never sourced as
//!   shell, there is **no injection surface** — even a malicious path cannot
//!   inject commands. This file is safe to pass through to alias/hook bodies.
//!
//! - **EXEC file** (`WORKTRUNK_DIRECTIVE_EXEC_FILE`): Contains arbitrary shell
//!   (from `--execute`). The wrapper sources it. Only wt-internal Rust code
//!   writes to this file — it is scrubbed from alias/hook child environments.
//!
//! ## Defense in Depth
//!
//! Multiple layers prevent injection even if one layer fails:
//!
//! 1. **Protocol separation**: CD file is a raw path (no shell parsing); EXEC
//!    file is only written by Rust code, never by external content.
//!
//! 2. **Channel separation**: User messages go to stderr; directive files are
//!    separate. Malicious content in stderr cannot reach the directive files.
//!
//! 3. **Git layer**: Git REJECTS invalid characters in ref names (NUL, most
//!    control characters, shell metacharacters like backtick).
//!
//! 4. **Filesystem layer**: OS enforces valid path characters (NUL is
//!    universally invalid in paths).
//!
//! 5. **Legacy escaping** (compat path only): When the pre-split legacy
//!    wrapper is active, paths use POSIX single-quote escaping (`'\''`),
//!    which handles `$`, `` ` ``, `;`, `&`, `|`, spaces, etc.
//!
//! ## What These Tests Verify
//!
//! The CD file holds a raw path, so path-based shell injection is structurally
//! impossible. These tests verify the remaining attack surface:
//!
//! 1. Branch names with shell metacharacters don't corrupt the cd path
//! 2. Malicious branch names don't create unexpected files
//! 3. The EXEC file only contains user-provided `--execute` content
//! 4. Git's ref-name validation rejects the most dangerous characters
//!
//! ## Testing Limitations
//!
//! These tests run the Rust binary, not the shell wrapper. They verify that
//! directive file contents are safe, but they don't test that the wrapper
//! handles the files correctly. Full end-to-end tests with the shell wrapper
//! are in `tests/integration_tests/shell_wrapper.rs`.

use crate::common::{
    TestRepo, configure_directive_files, directive_files, repo, setup_snapshot_settings, wt_command,
};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Command;

///
/// Git provides the first line of defense by refusing to create commits
/// with NUL bytes in the message.
#[rstest]
fn test_git_rejects_nul_in_commit_messages(repo: TestRepo) {
    use std::process::Stdio;

    // Try to create a commit with NUL in the message
    // We can't use Command::arg() because Rust rejects NUL bytes,
    // so we use printf piped to git commit -F -
    let malicious_message = "Fix bug\0__WORKTRUNK_EXEC__echo PWNED";

    // Create a file to commit
    std::fs::write(repo.root_path().join("test.txt"), "content").unwrap();
    repo.run_git(&["add", "."]);

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

///
/// This verifies that the OS/Rust provides protection against NUL injection.
/// Rust's Command API uses C strings internally, which reject NUL bytes.
#[rstest]
fn test_rust_prevents_nul_bytes_in_args(repo: TestRepo) {
    // Rust's Command API should reject NUL bytes in arguments
    let malicious_branch = "feature\0__WORKTRUNK_EXEC__echo PWNED";

    // Cmd::run() should fail with InvalidInput error (NUL bytes rejected by Command API)
    let result = repo.git_command().args(["branch", malicious_branch]).run();

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

///
/// This tests the case where the entire branch name is a directive
#[rstest]
fn test_branch_name_is_directive_not_executed(repo: TestRepo) {
    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked2";

    // Try to create this branch
    let result = repo
        .git_command()
        .args(["branch", malicious_branch])
        .run()
        .unwrap();

    if !result.status.success() {
        // Git rejected the malicious branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let (cd_path, exec_path, _guard) = directive_files();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd.arg("switch")
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

#[rstest]
fn test_branch_name_with_newline_directive_not_executed(repo: TestRepo) {
    let malicious_branch = "feature\n__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked3";

    let result = repo
        .git_command()
        .args(["branch", malicious_branch])
        .run()
        .unwrap();

    if !result.status.success() {
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let (cd_path, exec_path, _guard) = directive_files();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd.arg("switch")
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

///
/// This tests if commit messages shown in output (e.g., wt list, logs) could inject directives
#[rstest]
fn test_commit_message_with_directive_not_executed(mut repo: TestRepo) {
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
        repo.configure_wt_cmd(&mut cmd);
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

///
/// This tests if file paths shown in output could inject directives
#[cfg(unix)]
#[rstest]
fn test_path_with_directive_not_executed(repo: TestRepo) {
    // Create a directory with a malicious name
    let malicious_dir = repo
        .root_path()
        .join("__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked5");
    std::fs::create_dir_all(&malicious_dir).unwrap();

    let settings = setup_snapshot_settings(&repo);

    // Run a command that might display this path
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });

    assert!(
        !std::path::Path::new("/tmp/hacked5").exists(),
        "Malicious code was executed from path display!"
    );
}

///
/// Similar to EXEC injection, but for CD directives
#[rstest]
fn test_branch_name_with_cd_directive_not_executed(repo: TestRepo) {
    // Branch name that IS a CD directive (no NUL - git allows this)
    let malicious_branch = "__WORKTRUNK_CD__/tmp";

    let result = repo
        .git_command()
        .args(["branch", malicious_branch])
        .run()
        .unwrap();

    if !result.status.success() {
        // Git rejected it - that's fine, nothing to test
        return;
    }

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let (cd_path, exec_path, _guard) = directive_files();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd.arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        // Branch name should appear in success message, but not as a separate directive
        assert_cmd_snapshot!(cmd);
    });
}

///
/// This tests if error messages (e.g., from git) could inject directives
#[rstest]
fn test_error_message_with_directive_not_executed(repo: TestRepo) {
    // Try to switch to a non-existent branch with a name that looks like a directive
    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked6";

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let (cd_path, exec_path, _guard) = directive_files();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd.arg("switch")
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

///
/// The -x flag is SUPPOSED to execute commands, so this tests that:
/// 1. Commands from -x are written to the directive file
/// 2. User content in branch names that looks like old directives doesn't cause injection
#[rstest]
fn test_execute_flag_with_directive_like_branch_name(repo: TestRepo) {
    // Branch name that looks like a directive
    let malicious_branch = "__WORKTRUNK_EXEC__echo PWNED > /tmp/hacked7";

    let result = repo
        .git_command()
        .args(["branch", malicious_branch])
        .run()
        .unwrap();

    if !result.status.success() {
        // Git rejected the branch name
        return;
    }

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let (cd_path, exec_path, _guard) = directive_files();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd.arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .arg("-x")
            .arg("echo legitimate command")
            .current_dir(repo.root_path());

        // The -x command should be written to directive file
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

// =============================================================================
// ANSI escape sequence handling in branch names
// =============================================================================

/// Test that git rejects branch names containing ANSI escape sequences.
///
/// ANSI escape sequences could theoretically corrupt terminal output if they
/// appeared in branch names displayed by `wt list`. However, git blocks this
/// at the ref validation level: control characters (bytes < 0x20 or 0x7F)
/// are rejected by git check-ref-format rule 4.
///
/// The escape character (`\x1b` = 27) is a control character, so git rejects it.
///
/// Note: Git for Windows with MSYS2 bash behaves differently and may accept
/// these branch names, so this test is Unix-only.
#[rstest]
#[cfg(unix)]
fn test_git_rejects_ansi_escape_in_branch_names(repo: TestRepo) {
    let shell_cmd = r#"git branch $'feature-\x1b[31mRED\x1b[0m-test'"#;

    let output = Command::new("bash")
        .args(["-c", shell_cmd])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "Expected git to reject ANSI escape sequences in branch name"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a valid branch name") || stderr.contains("invalid"),
        "Expected git to complain about invalid branch name, got: {}",
        stderr
    );
}

/// Test that manually created refs with ANSI escapes are ignored by git.
///
/// Even if an attacker bypasses git's normal validation and creates a ref file
/// directly in .git/refs/heads/ with ANSI codes in the filename, git ignores it.
#[rstest]
#[cfg(unix)]
fn test_git_ignores_malformed_refs_with_ansi(repo: TestRepo) {
    let shell_cmd = r#"
        commit_sha=$(git rev-parse HEAD)
        printf "$commit_sha" > '.git/refs/heads/feature-'$'\x1b''[31mRED'$'\x1b''[0m-test'
        "#;

    let create_result = Command::new("bash")
        .args(["-c", shell_cmd])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    assert!(
        create_result.status.success(),
        "Failed to create malformed ref file: {}",
        String::from_utf8_lossy(&create_result.stderr)
    );

    // Git should ignore the malformed ref
    let branch_output = repo.git_output(&["branch", "-a"]);
    assert!(
        !branch_output.contains("RED"),
        "Malformed ref with ANSI escape should not appear in branch list"
    );

    // wt list should also not show it
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        assert_cmd_snapshot!(cmd);
    });
}

/// Test that literal escape-like text in branch names displays safely.
///
/// Branch names like "fix-backslash-x1b-test" contain literal characters
/// (not actual escape codes). Git allows this and they should display literally.
#[rstest]
fn test_literal_escape_like_branch_names_displayed_safely(repo: TestRepo) {
    let branch_name = "fix-backslash-x1b-test";

    let result = repo.git_command().args(["branch", branch_name]).run();

    if let Ok(output) = result
        && output.status.success()
    {
        let mut settings = setup_snapshot_settings(&repo);
        settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]");

        settings.bind(|| {
            let mut cmd = wt_command();
            repo.configure_wt_cmd(&mut cmd);
            cmd.args(["list", "--branches"])
                .current_dir(repo.root_path());
            assert_cmd_snapshot!(cmd);
        });
    }
}
