use crate::common::TestRepo;
use insta::Settings;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use rstest::rstest;
use std::process::Command;

/// Helper to create snapshot for init command
fn snapshot_init(test_name: &str, shell: &str, extra_args: &[&str]) {
    let repo = TestRepo::new();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("init").arg(shell);

        for arg in extra_args {
            cmd.arg(arg);
        }

        cmd.current_dir(repo.root_path());

        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
// Tier 1: Shells available in standard Ubuntu repos
#[case("bash")]
#[case("fish")]
#[case("zsh")]
// Tier 2: Shells requiring extra setup
// TODO: Fix non-core shells - all have snapshot mismatches in init output
// #[cfg_attr(feature = "tier-2-integration-tests", case("elvish"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("nushell"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("oil"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("powershell"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("xonsh"))]
fn test_init(#[case] shell: &str) {
    snapshot_init(&format!("init_{}", shell), shell, &[]);
}

#[test]
fn test_init_invalid_shell() {
    let repo = TestRepo::new();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("init")
            .arg("invalid-shell")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 2
        ----- stdout -----

        ----- stderr -----
        [1m[31merror:[0m invalid value '[33minvalid-shell[0m' for '[1m<SHELL>[0m'
          [possible values: [32mbash[0m, [32melvish[0m, [32mfish[0m, [32mnushell[0m, [32moil[0m, [32mpowershell[0m, [32mxonsh[0m, [32mzsh[0m]

        For more information, try '[1m--help[0m'.
        ");
    });
}

#[test]
fn test_fish_no_duplicate_base_completion() {
    // Verify that the fish completion doesn't have duplicate entries for --base
    let repo = TestRepo::new();
    let mut cmd = Command::new(get_cargo_bin("wt"));
    repo.clean_cli_env(&mut cmd);
    cmd.arg("init").arg("fish").current_dir(repo.root_path());

    let output = cmd.output().expect("Failed to run wt init fish");
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
