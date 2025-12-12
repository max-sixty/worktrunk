//! Snapshot tests for `-h` (short) and `--help` (long) output.
//!
//! These ensure our help formatting stays stable across releases and
//! catches accidental regressions in wording or wrapping.
//!
//! - Short help (`-h`): Compact format, single-line options
//! - Long help (`--help`): Verbose format with `after_long_help` content
//!
//! Skipped on Windows: clap renders markdown differently on Windows (tables, links,
//! emphasis) resulting in formatting-only differences. The help content is identical;
//! only the presentation varies.
#![cfg(not(windows))]

use crate::common::wt_command;
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

fn snapshot_help(test_name: &str, args: &[&str]) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Remove trailing ANSI reset codes at end of lines for cross-platform consistency
    settings.add_filter(r"\x1b\[0m$", "");
    settings.add_filter(r"\x1b\[0m\n", "\n");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// Root command (wt)
#[rstest]
#[case("help_root_short", "-h")]
#[case("help_root_long", "--help")]
#[case("help_no_args", "")]
// Major commands - short and long variants
#[case("help_config_short", "config -h")]
#[case("help_config_long", "config --help")]
#[case("help_list_short", "list -h")]
#[case("help_list_long", "list --help")]
#[case("help_switch_short", "switch -h")]
#[case("help_switch_long", "switch --help")]
#[case("help_remove_short", "remove -h")]
#[case("help_remove_long", "remove --help")]
#[case("help_merge_short", "merge -h")]
#[case("help_merge_long", "merge --help")]
#[case("help_step_short", "step -h")]
#[case("help_step_long", "step --help")]
// Config subcommands (long help only - these are less frequently accessed)
#[case("help_config_shell", "config shell --help")]
#[case("help_config_create", "config create --help")]
#[case("help_config_show", "config show --help")]
#[case("help_config_state", "config state --help")]
#[case(
    "help_config_state_default_branch",
    "config state default-branch --help"
)]
#[case(
    "help_config_state_previous_branch",
    "config state previous-branch --help"
)]
#[case("help_config_state_ci_status", "config state ci-status --help")]
#[case("help_config_state_marker", "config state marker --help")]
#[case("help_config_state_logs", "config state logs --help")]
#[case("help_config_state_get", "config state get --help")]
#[case("help_config_state_clear", "config state clear --help")]
#[case("help_hook_approvals", "hook approvals --help")]
#[case("help_hook_approvals_add", "hook approvals add --help")]
#[case("help_hook_approvals_clear", "hook approvals clear --help")]
fn test_help(#[case] test_name: &str, #[case] args_str: &str) {
    let args: Vec<&str> = if args_str.is_empty() {
        vec![]
    } else {
        args_str.split_whitespace().collect()
    };
    snapshot_help(test_name, &args);
}
