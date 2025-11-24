use crate::common::{
    TestRepo, resolve_git_common_dir,
    shell::{execute_shell_script, generate_init_code, path_export_syntax, wt_bin_dir},
};
use rstest::rstest;
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

fn wait_for_file(path: &std::path::Path, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "File was not created within {:?}: {}",
        timeout,
        path.display()
    );
}

/// Test that post-start background commands work with shell integration
#[rstest]
// Test with bash and fish
#[case("bash")]
#[case("fish")]
fn test_shell_integration_post_start_background(#[case] shell: &str) {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-start-command = "sleep 0.05 && echo 'Background task done' > bg_marker.txt""#,
    )
    .unwrap();

    repo.commit("Add post-start config");

    // Pre-approve the command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["sleep 0.05 && echo 'Background task done' > bg_marker.txt"]
"#,
    )
    .unwrap();

    let init_code = generate_init_code(&repo, shell);
    let bin_path = wt_bin_dir();

    let script = format!(
        r#"
        {}
        {}
        wt switch --create bg-feature
        echo "Switched to worktree"
        pwd
        "#,
        path_export_syntax(shell, &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, shell, &script);

    // Verify that:
    // 1. The switch command completed (shell returned)
    // 2. We're in the new worktree
    assert!(
        output.contains("Switched to worktree") && output.contains("bg-feature"),
        "Expected to see switch completion and be in bg-feature worktree, got: {}",
        output
    );

    // Verify background command actually ran
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.bg-feature");

    // First check if log file was created (proves process was spawned)
    // Logs are centralized in the common git directory
    let git_common_dir = resolve_git_common_dir(&worktree_path);
    let log_dir = git_common_dir.join("wt-logs");
    assert!(
        log_dir.exists(),
        "Log directory should exist at {}",
        log_dir.display()
    );

    // Check for log files
    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(
        !log_files.is_empty(),
        "Should have log files in {}, found: {:?}",
        log_dir.display(),
        log_files
    );

    // Wait for background command to complete (allow plenty of margin on CI)
    wait_for_file(
        worktree_path.join("bg_marker.txt").as_path(),
        Duration::from_secs(1),
    );

    let marker_file = worktree_path.join("bg_marker.txt");
    let content = fs::read_to_string(&marker_file).unwrap();
    assert!(
        content.contains("Background task done"),
        "Expected background task output, got: {}",
        content
    );
}

/// Test that multiple post-start commands run in parallel with shell integration
#[test]
fn test_bash_shell_integration_post_start_parallel() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with multiple background commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start-command]
task1 = "sleep 0.05 && echo 'Task 1' > task1.txt"
task2 = "sleep 0.05 && echo 'Task 2' > task2.txt"
"#,
    )
    .unwrap();

    repo.commit("Add multiple post-start commands");

    // Pre-approve commands
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = [
    "sleep 0.05 && echo 'Task 1' > task1.txt",
    "sleep 0.05 && echo 'Task 2' > task2.txt",
]
"#,
    )
    .unwrap();

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = wt_bin_dir();

    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create parallel-test
        echo "Returned from wt"
        "#,
        bin_path, init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Verify shell returned immediately (didn't wait for background tasks)
    assert!(
        output.contains("Returned from wt"),
        "Expected immediate return from wt, got: {}",
        output
    );

    // Wait for background commands to complete
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.parallel-test");

    wait_for_file(
        worktree_path.join("task1.txt").as_path(),
        Duration::from_secs(2),
    );
    wait_for_file(
        worktree_path.join("task2.txt").as_path(),
        Duration::from_secs(2),
    );
}

/// Test that post-create commands block before shell returns
#[test]
fn test_bash_shell_integration_post_create_blocks() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with blocking command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "echo 'Setup done' > setup.txt""#,
    )
    .unwrap();

    repo.commit("Add post-create command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["echo 'Setup done' > setup.txt"]
"#,
    )
    .unwrap();

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = wt_bin_dir();

    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.blocking-test");
    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create blocking-test
        pwd
        "#,
        bin_path, init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Verify we switched to the worktree
    assert!(
        output.contains("blocking-test"),
        "Expected to be in blocking-test worktree, got: {}",
        output
    );

    // Verify that post-create command completed before wt returned (blocking behavior)
    // The file should exist immediately after wt exits
    let setup_file = worktree_path.join("setup.txt");
    assert!(
        setup_file.exists(),
        "Setup file should exist immediately after wt returns (post-create is blocking)"
    );

    let content = fs::read_to_string(&setup_file).unwrap();
    assert!(
        content.contains("Setup done"),
        "Expected setup output, got: {}",
        content
    );
}

/// Test fish shell specifically with background tasks
#[cfg(unix)]
#[test]
fn test_fish_shell_integration_post_start_background() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start-command]
fish_bg = "sleep 0.05 && echo 'Fish background done' > fish_bg.txt"
"#,
    )
    .unwrap();

    repo.commit("Add fish background command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[projects."test-repo"]
approved-commands = ["sleep 0.05 && echo 'Fish background done' > fish_bg.txt"]
"#,
    )
    .unwrap();

    let init_code = generate_init_code(&repo, "fish");
    let bin_path = wt_bin_dir();

    let script = format!(
        r#"
        set -x PATH {} $PATH
        {}
        wt switch --create fish-bg-test
        echo "Fish shell returned"
        pwd
        "#,
        bin_path, init_code
    );

    let output = execute_shell_script(&repo, "fish", &script);

    // Verify fish shell returned immediately
    assert!(
        output.contains("Fish shell returned") && output.contains("fish-bg-test"),
        "Expected fish shell to return immediately, got: {}",
        output
    );

    // Wait for background command (allow plenty of margin on CI)
    wait_for_file(
        repo.root_path()
            .parent()
            .unwrap()
            .join("test-repo.fish-bg-test")
            .join("fish_bg.txt")
            .as_path(),
        Duration::from_secs(1),
    );

    // Verify background command ran
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.fish-bg-test");
    let marker_file = worktree_path.join("fish_bg.txt");
    assert!(
        marker_file.exists(),
        "Fish background command should have created fish_bg.txt"
    );

    let content = fs::read_to_string(&marker_file).unwrap();
    assert!(
        content.contains("Fish background done"),
        "Expected fish background output, got: {}",
        content
    );
}
