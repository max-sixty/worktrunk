use crate::common::{
    TestRepo, make_snapshot_cmd, repo, repo_with_remote, resolve_git_common_dir, set_temp_home_env,
    setup_snapshot_settings, wait_for_file, wait_for_file_content, wait_for_file_count,
    wait_for_file_lines, wait_for_valid_json,
};
use insta::assert_snapshot;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::thread;
use std::time::Duration; // Used for SLEEP_FOR_ABSENCE_CHECK
use tempfile::TempDir;

/// Wait duration when checking file absence (testing command did NOT run).
/// Must be long enough that a background command would have started and created
/// the file if it were going to. 500ms gives CI systems breathing room.
const SLEEP_FOR_ABSENCE_CHECK: Duration = Duration::from_millis(500);

/// Helper to create snapshot with normalized paths and SHAs
///
/// Tests should write to repo.test_config_path() to pre-approve commands.
/// Uses an isolated HOME to prevent tests from being affected by developer's shell config.
fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    // Create isolated HOME to ensure test determinism
    let temp_home = TempDir::new().unwrap();

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "switch", args, None);
        set_temp_home_env(&mut cmd, temp_home.path());
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// ============================================================================
// Post-Create Command Tests (sequential, blocking)
// ============================================================================

#[rstest]
fn test_post_create_no_config(repo: TestRepo) {
    // Switch without project config should work normally
    snapshot_switch("post_create_no_config", &repo, &["--create", "feature"]);
}

#[rstest]
fn test_post_create_single_command(repo: TestRepo) {
    // Create project config with a single command (string format)
    repo.write_project_config(r#"post-create = "echo 'Setup complete'""#);

    repo.commit("Add config");

    // Pre-approve the command by writing to the isolated test config
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'Setup complete'"]
"#,
    );

    // Command should execute without prompting
    snapshot_switch(
        "post_create_single_command",
        &repo,
        &["--create", "feature"],
    );
}

#[rstest]
fn test_post_create_named_commands(repo: TestRepo) {
    // Create project config with named commands (table format)
    repo.write_project_config(
        r#"[post-create]
install = "echo 'Installing deps'"
setup = "echo 'Running setup'"
"#,
    );

    repo.commit("Add config with named commands");

    // Pre-approve both commands in temp HOME
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = [
    "echo 'Installing deps'",
    "echo 'Running setup'",
]
"#,
    );

    // Commands should execute sequentially
    snapshot_switch(
        "post_create_named_commands",
        &repo,
        &["--create", "feature"],
    );
}

#[rstest]
fn test_post_create_failing_command(repo: TestRepo) {
    // Create project config with a command that will fail
    repo.write_project_config(r#"post-create = "exit 1""#);

    repo.commit("Add config with failing command");

    // Pre-approve the command in temp HOME
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["exit 1"]
"#,
    );

    // Should show warning but continue (worktree should still be created)
    snapshot_switch(
        "post_create_failing_command",
        &repo,
        &["--create", "feature"],
    );
}

