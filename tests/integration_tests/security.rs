//! Security tests for shell injection vulnerabilities
//!
//! # Security Model
//!
//! The CD directive (`WORKTRUNK_DIRECTIVE_CD_FILE`) contains a raw path. The shell
//! wrapper changes to that path without evaluating it. `--execute` starts one
//! external program directly; no shell parses its argv.
//!
//! 1. **Structured inputs**: CD is a raw path and execute is argv; neither is
//!    parsed as shell.
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
//! ## What These Tests Verify
//!
//! The CD file holds a raw path, so path-based shell injection is structurally
//! impossible. These tests verify the remaining attack surface:
//!
//! 1. Branch names with shell metacharacters don't corrupt the cd path
//! 2. Malicious branch names don't create unexpected files
//! 3. Git's ref-name validation rejects the most dangerous characters
//!
//! ## Testing Limitations
//!
//! These tests run the Rust binary, not the shell wrapper. They verify that
//! the direct execution input is safe, but they don't test the wrapper. Full
//! end-to-end tests with the shell wrapper
//! are in `tests/integration_tests/shell_wrapper.rs`.
//!
//! ## Writing a payload git will accept
//!
//! A branch-name payload has to survive git's ref-name rules (git-check-ref-format(1)
//! rule 4), which reject a space and every ASCII control character — so
//! `echo PWNED > /tmp/…` and anything carrying a newline can never become a
//! branch, and a test built on one exercises nothing. Word-split the payload
//! with `$IFS` instead: `touch$IFS/tmp/hacked2` is a legal ref name and still
//! creates the canary if a shell ever evaluates it. A payload that reaches a
//! worktree path must also be a legal filename on Windows, which rules out
//! `< > : " | ? *` — `sanitize_branch_name` only rewrites `/` and `\`.
//!
//! Where git's own rules are the defense, assert the refusal rather than
//! returning early from it: a skip reads as a pass on every platform.

use crate::common::{
    TestRepo, configure_directive_file, directive_file, repo, setup_snapshot_settings, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Command;
use worktrunk::config::sanitize_branch_name;

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
        malicious_message.replace('\0', r"\0")
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

/// A branch name that is entirely an EXEC directive must not be executed.
///
/// The payload carries no space: git's ref-name rule 4 rejects a space outright
/// (as it does control characters), so a name built around `echo PWNED >
/// /tmp/…` can never be created and everything below it would be skipped.
/// `touch$IFS/tmp/hacked2` is the space-free equivalent — a valid ref name, a
/// legal path component on every platform `wt` builds a worktree on, and a
/// shell command that still creates the canary through `$IFS` word splitting
/// if anything ever evaluates it.
///
/// `wt switch --create` creates the branch itself, so this covers the success
/// path: the snapshot shows the directive-shaped name reaching the worktree
/// path and the success message verbatim, and the canary assertion shows
/// nothing evaluated it on the way.
#[rstest]
fn test_branch_name_is_directive_not_executed(repo: TestRepo) {
    let malicious_branch = "__WORKTRUNK_EXEC__touch$IFS/tmp/hacked2";
    let expected_worktree = expected_worktree_path(&repo, malicious_branch);

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let (cd_path, _guard) = directive_file();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        assert_cd_file_holds_worktree_path(&cd_path, &expected_worktree);
    });

    // Verify the malicious file was NOT created
    assert!(
        !std::path::Path::new("/tmp/hacked2").exists(),
        "Malicious code was executed! File /tmp/hacked2 should not exist"
    );
}

/// The worktree path `wt switch --create <branch>` produces under the default
/// layout: a sibling of the repo named `<repo>.<branch>`, with `/` and `\`
/// dashed out by `sanitize_branch_name` and nothing else rewritten.
fn expected_worktree_path(repo: &TestRepo, branch: &str) -> std::path::PathBuf {
    let root = repo.root_path();
    let repo_name = root.file_name().expect("repo root has a name");
    root.parent().expect("repo root has a parent").join(format!(
        "{}.{}",
        repo_name.to_string_lossy(),
        sanitize_branch_name(branch)
    ))
}

/// Assert the CD directive file holds what `wt` promises a wrapper it holds:
/// one line, and that line is the worktree `wt` just created.
///
/// This is the assertion that carries the directive tests. `wt` writes the CD
/// file itself (`src/output/global.rs`), so a branch name that smuggled a
/// second line or a different destination past the display layer would show up
/// here — whereas the `/tmp/hackedN` canaries in the callers can only ever
/// pass: these tests run the binary directly, and no shell evaluates the
/// file's contents.
///
/// It compares against `expected` rather than asking whether the line names
/// *a* directory, because the directive these tests model
/// (`__WORKTRUNK_CD__/tmp`) smuggles a path that exists — a verifier that only
/// checked `is_dir()` would pass on the leak it is named for. Both sides are
/// canonicalized: `wt` writes the logical, symlink-preserved path, which is
/// not textually the tempdir path on macOS.
fn assert_cd_file_holds_worktree_path(cd_path: &std::path::Path, expected: &std::path::Path) {
    let cd_content = std::fs::read_to_string(cd_path).unwrap_or_default();
    assert_eq!(
        cd_content.lines().count(),
        1,
        "the CD file must hold a single line, got {cd_content:?}"
    );
    let written = std::fs::canonicalize(cd_content.trim()).unwrap_or_else(|e| {
        panic!("the CD file must hold an existing path, got {cd_content:?}: {e}")
    });
    let expected = std::fs::canonicalize(expected)
        .unwrap_or_else(|e| panic!("the new worktree {} should exist: {e}", expected.display()));
    assert_eq!(
        written, expected,
        "the CD file must hold the new worktree's path, got {cd_content:?}"
    );
}

