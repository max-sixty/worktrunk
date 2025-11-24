use crate::common::{TestRepo, make_snapshot_cmd, resolve_git_common_dir, setup_snapshot_settings};
use insta::assert_snapshot;
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use std::thread;
use std::time::Duration;

// Sleep duration constants for background command tests
// These allow time for background processes to complete and write output files

/// Short wait for fast commands (simple echo statements)
const SLEEP_FAST_COMMAND: Duration = Duration::from_millis(100);

/// Standard wait for background commands with sleep/processing
const SLEEP_BACKGROUND_COMMAND: Duration = Duration::from_millis(200);

/// Extended wait for commands that include explicit sleep in the command itself
/// (e.g., "sleep 0.1 && echo ..." requires at least 100ms + margin for CI overhead)
const SLEEP_EXTENDED: Duration = Duration::from_millis(500);

/// Helper to create snapshot with normalized paths and SHAs
///
/// Tests should write to repo.test_config_path() to pre-approve commands.
fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "switch", args, None);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

// ============================================================================
// Post-Create Command Tests (sequential, blocking)
// ============================================================================

#[test]
fn test_post_create_no_config() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Switch without project config should work normally
    snapshot_switch("post_create_no_config", &repo, &["--create", "feature"]);
}

#[test]
fn test_post_create_single_command() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with a single command (string format)
    repo.write_project_config(r#"post-create-command = "echo 'Setup complete'""#);

    repo.commit("Add config");

    // Pre-approve the command by writing to the isolated test config
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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

#[test]
fn test_post_create_multiple_commands_array() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with multiple commands (array format)
    repo.write_project_config(r#"post-create-command = ["echo 'First'", "echo 'Second'"]"#);

    repo.commit("Add config with multiple commands");

    // Pre-approve both commands in temp HOME
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo 'First'",
    "echo 'Second'",
]
"#,
    );

    // Both commands should execute sequentially
    snapshot_switch(
        "post_create_multiple_commands_array",
        &repo,
        &["--create", "feature"],
    );
}

#[test]
fn test_post_create_named_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with named commands (table format)
    repo.write_project_config(
        r#"[post-create-command]
install = "echo 'Installing deps'"
setup = "echo 'Running setup'"
"#,
    );

    repo.commit("Add config with named commands");

    // Pre-approve both commands in temp HOME
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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

#[test]
fn test_post_create_failing_command() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with a command that will fail
    repo.write_project_config(r#"post-create-command = "exit 1""#);

    repo.commit("Add config with failing command");

    // Pre-approve the command in temp HOME
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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

#[test]
fn test_post_create_template_expansion() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with template variables
    repo.write_project_config(
        r#"post-create-command = [
    "echo 'Repo: {{ main_worktree }}' > info.txt",
    "echo 'Branch: {{ branch }}' >> info.txt",
    "echo 'Worktree: {{ worktree }}' >> info.txt",
    "echo 'Root: {{ repo_root }}' >> info.txt"
]"#,
    );

    repo.commit("Add config with templates");

    // Pre-approve all commands in isolated test config
    let repo_name = "test-repo";
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo 'Repo: {{ main_worktree }}' > info.txt",
    "echo 'Branch: {{ branch }}' >> info.txt",
    "echo 'Worktree: {{ worktree }}' >> info.txt",
    "echo 'Root: {{ repo_root }}' >> info.txt",
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
        contents.contains("Branch: feature-test"),
        "Should contain expanded branch name (sanitized), got: {}",
        contents
    );
}

// ============================================================================
// Post-Start Command Tests (parallel, background)
// ============================================================================

#[test]
fn test_post_start_single_background_command() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with a background command
    repo.write_project_config(
        r#"post-start-command = "sleep 0.1 && echo 'Background task done' > background.txt""#,
    );

    repo.commit("Add background command");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    assert!(log_dir.exists(), "Log directory should be created");

    // Wait for the background command to complete
    thread::sleep(SLEEP_EXTENDED);

    // Verify the background command actually ran
    let output_file = worktree_path.join("background.txt");
    assert!(
        output_file.exists(),
        "Background command should have created output file"
    );
}

