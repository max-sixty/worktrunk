//! Snapshot tests for `wt config shell init` command output.
//!
//! Skipped on Windows: These tests verify shell init scripts for bash/zsh/fish.
//! Windows line endings (CRLF) cause snapshot mismatches, and these Unix shells
//! are not the primary shell integration path on Windows (PowerShell is).
#![cfg(not(windows))]

use crate::common::{TestRepo, add_standard_env_redactions, repo, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Helper to create snapshot for config shell init command
fn snapshot_init(test_name: &str, repo: &TestRepo, shell: &str, extra_args: &[&str]) {
    // Custom settings for init tests - these output shell scripts with intentional
    // backslashes (\cd, \n) so we can't use setup_snapshot_settings which has a
    // backslash normalization filter that would corrupt the output
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    add_standard_env_redactions(&mut settings);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
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
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config")
            .arg("shell")
            .arg("init")
            .arg("invalid-shell")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 2
        ----- stdout -----

        ----- stderr -----
        [1m[31merror:[0m invalid value '[1m[33minvalid-shell[0m' for '[1m[36m<bash|fish|zsh|powershell>[0m'
          [possible values: [1m[32mbash[0m, [1m[32mfish[0m, [1m[32mzsh[0m, [1m[32mpowershell[0m]

        For more information, try '[1m[36m--help[0m'.
        ");
    });
}
