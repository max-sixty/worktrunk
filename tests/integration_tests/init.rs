use crate::common::{TestRepo, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Helper to create snapshot for config shell init command
fn snapshot_init(test_name: &str, shell: &str, extra_args: &[&str]) {
    let repo = TestRepo::new();

    // Custom settings for init tests - these output shell scripts with intentional
    // backslashes (\cd, \n) so we can't use setup_snapshot_settings which has a
    // backslash normalization filter that would corrupt the output
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Redact volatile env vars in metadata
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");

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
fn test_init(#[case] shell: &str) {
    snapshot_init(&format!("init_{}", shell), shell, &[]);
}

#[test]
fn test_init_invalid_shell() {
    let repo = TestRepo::new();

    // Same custom settings as snapshot_init
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");

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
        [1m[31merror:[0m invalid value '[1m[33minvalid-shell[0m' for '[1m[36m<SHELL>[0m'
          [possible values: [1m[32mbash[0m, [1m[32mfish[0m, [1m[32mzsh[0m]

        For more information, try '[1m[36m--help[0m'.
        ");
    });
}

#[cfg(unix)]
#[test]
fn test_fish_no_duplicate_base_completion() {
    // Verify that the fish completion doesn't have duplicate entries for --base
    let repo = TestRepo::new();
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.arg("config")
        .arg("shell")
        .arg("init")
        .arg("fish")
        .current_dir(repo.root_path());

    let output = cmd
        .output()
        .expect("Failed to run wt config shell init fish");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Count how many lines contain "complete -c wt" and "-l base"
    let base_completions: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains("complete -c wt") && line.contains("-l base"))
        .collect();

    // Should only have one completion for --base (from clap's static generation)
    assert_eq!(
        base_completions.len(),
        1,
        "Expected exactly 1 completion for --base, but found {}:\n{}",
        base_completions.len(),
        base_completions.join("\n")
    );
}