#[test]
fn test_post_start_multiple_background_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with multiple background commands (table format)
    repo.write_project_config(
        r#"[post-start-command]
task1 = "echo 'Task 1 running' > task1.txt"
task2 = "echo 'Task 2 running' > task2.txt"
"#,
    );

    repo.commit("Add multiple background commands");

    // Pre-approve both commands
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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

    // Wait for background commands
    thread::sleep(SLEEP_BACKGROUND_COMMAND);

    // Verify both tasks ran
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    assert!(worktree_path.join("task1.txt").exists());
    assert!(worktree_path.join("task2.txt").exists());
}

#[test]
fn test_both_post_create_and_post_start() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with both command types
    repo.write_project_config(
        r#"post-create-command = "echo 'Setup done' > setup.txt"

[post-start-command]
server = "sleep 0.05 && echo 'Server running' > server.txt"
"#,
    );

    repo.commit("Add both command types");

    // Pre-approve all commands
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo 'Setup done' > setup.txt",
    "sleep 0.05 && echo 'Server running' > server.txt",
]
"#,
    );

    // Post-create should run first (blocking), then post-start (background)
    snapshot_switch("both_create_and_start", &repo, &["--create", "feature"]);

    // Setup file should exist immediately (post-create is blocking)
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    assert!(
        worktree_path.join("setup.txt").exists(),
        "Post-create command should have completed before wt exits"
    );

    // Wait for background command (command has sleep 0.05 + margin)
    thread::sleep(Duration::from_millis(150));

    // Server file should exist after background task completes
    assert!(
        worktree_path.join("server.txt").exists(),
        "Post-start background command should complete"
    );
}

#[test]
fn test_invalid_toml() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create invalid TOML
    repo.write_project_config("post-create-command = [invalid syntax\n");

    repo.commit("Add invalid config");

    // Should continue without executing commands, showing warning
    snapshot_switch("invalid_toml", &repo, &["--create", "feature"]);
}

// ============================================================================
// Additional Coverage Tests
// ============================================================================

#[test]
fn test_post_start_log_file_captures_output() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create command that writes to both stdout and stderr
    repo.write_project_config(
        r#"post-start-command = "echo 'stdout output' && echo 'stderr output' >&2""#,
    );

    repo.commit("Add command with stdout/stderr");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'stdout output' && echo 'stderr output' >&2"]
"#,
    );

    snapshot_switch(
        "post_start_log_captures_output",
        &repo,
        &["--create", "feature"],
    );

    // Give background command time to complete
    thread::sleep(SLEEP_FAST_COMMAND);

    // Find and read the log file from the common git directory
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    assert!(log_dir.exists(), "Log directory should exist");

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

    let log_contents = fs::read_to_string(&log_files[0]).unwrap();

    // Verify both stdout and stderr were captured
    assert_snapshot!(log_contents, @r"
    stdout output
    stderr output
    ");
}

#[test]
fn test_post_start_invalid_command_handling() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create command with syntax error (missing quote)
    repo.write_project_config(r#"post-start-command = "echo 'unclosed quote""#);

    repo.commit("Add invalid command");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    assert!(
        worktree_path.exists(),
        "Worktree should be created even if post-start command fails"
    );
}

#[test]
fn test_post_start_multiple_commands_separate_logs() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create multiple background commands with distinct output
    repo.write_project_config(
        r#"[post-start-command]
task1 = "echo 'TASK1_OUTPUT'"
task2 = "echo 'TASK2_OUTPUT'"
task3 = "echo 'TASK3_OUTPUT'"
"#,
    );

    repo.commit("Add three background commands");

    // Pre-approve all commands
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "echo 'TASK1_OUTPUT'",
    "echo 'TASK2_OUTPUT'",
    "echo 'TASK3_OUTPUT'",
]
"#,
    );

    snapshot_switch("post_start_separate_logs", &repo, &["--create", "feature"]);

    // Give background commands time to complete
    thread::sleep(SLEEP_FAST_COMMAND);

    // Verify we have 3 separate log files in the common git directory
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("log"))
        .collect();

    assert_eq!(
        log_files.len(),
        3,
        "Should have 3 separate log files, found: {}",
        log_files.len()
    );

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

