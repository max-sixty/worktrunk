use super::{TestRepo, wt_command};
use insta::Settings;
use std::path::Path;
use std::process::Command;

fn base_settings(repo: &TestRepo) -> Settings {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    normalize_worktree_paths(&mut settings, repo);
    settings.add_filter(r"\\", "/");

    // Redact volatile metadata captured by insta-cmd (same as common::setup_snapshot_settings)
    settings.add_redaction(".env.GIT_CONFIG_GLOBAL", "[TEST_GIT_CONFIG]");
    settings.add_redaction(".env.WORKTRUNK_CONFIG_PATH", "[TEST_CONFIG]");
    settings.add_redaction(".env.HOME", "[TEST_HOME]");
    settings.add_redaction(".env.XDG_CONFIG_HOME", "[TEST_CONFIG_HOME]");
    settings.add_redaction(".env.PATH", "[PATH]");

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
        "[TEST_CONFIG]",
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

fn normalize_worktree_paths(settings: &mut Settings, repo: &TestRepo) {
    // Canonicalize to handle macOS /var -> /private/var symlink
    let root_canonical = repo
        .root_path()
        .canonicalize()
        .unwrap_or_else(|_| repo.root_path().to_path_buf());
    settings.add_filter(&regex::escape(root_canonical.to_str().unwrap()), "[REPO]");

    for (name, path) in &repo.worktrees {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        settings.add_filter(
            &regex::escape(canonical.to_str().unwrap()),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }
}
