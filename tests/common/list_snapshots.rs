// Functions here are conditionally used based on platform (#[cfg(not(windows))]).
#![allow(dead_code)]

use super::{TestRepo, setup_snapshot_settings, wt_command};
use insta::Settings;
use std::path::Path;
use std::process::Command;
use worktrunk::styling::DEFAULT_HELP_WIDTH;

pub fn json_settings(repo: &TestRepo) -> Settings {
    let mut settings = setup_snapshot_settings(repo);
    // JSON-specific filters for timestamps and escaped ANSI codes
    settings.add_filter(r#""timestamp": \d+"#, r#""timestamp": 0"#);
    settings.add_filter(r"\\u001b\[32m", "[GREEN]");
    settings.add_filter(r"\\u001b\[31m", "[RED]");
    settings.add_filter(r"\\u001b\[2m", "[DIM]");
    settings.add_filter(r"\\u001b\[0m", "[RESET]");
    settings.add_filter(r"\\\\", "/");
    settings
}

pub fn command(repo: &TestRepo, cwd: &Path) -> Command {
    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg("list").current_dir(cwd);
    cmd
}

pub fn command_readme(repo: &TestRepo, cwd: &Path) -> Command {
    let mut cmd = command(repo, cwd);
    cmd.env("COLUMNS", DEFAULT_HELP_WIDTH.to_string());
    cmd
}

pub fn command_with_width(repo: &TestRepo, width: usize) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.env("COLUMNS", width.to_string());
    cmd
}