#[rstest]
fn test_post_create_template_expansion(repo: TestRepo) {
    // Create project config with template variables
    repo.write_project_config(
        r#"[post-create]
repo = "echo 'Repo: {{ repo }}' > info.txt"
branch = "echo 'Branch: {{ branch }}' >> info.txt"
hash_port = "echo 'Port: {{ branch | hash_port }}' >> info.txt"
worktree = "echo 'Worktree: {{ worktree_path }}' >> info.txt"
root = "echo 'Root: {{ repo_path }}' >> info.txt"
"#,
    );

    repo.commit("Add config with templates");

    // Pre-approve all commands in isolated test config
    let repo_name = "repo";
    repo.write_test_config(
        r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."repo"]
approved-commands = [
    "echo 'Repo: {{ repo }}' > info.txt",
    "echo 'Branch: {{ branch }}' >> info.txt",
    "echo 'Port: {{ branch | hash_port }}' >> info.txt",
    "echo 'Worktree: {{ worktree_path }}' >> info.txt",
    "echo 'Root: {{ repo_path }}' >> info.txt",
]
"#,
    );

    // Commands should execute with expanded templates
    snapshot_switch(
        "post_create_template_expansion",
        &repo,
        &["--create", "feature/test"],
    );

    // Verify template expansion actually worked by checking the output file
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.feature-test", repo_name));
    let info_file = worktree_path.join("info.txt");

    assert!(
        info_file.exists(),
        "info.txt should have been created in the worktree"
    );

    let contents = fs::read_to_string(&info_file).unwrap();

    // Verify that template variables were actually expanded
    assert!(
        contents.contains(&format!("Repo: {}", repo_name)),
        "Should contain expanded repo name, got: {}",
        contents
    );
    assert!(
        contents.contains("Branch: feature/test"),
        "Should contain raw branch name, got: {}",
        contents
    );

    // Verify port is a valid number in the expected range (10000-19999)
    let port_line = contents
        .lines()
        .find(|l| l.starts_with("Port: "))
        .expect("Should contain port line");
    let port: u16 = port_line
        .strip_prefix("Port: ")
        .unwrap()
        .parse()
        .expect("Port should be a valid number");
    assert!(
        (10000..20000).contains(&port),
        "Port should be in range 10000-19999, got: {}",
        port
    );
}

#[rstest]
fn test_post_create_default_branch_template(repo: TestRepo) {
    // Create project config with default_branch template variable
    repo.write_project_config(
        r#"post-create = "echo 'Default: {{ default_branch }}' > default.txt""#,
    );

    repo.commit("Add config with default_branch template");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'Default: {{ default_branch }}' > default.txt"]
"#,
    );

    // Create a feature branch worktree (--yes skips approval prompt)
    snapshot_switch(
        "post_create_default_branch_template",
        &repo,
        &["--create", "feature", "--yes"],
    );

    // Verify template expansion actually worked
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let default_file = worktree_path.join("default.txt");

    assert!(
        default_file.exists(),
        "default.txt should have been created in the worktree"
    );

    let contents = fs::read_to_string(&default_file).unwrap();
    assert!(
        contents.contains("Default: main"),
        "Should contain expanded default_branch, got: {}",
        contents
    );
}

#[rstest]
fn test_post_create_git_variables_template(#[from(repo_with_remote)] repo: TestRepo) {
    // Set up an upstream tracking branch
    repo.git_command()
        .args(["push", "-u", "origin", "main"])
        .output()
        .expect("failed to push");

    // Create project config with git-related template variables
    repo.write_project_config(
        r#"[post-create]
commit = "echo 'Commit: {{ commit }}' > git_vars.txt"
short = "echo 'Short: {{ short_commit }}' >> git_vars.txt"
remote = "echo 'Remote: {{ remote }}' >> git_vars.txt"
worktree_name = "echo 'Worktree Name: {{ worktree_name }}' >> git_vars.txt"
"#,
    );

    repo.commit("Add config with git template variables");

    // Create a feature branch worktree (--yes skips approval prompt)
    snapshot_switch(
        "post_create_git_variables_template",
        &repo,
        &["--create", "feature", "--yes"],
    );

    // Verify template expansion actually worked
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let vars_file = worktree_path.join("git_vars.txt");

    assert!(
        vars_file.exists(),
        "git_vars.txt should have been created in the worktree"
    );

    let contents = fs::read_to_string(&vars_file).unwrap();

    // Verify commit variable (should be 40 char hex)
    assert!(
        contents.contains("Commit: ")
            && contents.lines().any(|l| {
                l.starts_with("Commit: ") && l.len() == 48 // "Commit: " (8) + 40 hex chars
            }),
        "Should contain expanded commit SHA, got: {}",
        contents
    );

    // Verify short_commit variable (should be 7 char hex)
    assert!(
        contents.contains("Short: ")
            && contents.lines().any(|l| {
                l.starts_with("Short: ") && l.len() == 14 // "Short: " (7) + 7 hex chars
            }),
        "Should contain expanded short_commit SHA, got: {}",
        contents
    );

    // Verify remote variable
    assert!(
        contents.contains("Remote: origin"),
        "Should contain expanded remote name, got: {}",
        contents
    );

    // Verify worktree_name variable (basename of worktree path)
    assert!(
        contents.contains("Worktree Name: repo.feature"),
        "Should contain expanded worktree_name, got: {}",
        contents
    );
}

