//! Snapshot tests for `wt config shell init` command output.
//!
//! Skipped on Windows: These tests verify shell init scripts for bash/zsh/fish.
//! Windows line endings (CRLF) cause snapshot mismatches, and these Unix shells
//! are not the primary shell integration path on Windows (PowerShell is).
#![cfg(not(windows))]

use crate::common::{TestRepo, add_standard_env_redactions, repo, wt_bin, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Command;

/// Helper to create snapshot for config shell init command
fn snapshot_init(test_name: &str, repo: &TestRepo, shell: &str, extra_args: &[&str]) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    add_standard_env_redactions(&mut settings);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("config").arg("shell").arg("init").arg(shell);

        for arg in extra_args {
            cmd.arg(arg);
        }

        cmd.current_dir(repo.root_path());

        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
// Test supported shells
#[case("bash")]
#[case("fish")]
#[case("zsh")]
fn test_init(#[case] shell: &str, repo: TestRepo) {
    snapshot_init(&format!("init_{}", shell), &repo, shell, &[]);
}

#[rstest]
fn test_init_invalid_shell(repo: TestRepo) {
    // Same custom settings as snapshot_init
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    add_standard_env_redactions(&mut settings);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("config")
            .arg("shell")
            .arg("init")
            .arg("invalid-shell")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @"
        success: false
        exit_code: 2
        ----- stdout -----

        ----- stderr -----
        [1m[31merror:[0m invalid value '[1m[33minvalid-shell[0m' for '[1m[36m<bash|fish|nu|zsh|powershell>[0m'
          [possible values: [1m[32mbash[0m, [1m[32mfish[0m, [1m[32mnu[0m, [1m[32mzsh[0m, [1m[32mpowershell[0m]

        For more information, try '[1m[36m--help[0m'.
        ");
    });
}

#[rstest]
#[case("bash")]
#[case("fish")]
#[case("nu")]
#[case("powershell")]
#[case("zsh")]
fn test_init_rejects_unsafe_cmd(#[case] shell: &str, repo: TestRepo) {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config")
        .arg("shell")
        .arg("init")
        .arg(shell)
        .arg("--cmd")
        .arg("wt; touch /tmp/pwn")
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unsafe command name must not emit shell code:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Invalid shell integration command name"),
        "expected validation error, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A degenerate argv\[0\] — empty here, so it has no file name — never selects
/// mock playback: `path::executable_name` yields `None` and the process runs as
/// wt, rather than panicking inside `main()` or probing the config dir.
#[test]
fn test_mock_dispatch_ignores_degenerate_argv0() {
    use std::os::unix::process::CommandExt;

    let config_dir = tempfile::tempdir().unwrap();
    let output = Command::new(wt_bin())
        .arg0("")
        .arg("--version")
        .env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", config_dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "wt --version should succeed");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("wt "),
        "expected wt's own version output, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A non-UTF8 argv\[0\] must not panic the dispatch (`env::args()` panics on
/// non-Unicode arguments; the dispatch reads `args_os`): it lossily converts,
/// matches no config, and wt runs as itself.
#[test]
fn test_mock_dispatch_survives_non_utf8_argv0() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;

    let config_dir = tempfile::tempdir().unwrap();
    let output = Command::new(wt_bin())
        .arg0(OsStr::from_bytes(b"\xff\xfe"))
        .arg("--version")
        .env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", config_dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "wt --version should succeed");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("wt "),
        "expected wt's own version output, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A dot in argv\[0\] is part of the command name, not an extension to drop:
/// `validate_shell_command_name` accepts `.`, so a `wt.old` invocation must
/// wrap `wt.old` — wrapping the truncated `wt` would name a command the user
/// may not have installed at all.
#[rstest]
fn test_init_keeps_dotted_argv0_command_name(repo: TestRepo) {
    let temp_dir = tempfile::tempdir().unwrap();
    let dotted_bin = temp_dir.path().join("wt.old");
    std::os::unix::fs::symlink(wt_bin(), &dotted_bin).unwrap();

    let mut cmd = Command::new(&dotted_bin);
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config")
        .arg("shell")
        .arg("init")
        .arg("bash")
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "shell init failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("wt.old() {"),
        "expected a wrapper for the full invocation name:\n{stdout}"
    );
    assert!(
        !stdout.contains("wt() {"),
        "wrapper must not fall back to the truncated name:\n{stdout}"
    );
}

/// A non-UTF8 argv\[0\] names a command that cannot be spelled in shell syntax,
/// so it reaches the same rejection as the `wt;touch` case below. Answering
/// with wt's own name instead would generate integration for a command other
/// than the one that ran.
#[test]
fn test_init_rejects_non_utf8_argv0_command_name() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;

    let mut cmd = wt_command();
    cmd.arg0(OsStr::from_bytes(b"\xff\xfe"))
        .arg("config")
        .arg("shell")
        .arg("init")
        .arg("bash");

    let output = cmd.output().unwrap();

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unspellable command name must not emit shell code:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Invalid shell integration command name"),
        "expected validation error, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[rstest]
fn test_init_rejects_unsafe_argv0_command_name(repo: TestRepo) {
    let temp_dir = tempfile::tempdir().unwrap();
    let bad_bin = temp_dir.path().join("wt;touch");
    std::os::unix::fs::symlink(wt_bin(), &bad_bin).unwrap();

    let mut cmd = Command::new(&bad_bin);
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("config")
        .arg("shell")
        .arg("init")
        .arg("bash")
        .current_dir(repo.root_path());

    let output = cmd.output().unwrap();

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unsafe argv[0] command name must not emit shell code:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Invalid shell integration command name"),
        "expected validation error, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