#[test]
fn test_execute_flag_with_post_start_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create post-start command
    repo.write_project_config(r#"post-start-command = "echo 'Background task' > background.txt""#);

    repo.commit("Add background command");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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

    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");

    // Execute flag file should exist immediately (synchronous)
    assert!(
        worktree_path.join("execute.txt").exists(),
        "Execute command should run synchronously"
    );

    // Wait for background command
    thread::sleep(SLEEP_FAST_COMMAND);

    // Background file should also exist
    assert!(
        worktree_path.join("background.txt").exists(),
        "Post-start command should also run"
    );
}

#[test]
fn test_post_start_complex_shell_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create command with pipes and redirects
    repo.write_project_config(
        r#"post-start-command = "echo 'line1\nline2\nline3' | grep line2 > filtered.txt""#,
    );

    repo.commit("Add complex shell command");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'line1\nline2\nline3' | grep line2 > filtered.txt"]
"#,
    );

    snapshot_switch("post_start_complex_shell", &repo, &["--create", "feature"]);

    // Wait for background command
    thread::sleep(SLEEP_FAST_COMMAND);

    // Verify the piped command worked correctly
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let filtered_file = worktree_path.join("filtered.txt");
    assert!(
        filtered_file.exists(),
        "Complex shell command should create output file"
    );

    let contents = fs::read_to_string(&filtered_file).unwrap();
    assert_snapshot!(contents, @"line2");
}

#[test]
fn test_post_start_multiline_commands_with_newlines() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create command with actual newlines (using TOML triple-quoted string)
    repo.write_project_config(
        r#"post-start-command = """
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
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."test-repo"]
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

    // Wait for background command
    thread::sleep(SLEEP_FAST_COMMAND);

    // Verify the multiline command worked correctly
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let output_file = worktree_path.join("multiline.txt");
    assert!(
        output_file.exists(),
        "Multiline command should create output file"
    );

    let contents = fs::read_to_string(&output_file).unwrap();
    assert_snapshot!(contents, @r"
    first line
    second line
    third line
    ");
}

#[test]
fn test_post_create_multiline_with_control_structures() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Test multiline command with if-else control structure
    repo.write_project_config(
        r#"post-create-command = """
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
        r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."test-repo"]
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
    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
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

/// Test that post-start commands DO NOT run when switching to an existing worktree.
///
/// This is a regression test for a bug where post-start commands were running on ALL
/// `wt switch` operations instead of only on `wt switch --create`.
#[test]
fn test_post_start_skipped_on_existing_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with post-start command
    repo.write_project_config(
        r#"post-start-command = "echo 'POST-START-RAN' > post_start_marker.txt""#,
    );

    repo.commit("Add post-start config");

    // Pre-approve the command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
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
    thread::sleep(SLEEP_BACKGROUND_COMMAND);

    let worktree_path = repo.root_path().parent().unwrap().join("test-repo.feature");
    let marker_file = worktree_path.join("post_start_marker.txt");

    // Verify post-start ran on creation
    assert!(
        marker_file.exists(),
        "Post-start command should run when creating new worktree"
    );

    // Remove the marker file to detect if post-start runs again
    fs::remove_file(&marker_file).unwrap();

    // Second: Switch to EXISTING worktree - post-start should NOT run
    snapshot_switch("post_start_skip_existing", &repo, &["feature"]);

    // Wait briefly to ensure no background command starts
    thread::sleep(SLEEP_FAST_COMMAND);

    // Verify post-start did NOT run when switching to existing worktree
    assert!(
        !marker_file.exists(),
        "Post-start should NOT run when switching to existing worktree"
    );
}