#[rstest]
fn test_post_create_upstream_template(#[from(repo_with_remote)] repo: TestRepo) {
    // Push main to set up tracking
    repo.git_command()
        .args(["push", "-u", "origin", "main"])
        .output()
        .expect("failed to push main");

    // Create project config with upstream template variable
    repo.write_project_config(r#"post-create = "echo 'Upstream: {{ upstream }}' > upstream.txt""#);

    repo.commit("Add config with upstream template");

    // Create a feature branch based on main (which has upstream tracking)
    // The new branch will inherit upstream since it's created from main
    snapshot_switch(
        "post_create_upstream_template",
        &repo,
        &["--create", "feature", "--yes"],
    );

    // Verify template expansion actually worked
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let upstream_file = worktree_path.join("upstream.txt");

    assert!(
        upstream_file.exists(),
        "upstream.txt should have been created in the worktree"
    );

    let contents = fs::read_to_string(&upstream_file).unwrap();
    // New branches don't have upstream until pushed - upstream should be empty
    assert!(
        contents.contains("Upstream: "),
        "Should have upstream line (possibly empty), got: {}",
        contents
    );
}

#[rstest]
fn test_post_create_base_variables(repo: TestRepo) {
    // Create project config with base template variables
    repo.write_project_config(
        r#"[post-create]
base = "echo 'Base: {{ base }}' > base_info.txt"
base_path = "echo 'Base Path: {{ base_worktree_path }}' >> base_info.txt"
"#,
    );

    repo.commit("Add config with base template variables");

    // Pre-approve the commands
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = [
    "echo 'Base: {{ base }}' > base_info.txt",
    "echo 'Base Path: {{ base_worktree_path }}' >> base_info.txt",
]
"#,
    );

    // Create a feature branch worktree from main
    snapshot_switch(
        "post_create_base_variables",
        &repo,
        &["--create", "feature", "--base", "main"],
    );

    // Verify template expansion actually worked
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let base_file = worktree_path.join("base_info.txt");

    assert!(
        base_file.exists(),
        "base_info.txt should have been created in the worktree"
    );

    let contents = fs::read_to_string(&base_file).unwrap();

    // Verify base variable (branch name we branched from)
    assert!(
        contents.contains("Base: main"),
        "Should contain expanded base branch, got: {}",
        contents
    );

    // Verify base_worktree_path variable (path to main worktree)
    // The path should contain the repo root (main worktree is at repo root)
    assert!(
        contents.contains("Base Path: "),
        "Should have base_worktree_path line, got: {}",
        contents
    );

    // The base_worktree_path should be the main worktree's path (POSIX format)
    let base_path_line = contents
        .lines()
        .find(|l| l.starts_with("Base Path: "))
        .expect("Should have Base Path line");
    let base_path = base_path_line.strip_prefix("Base Path: ").unwrap();

    // Convert expected path to POSIX format for comparison
    let expected_base_path = worktrunk::path::to_posix_path(&repo.root_path().to_string_lossy());
    assert_eq!(
        base_path, expected_base_path,
        "Base path should match main worktree path"
    );
}

