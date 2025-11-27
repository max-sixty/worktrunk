use super::{TestRepo, setup_snapshot_settings, wt_command};
use insta::Settings;
use std::path::Path;
use std::process::Command;

pub fn standard_settings(repo: &TestRepo) -> Settings {
    setup_snapshot_settings(repo)
}

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
    repo.clean_cli_env(&mut cmd);
    cmd.arg("list").current_dir(cwd);
    cmd
}

pub fn command_json(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.arg("--format=json");
    cmd
}

pub fn command_branches(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.arg("--branches");
    cmd
}

pub fn command_with_width(repo: &TestRepo, width: usize) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.env("COLUMNS", width.to_string());
    cmd
}

pub fn command_progressive(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.arg("--progressive");
    cmd
}

pub fn command_no_progressive(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.arg("--no-progressive");
    cmd
}

pub fn command_progressive_json(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.args(["--progressive", "--format=json"]);
    cmd
}

pub fn command_remotes(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.arg("--remotes");
    cmd
}

pub fn command_branches_and_remotes(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.args(["--branches", "--remotes"]);
    cmd
}

pub fn command_no_progressive_json(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.args(["--no-progressive", "--format=json"]);
    cmd
}

pub fn command_progressive_branches(repo: &TestRepo) -> Command {
    let mut cmd = command(repo, repo.root_path());
    cmd.args(["--progressive", "--branches"]);
    cmd
}

pub fn command_task_dag(repo: &TestRepo) -> Command {
    // Task DAG is now the default for progressive mode
    command_progressive(repo)
}

pub fn command_task_dag_full(repo: &TestRepo) -> Command {
    let mut cmd = command_task_dag(repo);
    cmd.arg("--full");
    cmd
}

pub fn command_task_dag_from_dir(repo: &TestRepo, cwd: &Path) -> Command {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.args(["list", "--progressive"]).current_dir(cwd);
    cmd
}
