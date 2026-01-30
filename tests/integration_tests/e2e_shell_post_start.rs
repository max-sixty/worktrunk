//! Post-start command shell integration tests.
#![cfg(all(unix, feature = "shell-integration-tests"))]

use crate::common::{
    TestRepo, repo, resolve_git_common_dir,
    shell::{execute_shell_script, generate_init_code, path_export_syntax, wt_bin_dir},
    wait_for_file, wait_for_file_content,
};
use rstest::rstest;
use std::fs;

#[rstest]
// Test with bash and fish
#[case("bash")]
#[case("fish")]
fn test_shell_integration_post_start_background(#[case] shell: &str, repo: TestRepo) {
    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-start = "sleep 0.05 && echo 'Background task done' > bg_marker.txt""#,
    )
    .unwrap();

    repo.commit("Add post-start config");

    // Pre-approve the command
    fs::write(
        repo.test_config_path(),
        r#"[projects."../origin"]
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
    let worktree_path = repo.root_path().parent().unwrap().join("repo.bg-feature");

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

    // Wait for background command to complete AND flush content (allow plenty of margin on CI)
    let marker_file = worktree_path.join("bg_marker.txt");
    wait_for_file_content(marker_file.as_path());

    let content = fs::read_to_string(&marker_file).unwrap();
    assert!(
        content.contains("Background task done"),
        "Expected background task output, got: {}",
        content
    );
}

#[rstest]
fn test_bash_shell_integration_post_start_parallel(repo: TestRepo) {
    // Create project config with multiple background commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start]
task1 = "sleep 0.05 && echo 'Task 1' > task1.txt"
task2 = "sleep 0.05 && echo 'Task 2' > task2.txt"
"#,
    )
    .unwrap();

    repo.commit("Add multiple post-start commands");

    // Pre-approve commands
    fs::write(
        repo.test_config_path(),
        r#"[projects."../origin"]
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
        .join("repo.parallel-test");

    wait_for_file(worktree_path.join("task1.txt").as_path());
    wait_for_file(worktree_path.join("task2.txt").as_path());
}

#[rstest]
fn test_bash_shell_integration_post_create_blocks(repo: TestRepo) {
    // Create project config with blocking command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create = "echo 'Setup done' > setup.txt""#,
    )
    .unwrap();

    repo.commit("Add post-create command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"[projects."../origin"]
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
        .join("repo.blocking-test");
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

#[cfg(unix)]
#[rstest]
fn test_fish_shell_integration_post_start_background(repo: TestRepo) {
    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start]
fish_bg = "sleep 0.05 && echo 'Fish background done' > fish_bg.txt"
"#,
    )
    .unwrap();

    repo.commit("Add fish background command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"[projects."../origin"]
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

    // Wait for background command AND flush content (allow plenty of margin on CI)
    let worktree_path = repo.root_path().parent().unwrap().join("repo.fish-bg-test");
    let marker_file = worktree_path.join("fish_bg.txt");
    wait_for_file_content(marker_file.as_path());

    let content = fs::read_to_string(&marker_file).unwrap();
    assert!(
        content.contains("Fish background done"),
        "Expected fish background output, got: {}",
        content
    );
}