#[rstest]
fn test_post_create_json_stdin(repo: TestRepo) {
    use crate::common::wt_command;

    // Create project config with a command that reads JSON from stdin
    // Use cat to capture stdin to a file
    repo.write_project_config(r#"post-create = "cat > context.json""#);

    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["cat > context.json"]
"#,
    );

    // Create worktree - this should pipe JSON to the hook's stdin
    let temp_home = TempDir::new().unwrap();
    let mut cmd = wt_command();
    cmd.args(["switch", "--create", "feature-json"])
        .current_dir(repo.root_path())
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    let output = cmd.output().expect("failed to run wt switch");

    assert!(
        output.status.success(),
        "wt switch should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Find the worktree and read the JSON
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature-json");
    let json_file = worktree_path.join("context.json");

    assert!(
        json_file.exists(),
        "context.json should have been created from stdin"
    );

    let contents = fs::read_to_string(&json_file).unwrap();

    // Parse and verify the JSON contains expected fields
    let json: serde_json::Value = serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("Should be valid JSON: {}\nContents: {}", e, contents));

    assert!(
        json.get("repo").is_some(),
        "JSON should contain 'repo' field"
    );
    assert!(
        json.get("branch").is_some(),
        "JSON should contain 'branch' field"
    );
    assert_eq!(
        json["branch"].as_str(),
        Some("feature-json"),
        "Branch should be sanitized (feature-json)"
    );
    assert!(
        json.get("worktree").is_some(),
        "JSON should contain 'worktree' field"
    );
    assert!(
        json.get("repo_root").is_some(),
        "JSON should contain 'repo_root' field"
    );
    assert_eq!(
        json["hook_type"].as_str(),
        Some("post-create"),
        "JSON should contain hook_type"
    );
}

#[rstest]
#[cfg(unix)]
fn test_post_create_script_reads_json(repo: TestRepo) {
    use crate::common::wt_command;
    use std::os::unix::fs::PermissionsExt;

    // Create a scripts directory and a Python script that reads JSON from stdin
    let scripts_dir = repo.root_path().join("scripts");
    fs::create_dir_all(&scripts_dir).unwrap();

    let script_content = r#"#!/usr/bin/env python3
import json
import sys

ctx = json.load(sys.stdin)
with open('hook_output.txt', 'w') as f:
    f.write(f"repo={ctx['repo']}\n")
    f.write(f"branch={ctx['branch']}\n")
    f.write(f"hook_type={ctx['hook_type']}\n")
    f.write(f"hook_name={ctx.get('hook_name', 'unnamed')}\n")
"#;
    let script_path = scripts_dir.join("setup.py");
    fs::write(&script_path, script_content).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    // Create project config that runs the script
    repo.write_project_config(
        r#"[post-create]
setup = "./scripts/setup.py"
"#,
    );

    repo.commit("Add setup script and config");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["./scripts/setup.py"]
"#,
    );

    // Create worktree
    let temp_home = TempDir::new().unwrap();
    let mut cmd = wt_command();
    cmd.args(["switch", "--create", "feature-script"])
        .current_dir(repo.root_path())
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    let output = cmd.output().expect("failed to run wt switch");

    assert!(
        output.status.success(),
        "wt switch should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Find the worktree and verify the script wrote the expected output
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.feature-script");
    let output_file = worktree_path.join("hook_output.txt");

    assert!(
        output_file.exists(),
        "Script should have created hook_output.txt"
    );

    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("repo=repo"),
        "Output should contain repo name: {}",
        contents
    );
    assert!(
        contents.contains("branch=feature-script"),
        "Output should contain branch: {}",
        contents
    );
    assert!(
        contents.contains("hook_type=post-create"),
        "Output should contain hook_type: {}",
        contents
    );
    assert!(
        contents.contains("hook_name=setup"),
        "Output should contain hook_name: {}",
        contents
    );
}

#[rstest]
fn test_post_start_json_stdin(repo: TestRepo) {
    use crate::common::wt_command;

    // Create project config with a background command that reads JSON from stdin
    repo.write_project_config(r#"post-start = "cat > context.json""#);

    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["cat > context.json"]
"#,
    );

    // Create worktree
    let temp_home = TempDir::new().unwrap();
    let mut cmd = wt_command();
    cmd.args(["switch", "--create", "bg-json"])
        .current_dir(repo.root_path())
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    let output = cmd.output().expect("failed to run wt switch");

    assert!(
        output.status.success(),
        "wt switch should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Find the worktree and wait for valid JSON (polls until cat finishes writing)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.bg-json");
    let json_file = worktree_path.join("context.json");
    let json = wait_for_valid_json(&json_file);

    assert_eq!(
        json["branch"].as_str(),
        Some("bg-json"),
        "Background hook should receive JSON with branch"
    );
    assert!(
        json.get("repo").is_some(),
        "Background hook should receive JSON with repo"
    );
    assert_eq!(
        json["hook_type"].as_str(),
        Some("post-start"),
        "Background hook should receive hook_type"
    );
}

