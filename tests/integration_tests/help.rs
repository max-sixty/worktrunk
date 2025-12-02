//! Snapshot tests for `-h` (short) and `--help` (long) output.
//!
//! These ensure our help formatting stays stable across releases and
//! catches accidental regressions in wording or wrapping.
//!
//! - Short help (`-h`): Compact format, single-line options
//! - Long help (`--help`): Verbose format with `after_long_help` content

use crate::common::wt_command;
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;

fn snapshot_help(test_name: &str, args: &[&str]) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// =============================================================================
// Root command (wt)
// =============================================================================

#[test]
fn help_root_short() {
    snapshot_help("help_root_short", &["-h"]);
}

#[test]
fn help_root_long() {
    snapshot_help("help_root_long", &["--help"]);
}

#[test]
fn help_no_args() {
    // Running `wt` with no args should show short help and exit 0
    snapshot_help("help_no_args", &[]);
}

// =============================================================================
// Major commands - short and long variants
// =============================================================================

#[test]
fn help_config_short() {
    snapshot_help("help_config_short", &["config", "-h"]);
}

#[test]
fn help_config_long() {
    snapshot_help("help_config_long", &["config", "--help"]);
}

#[test]
fn help_list_short() {
    snapshot_help("help_list_short", &["list", "-h"]);
}

#[test]
fn help_list_long() {
    snapshot_help("help_list_long", &["list", "--help"]);
}

#[test]
fn help_switch_short() {
    snapshot_help("help_switch_short", &["switch", "-h"]);
}

#[test]
fn help_switch_long() {
    snapshot_help("help_switch_long", &["switch", "--help"]);
}

#[test]
fn help_remove_short() {
    snapshot_help("help_remove_short", &["remove", "-h"]);
}

#[test]
fn help_remove_long() {
    snapshot_help("help_remove_long", &["remove", "--help"]);
}

#[test]
fn help_merge_short() {
    snapshot_help("help_merge_short", &["merge", "-h"]);
}

#[test]
fn help_merge_long() {
    snapshot_help("help_merge_long", &["merge", "--help"]);
}

#[test]
fn help_step_short() {
    snapshot_help("help_step_short", &["step", "-h"]);
}

#[test]
fn help_step_long() {
    snapshot_help("help_step_long", &["step", "--help"]);
}

// =============================================================================
// Config subcommands (long help only - these are less frequently accessed)
// =============================================================================

#[test]
fn help_config_shell() {
    snapshot_help("help_config_shell", &["config", "shell", "--help"]);
}

#[test]
fn help_config_create() {
    snapshot_help("help_config_create", &["config", "create", "--help"]);
}

#[test]
fn help_config_show() {
    snapshot_help("help_config_show", &["config", "show", "--help"]);
}

#[test]
fn help_config_var() {
    snapshot_help("help_config_var", &["config", "var", "--help"]);
}

#[test]
fn help_config_var_get() {
    snapshot_help("help_config_var_get", &["config", "var", "get", "--help"]);
}

#[test]
fn help_config_var_set() {
    snapshot_help("help_config_var_set", &["config", "var", "set", "--help"]);
}

#[test]
fn help_config_var_clear() {
    snapshot_help(
        "help_config_var_clear",
        &["config", "var", "clear", "--help"],
    );
}

#[test]
fn help_config_approvals() {
    snapshot_help("help_config_approvals", &["config", "approvals", "--help"]);
}

#[test]
fn help_config_approvals_add() {
    snapshot_help(
        "help_config_approvals_add",
        &["config", "approvals", "add", "--help"],
    );
}

#[test]
fn help_config_approvals_clear() {
    snapshot_help(
        "help_config_approvals_clear",
        &["config", "approvals", "clear", "--help"],
    );
}