/// Assert `wt` left the CD directive file untouched.
///
/// The counterpart to [`assert_cd_file_holds_worktree_path`] for the tests
/// whose `wt switch` fails: no switch happened, so nothing may move the user's
/// shell. `directive_file()` hands over an empty file, and `wt` only writes one
/// after a successful switch (`handle_switch_output`), so any content here is a
/// destination that came from somewhere other than a completed switch.
fn assert_cd_file_unwritten(cd_path: &std::path::Path) {
    let cd_content = std::fs::read_to_string(cd_path).unwrap_or_default();
    assert!(
        cd_content.is_empty(),
        "a failed switch must not write a cd directive, got {cd_content:?}"
    );
}

/// A directive smuggled onto a second line never reaches `wt` at all.
///
/// Git's ref-name validation is the layer that stops this one (defense layer 3
/// above): a newline is a control character, so `git branch` refuses the name
/// on every platform and no such branch can exist for `wt` to read. Assert that
/// refusal rather than skipping past it — the earlier form of this test tried
/// to create the branch and returned early when git said no, so its body never
/// ran and it passed without exercising anything.
#[rstest]
fn test_git_rejects_newline_directive_in_branch_name(repo: TestRepo) {
    let malicious_branch = "feature\n__WORKTRUNK_EXEC__touch$IFS/tmp/hacked3";

    let result = repo
        .git_command()
        .args(["branch", malicious_branch])
        .run()
        .unwrap();

    assert!(
        !result.status.success(),
        "git should reject a branch name containing a newline"
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

    assert!(
        result.status.success(),
        "git should accept {malicious_branch} as a branch name; \
         without the branch there is nothing for this test to exercise"
    );

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let (cd_path, _guard) = directive_file();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        // The branch is pre-created above, so `--create` fails: this snapshots
        // how a directive-shaped name renders in an error message.
        assert_cmd_snapshot!(cmd);

        // The payload names `/tmp`, which exists: the proof that nothing
        // honored it is that the failed switch wrote no destination at all.
        assert_cd_file_unwritten(&cd_path);
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
        let (cd_path, _guard) = directive_file();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("switch")
            .arg(malicious_branch)
            .current_dir(repo.root_path());

        // Should fail with error, but not execute directive
        assert_cmd_snapshot!(cmd);

        assert_cd_file_unwritten(&cd_path);
    });

    assert!(
        !std::path::Path::new("/tmp/hacked6").exists(),
        "Malicious code was executed from error message!"
    );
}

///
/// User content in branch names that looks like old directives must not become
/// part of the explicitly requested command.
#[rstest]
fn test_execute_flag_with_directive_like_branch_name(repo: TestRepo) {
    // Branch name that looks like a directive. Space-free for the same reason
    // as `test_branch_name_is_directive_not_executed`: git rejects a ref-name
    // containing a space, so the old payload could never be created and the
    // branch it needs never existed.
    let malicious_branch = "__WORKTRUNK_EXEC__touch$IFS/tmp/hacked7";
    let expected_worktree = expected_worktree_path(&repo, malicious_branch);

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let (cd_path, _guard) = directive_file();
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("switch")
            .arg("--create")
            .arg(malicious_branch)
            .arg("-x")
            .arg("echo")
            .args(["--", "legitimate command"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        assert_cd_file_holds_worktree_path(&cd_path, &expected_worktree);
    });

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

    // The `git` this bash child spawns inherits its environment, so the
    // isolation goes on the shell (as it does for the `sh` child above).
    // `LC_ALL`/`LANG=C` from `git_test_env` is what the stderr assertion below
    // rests on — git ships translated messages, so on a contributor's machine
    // with a non-English locale the check for "not a valid branch name" reads a
    // German or French rendering of it and fails.
    let mut cmd = Command::new("bash");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
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

/// Test that literal escape-like text in branch names displays safely.
///
/// Branch names like "fix-backslash-x1b-test" contain literal characters
/// (not actual escape codes). Git allows this and they should display literally.
#[rstest]
fn test_literal_escape_like_branch_names_displayed_safely(repo: TestRepo) {
    let branch_name = "fix-backslash-x1b-test";

    let output = repo
        .git_command()
        .args(["branch", branch_name])
        .run()
        .expect("git branch should run");
    assert!(
        output.status.success(),
        "git should accept {branch_name} as a branch name"
    );

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