// ============================================================================
// Post-Start Command Tests (parallel, background)
// ============================================================================

#[rstest]
fn test_post_start_single_background_command(repo: TestRepo) {
    // Create project config with a background command
    repo.write_project_config(
        r#"post-start = "sleep 0.1 && echo 'Background task done' > background.txt""#,
    );

    repo.commit("Add background command");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["sleep 0.1 && echo 'Background task done' > background.txt"]
"#,
    );

    // Command should spawn in background (wt exits immediately)
    snapshot_switch(
        "post_start_single_background",
        &repo,
        &["--create", "feature"],
    );

    // Verify log file was created in the common git directory
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    assert!(log_dir.exists());

    // Wait for the background command to complete
    let output_file = worktree_path.join("background.txt");
    wait_for_file(output_file.as_path());
}

#[rstest]
fn test_post_start_multiple_background_commands(repo: TestRepo) {
    // Create project config with multiple background commands (table format)
    repo.write_project_config(
        r#"[post-start]
task1 = "echo 'Task 1 running' > task1.txt"
task2 = "echo 'Task 2 running' > task2.txt"
"#,
    );

    repo.commit("Add multiple background commands");

    // Pre-approve both commands
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = [
    "echo 'Task 1 running' > task1.txt",
    "echo 'Task 2 running' > task2.txt",
]
"#,
    );

    // Commands should spawn in parallel
    snapshot_switch(
        "post_start_multiple_background",
        &repo,
        &["--create", "feature"],
    );

    // Wait for both background commands
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    wait_for_file(worktree_path.join("task1.txt").as_path());
    wait_for_file(worktree_path.join("task2.txt").as_path());
}

#[rstest]
fn test_both_post_create_and_post_start(repo: TestRepo) {
    // Create project config with both command types
    repo.write_project_config(
        r#"post-create = "echo 'Setup done' > setup.txt"

[post-start]
server = "sleep 0.05 && echo 'Server running' > server.txt"
"#,
    );

    repo.commit("Add both command types");

    // Pre-approve all commands
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = [
    "echo 'Setup done' > setup.txt",
    "sleep 0.05 && echo 'Server running' > server.txt",
]
"#,
    );

    // Post-create should run first (blocking), then post-start (background)
    snapshot_switch("both_create_and_start", &repo, &["--create", "feature"]);

    // Setup file should exist immediately (post-create is blocking)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    assert!(
        worktree_path.join("setup.txt").exists(),
        "Post-create command should have completed before wt exits"
    );

    // Wait for background command
    wait_for_file(worktree_path.join("server.txt").as_path());
}

#[rstest]
fn test_invalid_toml(repo: TestRepo) {
    // Create invalid TOML
    repo.write_project_config("post-create = [invalid syntax\n");

    repo.commit("Add invalid config");

    // Should continue without executing commands, showing warning
    snapshot_switch("invalid_toml", &repo, &["--create", "feature"]);
}

// ============================================================================
// Additional Coverage Tests
// ============================================================================

#[rstest]
fn test_post_start_log_file_captures_output(repo: TestRepo) {
    // Create command that writes to both stdout and stderr
    repo.write_project_config(r#"post-start = "echo 'stdout output' && echo 'stderr output' >&2""#);

    repo.commit("Add command with stdout/stderr");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'stdout output' && echo 'stderr output' >&2"]
"#,
    );

    snapshot_switch(
        "post_start_log_captures_output",
        &repo,
        &["--create", "feature"],
    );

    // Wait for log file to be created (not just the directory)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    wait_for_file_count(&log_dir, "log", 1);

    // Find the log file
    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("log"))
        .collect();

    assert_eq!(
        log_files.len(),
        1,
        "Should have exactly one log file, found: {:?}",
        log_files
    );

    // Wait for content to be written (background command might still be writing)
    let log_file = &log_files[0];
    wait_for_file_content(log_file);

    let log_contents = fs::read_to_string(log_file).unwrap();

    // Verify both stdout and stderr were captured
    assert_snapshot!(log_contents, @"
    stdout output
    stderr output
    ");
}

