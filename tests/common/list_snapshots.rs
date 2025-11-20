use super::{TestRepo, wt_command};
use insta::Settings;
use std::path::Path;
use std::process::Command;

fn base_settings(repo: &TestRepo) -> Settings {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    normalize_worktree_paths(&mut settings, repo);
    settings.add_filter(r"\\", "/");
    settings
}

pub fn standard_settings(repo: &TestRepo) -> Settings {
    let mut settings = base_settings(repo);
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");
    // Normalize WORKTRUNK_CONFIG_PATH across platforms (macOS, Linux, Windows)
    // macOS: /var/folders/.../T/.tmpXXX/test-config.toml
    // Linux: /tmp/.tmpXXX/test-config.toml
    settings.add_filter(
        r"(/var/folders/[^/]+/[^/]+/T/\.tmp[^/]+|/tmp/\.tmp[^/]+)/test-config\.toml",
        "[TEST_TEMP]/test-config.toml",
    );
    settings
}

pub fn json_settings(repo: &TestRepo) -> Settings {
    let mut settings = base_settings(repo);
    settings.add_filter(r#""head": "[0-9a-f]{40}""#, r#""head": "[SHA]""#);
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

fn normalize_worktree_paths(settings: &mut Settings, repo: &TestRepo) {
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }
}
