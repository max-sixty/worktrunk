//! Snapshot tests for top-level `--help` output.
//!
//! These ensure our compact help formatting stays stable across releases and
//! catches accidental regressions in wording or wrapping.

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

#[test]
fn help_root() {
    snapshot_help("help_root", &["--help"]);
}

#[test]
fn help_config_shell() {
    snapshot_help("help_config_shell", &["config", "shell", "--help"]);
}

#[test]
fn help_config() {
    snapshot_help("help_config", &["config", "--help"]);
}

#[test]
fn help_beta() {
    snapshot_help("help_beta", &["beta", "--help"]);
}

#[test]
fn help_list() {
    snapshot_help("help_list", &["list", "--help"]);
}

#[test]
fn help_switch() {
    snapshot_help("help_switch", &["switch", "--help"]);
}

#[test]
fn help_remove() {
    snapshot_help("help_remove", &["remove", "--help"]);
}

#[test]
fn help_merge() {
    snapshot_help("help_merge", &["merge", "--help"]);
}

#[test]
fn help_config_create() {
    snapshot_help("help_config_create", &["config", "create", "--help"]);
}

#[test]
fn help_config_list() {
    snapshot_help("help_config_list", &["config", "list", "--help"]);
}

#[test]
fn help_config_refresh_cache() {
    snapshot_help(
        "help_config_refresh_cache",
        &["config", "refresh-cache", "--help"],
    );
}

#[test]
fn help_config_status_set() {
    snapshot_help(
        "help_config_status_set",
        &["config", "status", "set", "--help"],
    );
}

#[test]
fn help_config_status_unset() {
    snapshot_help(
        "help_config_status_unset",
        &["config", "status", "unset", "--help"],
    );
}

#[test]
fn help_config_approvals() {
    snapshot_help("help_config_approvals", &["config", "approvals", "--help"]);
}

#[test]
fn help_config_approvals_ask() {
    snapshot_help(
        "help_config_approvals_ask",
        &["config", "approvals", "ask", "--help"],
    );
}

#[test]
fn help_config_approvals_clear() {
    snapshot_help(
        "help_config_approvals_clear",
        &["config", "approvals", "clear", "--help"],
    );
}