#[rstest]
fn test_post_start_invalid_command_handling(repo: TestRepo) {
    // Create command with syntax error (missing quote)
    repo.write_project_config(r#"post-start = "echo 'unclosed quote""#);

    repo.commit("Add invalid command");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'unclosed quote"]
"#,
    );

    // wt should still complete successfully even if background command has errors
    snapshot_switch(
        "post_start_invalid_command",
        &repo,
        &["--create", "feature"],
    );

    // Verify worktree was created despite command error
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    assert!(
        worktree_path.exists(),
        "Worktree should be created even if post-start command fails"
    );
}

#[rstest]
fn test_post_start_multiple_commands_separate_logs(repo: TestRepo) {
    // Create multiple background commands with distinct output
    repo.write_project_config(
        r#"[post-start]
task1 = "echo 'TASK1_OUTPUT'"
task2 = "echo 'TASK2_OUTPUT'"
task3 = "echo 'TASK3_OUTPUT'"
"#,
    );

    repo.commit("Add three background commands");

    // Pre-approve all commands
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = [
    "echo 'TASK1_OUTPUT'",
    "echo 'TASK2_OUTPUT'",
    "echo 'TASK3_OUTPUT'",
]
"#,
    );

    snapshot_switch("post_start_separate_logs", &repo, &["--create", "feature"]);

    // Wait for all 3 log files to be created (poll, don't use fixed sleep)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    wait_for_file_count(&log_dir, "log", 3);

    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("log"))
        .collect();

    // Wait for content to be flushed in each log file before reading
    for entry in &log_files {
        wait_for_file_content(&entry.path());
    }

    // Read all log files and verify no cross-contamination
    let mut found_outputs = vec![false, false, false];
    for entry in log_files {
        let contents = fs::read_to_string(entry.path()).unwrap();
        let count_task1 = contents.matches("TASK1_OUTPUT").count();
        let count_task2 = contents.matches("TASK2_OUTPUT").count();
        let count_task3 = contents.matches("TASK3_OUTPUT").count();

        // Each log should contain exactly one task's output
        let total_outputs = count_task1 + count_task2 + count_task3;
        assert_eq!(
            total_outputs,
            1,
            "Each log should contain exactly one task's output, found {} in {:?}",
            total_outputs,
            entry.path()
        );

        if count_task1 == 1 {
            found_outputs[0] = true;
        }
        if count_task2 == 1 {
            found_outputs[1] = true;
        }
        if count_task3 == 1 {
            found_outputs[2] = true;
        }
    }

    assert!(
        found_outputs.iter().all(|&x| x),
        "Should find output from all three tasks, found: {:?}",
        found_outputs
    );
}

#[rstest]
fn test_execute_flag_with_post_start_commands(repo: TestRepo) {
    // Create post-start command
    repo.write_project_config(r#"post-start = "echo 'Background task' > background.txt""#);

    repo.commit("Add background command");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'Background task' > background.txt"]
"#,
    );

    // Use --execute flag along with post-start command
    snapshot_switch(
        "execute_with_post_start",
        &repo,
        &[
            "--create",
            "feature",
            "--execute",
            "echo 'Execute flag' > execute.txt",
        ],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // Execute flag file should exist immediately (synchronous)
    assert!(
        worktree_path.join("execute.txt").exists(),
        "Execute command should run synchronously"
    );

    // Wait for background command to complete
    wait_for_file(worktree_path.join("background.txt").as_path());
}

#[rstest]
fn test_post_start_complex_shell_commands(repo: TestRepo) {
    // Create command with pipes and redirects
    repo.write_project_config(
        r#"post-start = "echo 'line1\nline2\nline3' | grep line2 > filtered.txt""#,
    );

    repo.commit("Add complex shell command");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'line1\nline2\nline3' | grep line2 > filtered.txt"]
"#,
    );

    snapshot_switch("post_start_complex_shell", &repo, &["--create", "feature"]);

    // Wait for background command to create the file AND flush content
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let filtered_file = worktree_path.join("filtered.txt");
    wait_for_file_content(filtered_file.as_path());

    let contents = fs::read_to_string(&filtered_file).unwrap();
    assert_snapshot!(contents, @"line2");
}

