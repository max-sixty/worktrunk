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
