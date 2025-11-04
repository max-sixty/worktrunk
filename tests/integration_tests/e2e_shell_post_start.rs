use crate::common::TestRepo;
use insta_cmd::get_cargo_bin;
use rstest::rstest;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// Map shell display names to actual binary names
fn get_shell_binary(shell: &str) -> &str {
    match shell {
        "nushell" => "nu",
        "powershell" => "pwsh",
        "oil" => "osh",
        _ => shell,
    }
}

/// Execute a shell script in the given shell and return stdout
fn execute_shell_script(repo: &TestRepo, shell: &str, script: &str) -> String {
    let binary = get_shell_binary(shell);
    let mut cmd = Command::new(binary);
    repo.clean_cli_env(&mut cmd);

    // Additional shell-specific isolation to prevent user config interference
    cmd.env_remove("BASH_ENV");
    cmd.env_remove("ENV"); // for sh/dash
    cmd.env_remove("ZDOTDIR"); // for zsh
    cmd.env_remove("XONSHRC"); // for xonsh
    cmd.env_remove("XDG_CONFIG_HOME"); // for elvish and others

    // Prevent loading user config files
    match shell {
        "fish" => {
            cmd.arg("--no-config");
        }
        "powershell" | "pwsh" => {
            cmd.arg("-NoProfile");
        }
        "xonsh" => {
            cmd.arg("--no-rc");
        }
        "nushell" | "nu" => {
            cmd.arg("--no-config-file");
        }
        _ => {}
    }

    let output = cmd
        .arg("-c")
        .arg(script)
        .current_dir(repo.root_path())
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute {} script: {}", shell, e));

    if !output.status.success() {
        panic!(
            "Shell script failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).expect("Invalid UTF-8 in output")
}

/// Generate shell integration code for the given shell
fn generate_init_code(repo: &TestRepo, shell: &str) -> String {
    let mut cmd = Command::new(get_cargo_bin("wt"));
    repo.clean_cli_env(&mut cmd);

    let output = cmd
        .args(["init", shell])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to generate init code");

    // For shells that don't support completions, the command will exit with code 1
    // but still output the shell integration code to stdout. We can use that output.
    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in init code");

    if !output.status.success() && stdout.trim().is_empty() {
        panic!(
            "Failed to generate init code:\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    stdout
}

/// Generate shell-specific PATH export syntax
fn path_export_syntax(shell: &str, bin_path: &str) -> String {
    match shell {
        "fish" => format!(r#"set -x PATH {} $PATH"#, bin_path),
        "nushell" => format!(r#"$env.PATH = ($env.PATH | prepend "{}")"#, bin_path),
        "powershell" => format!(r#"$env:PATH = "{}:$env:PATH""#, bin_path),
        "elvish" => format!(r#"set E:PATH = {}:$E:PATH"#, bin_path),
        "xonsh" => format!(r#"$PATH.insert(0, "{}")"#, bin_path),
        _ => format!(r#"export PATH="{}:$PATH""#, bin_path), // bash, zsh, oil
    }
}

/// Test that post-start background commands work with shell integration
#[rstest]
// Test with bash (POSIX baseline) and fish (different syntax)
// zsh removed - too similar to bash
#[case("bash")]
#[case("fish")]
// Tier 2: Shells requiring extra setup
// TODO: Fix non-core shells - same issues as e2e_shell.rs
// #[cfg_attr(feature = "tier-2-integration-tests", case("elvish"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("nushell"))]
#[cfg_attr(feature = "tier-2-integration-tests", case("oil"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("xonsh"))]
fn test_e2e_post_start_background_command(#[case] shell: &str) {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-start-command = "sleep 0.5 && echo 'Background task done' > bg_marker.txt""#,
    )
    .expect("Failed to write config");

    repo.commit("Add post-start config");

    // Pre-approve the command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{main-worktree}.{branch}"

[projects."test-repo"]
approved-commands = ["sleep 0.5 && echo 'Background task done' > bg_marker.txt"]
"#,
    )
    .expect("Failed to write user config");

    let init_code = generate_init_code(&repo, shell);
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

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

    // Wait for background command to complete
    thread::sleep(Duration::from_secs(2));

    // Verify background command actually ran
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.bg-feature");

    // First check if log file was created (proves process was spawned)
    let git_dir = worktree_path.join(".git");
    let actual_git_dir = if git_dir.is_file() {
        let content = fs::read_to_string(&git_dir).expect("Failed to read .git file");
        let gitdir_path = content
            .trim()
            .strip_prefix("gitdir: ")
            .expect("Invalid .git format");
        std::path::PathBuf::from(gitdir_path)
    } else {
        git_dir
    };

    let log_dir = actual_git_dir.join("wt-logs");
    assert!(
        log_dir.exists(),
        "Log directory should exist at {}",
        log_dir.display()
    );

    // Check for log files
    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .expect("Failed to read log dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(
        !log_files.is_empty(),
        "Should have log files in {}, found: {:?}",
        log_dir.display(),
        log_files
    );

    let marker_file = worktree_path.join("bg_marker.txt");
    assert!(
        marker_file.exists(),
        "Background command should have created bg_marker.txt in {} (logs: {:?})",
        worktree_path.display(),
        log_files
    );

    let content = fs::read_to_string(&marker_file).expect("Failed to read marker file");
    assert!(
        content.contains("Background task done"),
        "Expected background task output, got: {}",
        content
    );
}

/// Test that multiple post-start commands run in parallel with shell integration
#[test]
fn test_bash_post_start_multiple_parallel_commands() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with multiple background commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start-command]
task1 = "sleep 0.5 && echo 'Task 1' > task1.txt"
task2 = "sleep 0.5 && echo 'Task 2' > task2.txt"
"#,
    )
    .expect("Failed to write config");

    repo.commit("Add multiple post-start commands");

    // Pre-approve commands
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{main-worktree}.{branch}"

[projects."test-repo"]
approved-commands = [
    "sleep 0.5 && echo 'Task 1' > task1.txt",
    "sleep 0.5 && echo 'Task 2' > task2.txt",
]
"#,
    )
    .expect("Failed to write test config");

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

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
    thread::sleep(Duration::from_secs(1));

    // Verify both background commands ran
    let worktree_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.parallel-test");
    assert!(
        worktree_path.join("task1.txt").exists(),
        "Task 1 should have completed"
    );
    assert!(
        worktree_path.join("task2.txt").exists(),
        "Task 2 should have completed"
    );
}

/// Test that post-create commands block before shell returns
#[test]
fn test_bash_post_create_blocks() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with blocking command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create-command = "echo 'Setup done' > setup.txt""#,
    )
    .expect("Failed to write config");

    repo.commit("Add post-create command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{main-worktree}.{branch}"

[projects."test-repo"]
approved-commands = ["echo 'Setup done' > setup.txt"]
"#,
    )
    .expect("Failed to write test config");

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

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

    let content = fs::read_to_string(&setup_file).expect("Failed to read setup file");
    assert!(
        content.contains("Setup done"),
        "Expected setup output, got: {}",
        content
    );
}

/// Test fish shell specifically with background tasks
#[test]
fn test_fish_post_start_background() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with background command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"[post-start-command]
fish_bg = "sleep 0.5 && echo 'Fish background done' > fish_bg.txt"
"#,
    )
    .expect("Failed to write config");

    repo.commit("Add fish background command");

    // Pre-approve command
    fs::write(
        repo.test_config_path(),
        r#"worktree-path = "../{main-worktree}.{branch}"

[projects."test-repo"]
approved-commands = ["sleep 0.5 && echo 'Fish background done' > fish_bg.txt"]
"#,
    )
    .expect("Failed to write test config");

    let init_code = generate_init_code(&repo, "fish");
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

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

    // Wait for background command
    thread::sleep(Duration::from_secs(1));

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

    let content = fs::read_to_string(&marker_file).expect("Failed to read marker file");
    assert!(
        content.contains("Fish background done"),
        "Expected fish background output, got: {}",
        content
    );
}