#[rstest]
fn test_post_start_multiline_commands_with_newlines(repo: TestRepo) {
    // Create command with actual newlines (using TOML triple-quoted string)
    repo.write_project_config(
        r#"post-start = """
echo 'first line' > multiline.txt
echo 'second line' >> multiline.txt
echo 'third line' >> multiline.txt
"""
"#,
    );

    repo.commit("Add multiline command with actual newlines");

    // Pre-approve the command
    let multiline_cmd = "echo 'first line' > multiline.txt
echo 'second line' >> multiline.txt
echo 'third line' >> multiline.txt
";
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ repo }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["""
{}"""]
"#,
        multiline_cmd
    ));

    snapshot_switch(
        "post_start_multiline_with_newlines",
        &repo,
        &["--create", "feature"],
    );

    // Wait for background command to write all 3 lines (not just the first)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let output_file = worktree_path.join("multiline.txt");
    wait_for_file_lines(output_file.as_path(), 3);

    let contents = fs::read_to_string(&output_file).unwrap();
    assert_snapshot!(contents, @"
    first line
    second line
    third line
    ");
}

#[rstest]
fn test_post_create_multiline_with_control_structures(repo: TestRepo) {
    // Test multiline command with if-else control structure
    repo.write_project_config(
        r#"post-create = """
if [ ! -f test.txt ]; then
  echo 'File does not exist' > result.txt
else
  echo 'File exists' > result.txt
fi
"""
"#,
    );

    repo.commit("Add multiline control structure");

    // Pre-approve the command
    let multiline_cmd = "if [ ! -f test.txt ]; then
  echo 'File does not exist' > result.txt
else
  echo 'File exists' > result.txt
fi
";
    repo.write_test_config(&format!(
        r#"worktree-path = "../{{{{ repo }}}}.{{{{ branch }}}}"

[projects."repo"]
approved-commands = ["""
{}"""]
"#,
        multiline_cmd
    ));

    snapshot_switch(
        "post_create_multiline_control_structure",
        &repo,
        &["--create", "feature"],
    );

    // Verify the command executed correctly
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let result_file = worktree_path.join("result.txt");
    assert!(
        result_file.exists(),
        "Control structure command should create result file"
    );

    let contents = fs::read_to_string(&result_file).unwrap();
    assert_snapshot!(contents, @"File does not exist");
}

// ============================================================================
// Regression Tests
// ============================================================================

///
/// This is a regression test for a bug where post-start commands were running on ALL
/// `wt switch` operations instead of only on `wt switch --create`.
#[rstest]
fn test_post_start_skipped_on_existing_worktree(repo: TestRepo) {
    // Create project config with post-start command
    repo.write_project_config(r#"post-start = "echo 'POST-START-RAN' > post_start_marker.txt""#);

    repo.commit("Add post-start config");

    // Pre-approve the command
    repo.write_test_config(
        r#"[projects."repo"]
approved-commands = ["echo 'POST-START-RAN' > post_start_marker.txt"]
"#,
    );

    // First: Create worktree - post-start SHOULD run
    snapshot_switch(
        "post_start_create_with_command",
        &repo,
        &["--create", "feature"],
    );

    // Wait for background post-start command to complete
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("post_start_marker.txt");
    wait_for_file(marker_file.as_path());

    // Remove the marker file to detect if post-start runs again
    fs::remove_file(&marker_file).unwrap();

    // Second: Switch to EXISTING worktree - post-start should NOT run
    snapshot_switch("post_start_skip_existing", &repo, &["feature"]);

    // Wait to ensure no background command starts (testing absence requires fixed wait)
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    // Verify post-start did NOT run when switching to existing worktree
    assert!(
        !marker_file.exists(),
        "Post-start should NOT run when switching to existing worktree"
    );
}
