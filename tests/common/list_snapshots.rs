// Functions here are conditionally used based on platform (#[cfg(not(windows))]).
#![allow(dead_code)]

use super::{TestRepo, wt_command};
use std::path::Path;
use std::process::Command;
use worktrunk::styling::DEFAULT_HELP_WIDTH;

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
