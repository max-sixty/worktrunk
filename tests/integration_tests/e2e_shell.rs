use crate::common::TestRepo;
use insta_cmd::get_cargo_bin;
use rstest::rstest;
use std::process::Command;

/// Map shell display names to actual binary names
fn get_shell_binary(shell: &str) -> &str {
    match shell {
        "nushell" => "nu",
        "powershell" => "pwsh",
        "oil" => "osh", // oil shell binary is typically named 'osh'
        _ => shell,
    }
}

/// Helper to check if a shell is available on the system
fn is_shell_available(shell: &str) -> bool {
    let binary = get_shell_binary(shell);
    Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

#[rstest]
// Test with bash (POSIX baseline) and fish (different syntax)
// zsh removed - too similar to bash
#[case("bash")]
#[case("fish")]
// Tier 2: Shells requiring extra setup
// TODO: Fix non-core shells - elvish/nushell fail with "Parse error: unexpected rune '\x1b'"
// when parsing ANSI escape codes. Powershell and xonsh fail with syntax errors when they
// encounter emoji characters in the output. Need to investigate if we need to disable colors
// and emojis for these shells or if they need special handling.
// #[cfg_attr(feature = "tier-2-integration-tests", case("elvish"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("nushell"))]
#[cfg_attr(feature = "tier-2-integration-tests", case("oil"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("powershell"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("xonsh"))]
fn test_e2e_switch_changes_directory(#[case] shell: &str) {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

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
        wt switch --create my-feature
        pwd
        "#,
        path_export_syntax(shell, &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, shell, &script);

    // Verify that pwd shows we're in a worktree directory containing "my-feature"
    assert!(
        output.contains("my-feature"),
        "Expected pwd to show my-feature worktree, got: {}",
        output
    );
}

#[rstest]
// Test with bash (POSIX baseline) and fish (different syntax)
// zsh removed - too similar to bash
#[case("bash")]
#[case("fish")]
// Tier 2: Shells requiring extra setup
// TODO: Fix non-core shells - elvish/nushell fail with "Parse error: unexpected rune '\x1b'"
// when parsing ANSI escape codes. Powershell and xonsh fail with syntax errors when they
// encounter emoji characters in the output. Need to investigate if we need to disable colors
// and emojis for these shells or if they need special handling.
// #[cfg_attr(feature = "tier-2-integration-tests", case("elvish"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("nushell"))]
#[cfg_attr(feature = "tier-2-integration-tests", case("oil"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("powershell"))]
// #[cfg_attr(feature = "tier-2-integration-tests", case("xonsh"))]
fn test_e2e_remove_returns_to_main(#[case] shell: &str) {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let init_code = generate_init_code(&repo, shell);
    let repo_path = repo.root_path().to_string_lossy().to_string();
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let script = format!(
        r#"
        {}
        {}
        wt switch --create my-feature
        wt remove
        pwd
        "#,
        path_export_syntax(shell, &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, shell, &script);

    // Verify that pwd shows we're back in the main repo directory
    assert!(
        output.trim().ends_with(&repo_path),
        "Expected pwd to show main repo at {}, got: {}",
        repo_path,
        output
    );
}

#[test]
fn test_bash_e2e_switch_preserves_output() {
    if !is_shell_available("bash") {
        eprintln!("Skipping test: bash not available");
        return;
    }

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");

    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create test-branch 2>&1
        "#,
        get_cargo_bin("wt").parent().unwrap().to_string_lossy(),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Verify that user-facing output is preserved (not just directives)
    assert!(
        output.contains("test-branch") || output.contains("Created") || output.contains("Switched"),
        "Expected informative output, got: {}",
        output
    );
    // Verify directives are NOT shown to user
    assert!(
        !output.contains("__WORKTRUNK_CD__"),
        "Directives should not be visible to user, got: {}",
        output
    );
}

#[test]
fn test_bash_e2e_error_handling() {
    if !is_shell_available("bash") {
        eprintln!("Skipping test: bash not available");
        return;
    }

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");

    // Try to switch to a branch twice (should error on second attempt)
    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create test-feature
        wt switch --create test-feature 2>&1 || echo "ERROR_CAUGHT"
        "#,
        get_cargo_bin("wt").parent().unwrap().to_string_lossy(),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Verify that error is caught and handled
    assert!(
        output.contains("ERROR_CAUGHT")
            || output.contains("already exists")
            || output.contains("error"),
        "Expected error output when switching to same branch twice, got: {}",
        output
    );
}

#[test]
fn test_bash_e2e_switch_to_existing_worktree() {
    if !is_shell_available("bash") {
        eprintln!("Skipping test: bash not available");
        return;
    }

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");

    // Create worktree, move away, then switch back to it (without --create)
    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create existing-branch
        pwd
        cd /tmp
        wt switch existing-branch
        pwd
        "#,
        get_cargo_bin("wt").parent().unwrap().to_string_lossy(),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Should show the existing-branch path twice (once after creation, once after switching back)
    let count = output.matches("existing-branch").count();
    assert!(
        count >= 2,
        "Expected to see existing-branch path at least twice, got: {}",
        output
    );
}

#[test]
fn test_bash_e2e_multiple_switches() {
    if !is_shell_available("bash") {
        eprintln!("Skipping test: bash not available");
        return;
    }

    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let init_code = generate_init_code(&repo, "bash");

    // Test that multiple switches work
    let script = format!(
        r#"
        export PATH="{}:$PATH"
        {}
        wt switch --create test-branch
        pwd
        wt remove
        pwd
        "#,
        get_cargo_bin("wt").parent().unwrap().to_string_lossy(),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Should have switched to test-branch
    assert!(
        output.contains("test-branch"),
        "Expected wt switch to work, got: {}",
        output
    );

    // Should have returned to test-repo (wt remove should work)
    assert!(
        output.contains("test-repo"),
        "Expected wt remove to work, got: {}",
        output
    );
}

#[test]
fn test_bash_e2e_switch_linebreaks() {
    if !is_shell_available("bash") {
        eprintln!("Skipping test: bash not available");
        return;
    }

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Script that switches and immediately echoes a marker to count linebreaks
    let script = format!(
        r#"
        {}
        {}
        wt switch --create test-branch 2>&1
        echo "MARKER"
        "#,
        path_export_syntax("bash", &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);

    // Setup insta settings to normalize paths
    let settings = crate::common::setup_snapshot_settings(&repo);
    settings.bind(|| {
        insta::assert_snapshot!("bash_e2e_switch_linebreaks", output);
    });
}
