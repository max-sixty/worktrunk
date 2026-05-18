#![cfg(all(unix, feature = "shell-integration-tests"))]
//! PTY-based tests for interactive approval prompts
//!
//! These tests verify the approval workflow in a real PTY environment where stdin is a TTY.
//! This allows testing the actual interactive prompt behavior that users experience.
//!
//! Note: These tests are separate from `approval_ui.rs` because they require PTY setup
//! to simulate interactive terminals. The non-PTY tests in `approval_ui.rs` verify the
//! error case (non-TTY environments).

use crate::common::pty::{build_pty_command, exec_cmd_in_pty, exec_cmd_in_pty_prompted};
use crate::common::{TestRepo, add_pty_binary_path_filters, add_pty_filters, repo, wt_bin};
use insta::assert_snapshot;
use rstest::rstest;
use std::path::Path;

/// Execute wt in a PTY, waiting for the approval prompt before sending input.
fn exec_wt_in_pty(
    repo: &TestRepo,
    args: &[&str],
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    exec_wt_in_pty_cwd(repo.root_path(), args, env_vars, input)
}

/// Execute wt in a PTY from a specific directory.
fn exec_wt_in_pty_cwd(
    cwd: &Path,
    args: &[&str],
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    let cmd = build_pty_command(wt_bin().to_str().unwrap(), args, cwd, env_vars, None);
    exec_cmd_in_pty_prompted(cmd, &[input], "[y/N")
}

/// Create insta settings for approval PTY tests.
///
/// Uses shared PTY filters plus test-specific normalizations for config file paths.
fn approval_pty_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = crate::common::setup_snapshot_settings(repo);

    // Add PTY-specific filters (CRLF, ^D, ANSI resets)
    add_pty_filters(&mut settings);

    // Binary path normalization
    add_pty_binary_path_filters(&mut settings);

    // Config paths specific to these tests
    settings.add_filter(r"/var/folders/[^\s]+/test-config\.toml", "[CONFIG]");

    settings
}

/// Decline-path regression: a declined approval prompt prints its own "Commands declined,
/// …" line; the downstream commit/squash phase must not also print
/// "Skipping pre-commit hooks (--no-hooks)" — the user didn't pass that flag.
fn assert_no_spurious_no_hooks(output: &str) {
    assert!(
        !output.contains("--no-hooks"),
        "Declined approval should not produce a spurious '(--no-hooks)' line. Output:\n{output}"
    );
}

/// Get test env vars with shell integration configured.
///
/// This adds SHELL=/bin/zsh to the env vars, which is needed because:
/// - Tests write to .zshrc to simulate configured shell integration
/// - scan_shell_configs() uses $SHELL to determine which config file to check
/// - Without this, CI (which has SHELL=/bin/bash) wouldn't find the .zshrc config
fn test_env_vars_with_shell(repo: &TestRepo) -> Vec<(String, String)> {
    let mut env_vars = repo.test_env_vars();
    env_vars.push(("SHELL".to_string(), "/bin/zsh".to_string()));
    env_vars
}

#[rstest]
fn test_approval_prompt_accept(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"pre-start = "echo 'test command'""#);
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-approve"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_accept", &output);
    });
}

#[rstest]
fn test_approval_prompt_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"pre-start = "echo 'test command'""#);
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-decline"],
        &env_vars,
        "n\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_decline", &output);
    });
}

#[rstest]
fn test_approval_prompt_multiple_commands(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"pre-start = [
    {first = "echo 'First command'"},
    {second = "echo 'Second command'"},
    {third = "echo 'Third command'"},
]
"#,
    );
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-multi"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_multiple_commands", &output);
    });
}

/// TODO: Find a way to test permission errors without skipping when running as root.
/// See test_permission_error_prevents_save in approval_save.rs for details.
#[rstest]
fn test_approval_prompt_permission_error(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(r#"pre-start = "echo 'test command'""#);
    repo.commit("Add config");

    // Configure shell integration before making the approvals directory read-only
    repo.configure_shell_integration();

    // Create a subdirectory for approvals so we can make it read-only
    // without affecting the temp dir root (which holds .zshrc, git config, etc.)
    let approvals_dir = repo.home_path().join("readonly-approvals");
    let approvals_path = approvals_dir.join("approvals.toml");
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(&approvals_dir).unwrap();

        // Make the directory read-only (prevents creating approvals.toml or lock file)
        let mut perms = fs::metadata(&approvals_dir).unwrap().permissions();
        perms.set_mode(0o555); // Read + execute only
        fs::set_permissions(&approvals_dir, perms).unwrap();

        // Test if permissions actually restrict us (skip if running as root)
        let test_file = approvals_dir.join("test_write");
        if fs::write(&test_file, "test").is_ok() {
            // Running as root - restore permissions and skip test
            let _ = fs::remove_file(&test_file);
            let mut perms = fs::metadata(&approvals_dir).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&approvals_dir, perms).unwrap();
            eprintln!("Skipping permission test - running with elevated privileges");
            return;
        }
    }
    let mut env_vars = test_env_vars_with_shell(&repo);
    // Override the approvals path to point to the read-only directory
    env_vars.push((
        "WORKTRUNK_APPROVALS_PATH".to_string(),
        approvals_path.display().to_string(),
    ));
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-permission"],
        &env_vars,
        "y\n",
    );

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when saving approval fails"
    );
    assert!(
        output.contains("Failed to save command approval"),
        "Should show permission error warning"
    );
    assert!(
        output.contains("Approval will be requested again next time"),
        "Should show hint about approval being requested again"
    );
    assert!(
        output.contains("test command"),
        "Command should still execute despite save failure"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_permission_error", &output);
    });
}

#[rstest]
fn test_approval_prompt_named_commands(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"pre-start = [
    {install = "echo 'Installing dependencies...'"},
    {build = "echo 'Building project...'"},
    {test = "echo 'Running tests...'"},
]
"#,
    );
    repo.commit("Add config");

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-named"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);
    assert!(
        output.contains("install") && output.contains("Installing dependencies"),
        "Should show command name 'install' and execute it"
    );
    assert!(
        output.contains("build") && output.contains("Building project"),
        "Should show command name 'build' and execute it"
    );
    assert!(
        output.contains("test") && output.contains("Running tests"),
        "Should show command name 'test' and execute it"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_named_commands", &output);
    });
}

#[rstest]
fn test_approval_prompt_mixed_approved_unapproved_accept(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"pre-start = [
    {first = "echo 'First command'"},
    {second = "echo 'Second command'"},
    {third = "echo 'Third command'"},
]
"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    repo.write_test_approvals(&format!(
        r#"[projects.'{}']
approved-commands = ["echo 'Second command'"]
"#,
        repo.project_id()
    ));

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-mixed-accept"],
        &env_vars,
        "y\n",
    );

    assert_eq!(exit_code, 0);

    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        output.contains("execute") && output.contains("2") && output.contains("command"),
        "Should show 2 unapproved commands in prompt"
    );
    assert!(
        output.contains("First command"),
        "Should execute first command"
    );
    assert!(
        output.contains("Second command"),
        "Should execute pre-approved second command"
    );
    assert!(
        output.contains("Third command"),
        "Should execute third command"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_mixed_approved_unapproved_accept", &output);
    });
}

#[rstest]
fn test_approval_prompt_mixed_approved_unapproved_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"pre-start = [
    {first = "echo 'First command'"},
    {second = "echo 'Second command'"},
    {third = "echo 'Third command'"},
]
"#,
    );
    repo.commit("Add config");

    // Pre-approve the second command
    repo.write_test_approvals(&format!(
        r#"[projects.'{}']
approved-commands = ["echo 'Second command'"]
"#,
        repo.project_id()
    ));

    // Configure shell integration so we get the "Restart shell" hint instead of the prompt
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["switch", "--create", "test-mixed-decline"],
        &env_vars,
        "n\n",
    );

    assert_eq!(
        exit_code, 0,
        "Command should succeed even when declined (worktree still created)"
    );
    // Check that only 2 commands are shown in the prompt (ANSI codes may be in between)
    assert!(
        output.contains("execute") && output.contains("2") && output.contains("command"),
        "Should show only 2 unapproved commands in prompt (not 3)"
    );
    // When declined, ALL commands are skipped (including pre-approved ones)
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    // Commands appear in the prompt, but should not be executed
    // Check for "Running pre-start" which indicates execution
    assert!(
        !output.contains("Running pre-start"),
        "Should NOT execute any commands when declined"
    );
    assert!(
        output.contains("Created branch") && output.contains("and worktree"),
        "Should still create worktree even when commands declined"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_mixed_approved_unapproved_decline", &output);
    });
}

#[rstest]
fn test_approval_prompt_remove_decline(repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // Add pre-remove hook before creating the worktree so the new worktree
    // inherits the committed config — each worktree's `.config/wt.toml` stands
    // alone, with no fallback to the primary worktree's.
    repo.write_project_config(r#"pre-remove = "echo 'pre-remove hook'""#);
    repo.commit("Add pre-remove config");

    // Create a worktree to remove
    let output = repo
        .wt_command()
        .args(["switch", "--create", "to-remove", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "Initial switch should succeed");

    // Configure shell integration
    repo.configure_shell_integration();
    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the approval prompt
    let (output, exit_code) = exec_wt_in_pty(&repo, &["remove", "to-remove"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Remove should succeed even when hooks declined"
    );
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_remove_decline", &output);
    });
}

#[rstest]
fn test_approval_prompt_prune_decline(mut repo: TestRepo) {
    // Remove origin so worktrunk uses the directory name as the project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // A merged worktree carrying a `pre-remove` hook in its `.config/wt.toml`
    repo.write_project_config(r#"pre-remove = "echo 'pre-remove hook'""#);
    repo.commit("Add pre-remove config");
    let wt_path = repo.add_worktree("to-prune");

    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the approval prompt — prune continues without running hooks
    let (output, exit_code) = exec_wt_in_pty(
        &repo,
        &["step", "prune", "--foreground", "--min-age=0s"],
        &env_vars,
        "n\n",
    );

    assert_eq!(
        exit_code, 0,
        "prune should succeed even when hooks declined; output:\n{output}"
    );
    assert!(
        output.contains("Commands declined"),
        "should show 'Commands declined' message; output:\n{output}"
    );
    assert!(
        !wt_path.exists(),
        "the merged worktree should still be pruned"
    );
    approval_pty_settings(&repo).bind(|| {
        assert_snapshot!("approval_prompt_prune_decline", &output);
    });
}

#[rstest]
fn test_approval_prompt_step_commit_decline(mut repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // Add pre-commit hook to project config and commit it
    repo.write_project_config(r#"pre-commit = "echo 'pre-commit hook'""#);
    repo.commit("Add pre-commit config");

    // Create a feature worktree
    let feature_wt = repo.add_worktree("feature-commit");

    // Make dirty changes in the feature worktree
    std::fs::write(feature_wt.join("new-file.txt"), "new content").unwrap();

    // Configure LLM commit generation
    repo.write_test_config(
        r#"
[commit.generation]
command = "cat >/dev/null && echo 'feat: test commit message'"
"#,
    );

    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the pre-commit hook approval prompt
    let (output, exit_code) =
        exec_wt_in_pty_cwd(&feature_wt, &["step", "commit"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Commit should succeed even when hooks declined. Output:\n{output}"
    );
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message. Output:\n{output}"
    );
    assert!(
        output.contains("committing without hooks"),
        "Should indicate commit proceeds without hooks. Output:\n{output}"
    );
    assert_no_spurious_no_hooks(&output);
}

#[rstest]
fn test_approval_prompt_step_squash_decline(mut repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // Add pre-commit hook to project config and commit it
    repo.write_project_config(r#"pre-commit = "echo 'pre-commit hook'""#);
    repo.commit("Add pre-commit config");

    // Create a feature worktree with multiple commits ahead of main
    let feature_wt = repo.add_worktree("feature-squash");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: first change");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "feat: second change");

    // Configure LLM commit generation
    repo.write_test_config(
        r#"
[commit.generation]
command = "cat >/dev/null && echo 'feat: squashed commit message'"
"#,
    );

    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the pre-commit hook approval prompt
    let (output, exit_code) =
        exec_wt_in_pty_cwd(&feature_wt, &["step", "squash"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Squash should succeed even when hooks declined. Output:\n{output}"
    );
    assert!(
        output.contains("Commands declined"),
        "Should show 'Commands declined' message. Output:\n{output}"
    );
    assert!(
        output.contains("squashing without hooks"),
        "Should indicate squash proceeds without hooks. Output:\n{output}"
    );
    assert_no_spurious_no_hooks(&output);
}

/// `wt merge` with a pre-commit hook + dirty worktree: declining the approval
/// prompt should print only "Commands declined, continuing merge", not also
/// "Skipping pre-commit hooks (--no-hooks)" from the commit phase.
#[rstest]
fn test_approval_prompt_merge_decline_no_spurious_skip(mut repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    // Add pre-commit hook to project config and commit it
    repo.write_project_config(r#"pre-commit = "echo 'pre-commit hook'""#);
    repo.commit("Add pre-commit config");

    // Create a feature worktree with a dirty change so the merge runs the commit phase
    let feature_wt = repo.add_worktree("feature-merge-decline");
    std::fs::write(feature_wt.join("new-file.txt"), "new content").unwrap();

    // Configure LLM commit generation so the commit phase has a message to use
    repo.write_test_config(
        r#"
[commit.generation]
command = "cat >/dev/null && echo 'feat: test commit message'"
"#,
    );

    let env_vars = test_env_vars_with_shell(&repo);

    // Decline the approval prompt
    let (output, exit_code) = exec_wt_in_pty_cwd(&feature_wt, &["merge"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Merge should succeed even when hooks declined. Output:\n{output}"
    );
    assert!(
        output.contains("Commands declined, continuing merge"),
        "Should show merge-specific decline message. Output:\n{output}"
    );
    assert_no_spurious_no_hooks(&output);
}

/// Project commit-message append must be approved before the LLM sees it.
///
/// Accept → the fragment is included in the prompt the LLM receives.
#[rstest]
fn test_commit_template_append_approval_accept(mut repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier.
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"
[commit.generation]
template-append = "Use conventional commits"
"#,
    );
    repo.commit("Add project commit append");

    let feature_wt = repo.add_worktree("feature-guidance-accept");
    std::fs::write(feature_wt.join("new-file.txt"), "new content").unwrap();

    // Sink the LLM prompt to a file so we can confirm the guidance reached it.
    let prompt_capture = repo.root_path().join("captured-prompt.txt");
    let prompt_capture_str = prompt_capture.display().to_string();
    repo.write_test_config(&format!(
        r#"
[commit.generation]
command = "tee {prompt_capture_str} > /dev/null && echo 'feat: accept guidance'"
"#
    ));

    let env_vars = test_env_vars_with_shell(&repo);

    let (output, exit_code) =
        exec_wt_in_pty_cwd(&feature_wt, &["step", "commit"], &env_vars, "y\n");

    assert_eq!(
        exit_code, 0,
        "Commit should succeed when guidance approved. Output:\n{output}"
    );
    assert!(
        output.contains("commit-template-append"),
        "Approval prompt should label the append phase. Output:\n{output}"
    );
    assert!(
        output.contains("Use conventional commits"),
        "Approval prompt should display the append text. Output:\n{output}"
    );

    let captured = std::fs::read_to_string(&prompt_capture).expect("captured prompt");
    assert!(
        captured.contains("<project-guidance>") && captured.contains("Use conventional commits"),
        "Append should be sent to the LLM inside <project-guidance>. Captured:\n{captured}"
    );

    // Second run: the fragment is already approved, so it must NOT re-prompt
    // — exercises the `is_command_approved` cache-hit path in
    // `approve_commit_template_append`.
    std::fs::write(feature_wt.join("second.txt"), "more content").unwrap();
    let cmd = build_pty_command(
        wt_bin().to_str().unwrap(),
        &["step", "commit"],
        &feature_wt,
        &env_vars,
        None,
    );
    let (output2, exit2) = exec_cmd_in_pty(cmd, "");
    assert_eq!(
        exit2, 0,
        "Second commit should succeed without re-prompting. Output:\n{output2}"
    );
    assert!(
        !output2.contains("Allow and remember?"),
        "Already-approved append must not prompt again. Output:\n{output2}"
    );
    let captured2 = std::fs::read_to_string(&prompt_capture).expect("captured prompt (2)");
    assert!(
        captured2.contains("<project-guidance>") && captured2.contains("Use conventional commits"),
        "Approved append should still reach the LLM on re-run. Captured:\n{captured2}"
    );
}

/// Declining the append prompt continues without it — the LLM receives the
/// rest of the prompt as if no project append were configured.
#[rstest]
fn test_commit_template_append_approval_decline(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"
[commit.generation]
template-append = "Reference the issue ID"
"#,
    );
    repo.commit("Add project commit append");

    let feature_wt = repo.add_worktree("feature-guidance-decline");
    std::fs::write(feature_wt.join("new-file.txt"), "new content").unwrap();

    let prompt_capture = repo.root_path().join("captured-prompt.txt");
    let prompt_capture_str = prompt_capture.display().to_string();
    repo.write_test_config(&format!(
        r#"
[commit.generation]
command = "tee {prompt_capture_str} > /dev/null && echo 'feat: decline guidance'"
"#
    ));

    let env_vars = test_env_vars_with_shell(&repo);

    let (output, exit_code) =
        exec_wt_in_pty_cwd(&feature_wt, &["step", "commit"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "Commit should succeed even when template declined. Output:\n{output}"
    );
    assert!(
        output.contains("Project commit guidance declined"),
        "Should announce that the append was declined. Output:\n{output}"
    );

    let captured = std::fs::read_to_string(&prompt_capture).expect("captured prompt");
    assert!(
        !captured.contains("Reference the issue ID"),
        "Declined append must not reach the LLM. Captured:\n{captured}"
    );
    assert!(
        !captured.contains("<project-guidance>"),
        "No <project-guidance> block when append declined. Captured:\n{captured}"
    );
}

/// `wt merge` with both project hooks and a commit append bundles the two
/// items into the same approval prompt — the user sees one prompt covering
/// both, not one for hooks and a second for the append mid-merge.
#[rstest]
fn test_merge_bundles_append_into_hook_approval(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.write_project_config(
        r#"
pre-commit = "echo 'pre-commit hook'"

[commit.generation]
template-append = "Reference the related issue"
"#,
    );
    repo.commit("Add project config with hook + guidance");

    let feature_wt = repo.add_worktree("feature-merge-bundle");
    std::fs::write(feature_wt.join("new-file.txt"), "new content").unwrap();

    let prompt_capture = repo.root_path().join("captured-prompt.txt");
    let prompt_capture_str = prompt_capture.display().to_string();
    repo.write_test_config(&format!(
        r#"
[commit.generation]
command = "tee {prompt_capture_str} > /dev/null && echo 'feat: bundled'"
"#
    ));

    let env_vars = test_env_vars_with_shell(&repo);

    let (output, exit_code) = exec_wt_in_pty_cwd(&feature_wt, &["merge"], &env_vars, "y\n");

    assert_eq!(
        exit_code, 0,
        "Merge should succeed when both prompts approved. Output:\n{output}"
    );
    assert!(
        output.contains("pre-commit hook"),
        "Bundled prompt should list the pre-commit hook command. Output:\n{output}"
    );
    assert!(
        output.contains("commit-template-append") && output.contains("Reference the related issue"),
        "Bundled prompt should list the commit append. Output:\n{output}"
    );
    // The downstream commit step must not present a second prompt.
    let prompts = output.matches("Allow and remember?").count();
    assert_eq!(
        prompts, 1,
        "Bundled approval should produce exactly one [y/N] prompt. Saw {prompts}. Output:\n{output}"
    );

    let captured = std::fs::read_to_string(&prompt_capture).expect("captured prompt");
    assert!(
        captured.contains("<project-guidance>") && captured.contains("Reference the related issue"),
        "Approved append should reach the LLM. Captured:\n{captured}"
    );
}

/// `wt config approvals add` accepts the prompt — covers the success branch of
/// `add_approvals` after `approve_command_batch` returns Ok(true).
#[rstest]
fn test_config_approvals_add_accept(repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    // Include a project commit append so `add_approvals` also collects the
    // `commit-template-append` entry (covers that branch in add_approvals).
    repo.write_project_config(
        r#"pre-start = "echo 'test command'"

[commit.generation]
template-append = "Use conventional commits"
"#,
    );
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) =
        exec_wt_in_pty(&repo, &["config", "approvals", "add"], &env_vars, "y\n");

    assert_eq!(exit_code, 0, "add should exit cleanly. Output:\n{output}");
    assert!(
        output.contains("Commands approved"),
        "Should show approval success. Output:\n{output}"
    );
    assert!(
        output.contains("commit-template-append") && output.contains("Use conventional commits"),
        "add should list the project commit append. Output:\n{output}"
    );
}

/// `wt config approvals add` declines the prompt — covers the declined branch
/// of `add_approvals` after `approve_command_batch` returns Ok(false).
#[rstest]
fn test_config_approvals_add_decline(repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.write_project_config(r#"pre-start = "echo 'test command'""#);
    repo.commit("Add config");

    let env_vars = repo.test_env_vars();
    let (output, exit_code) =
        exec_wt_in_pty(&repo, &["config", "approvals", "add"], &env_vars, "n\n");

    assert_eq!(
        exit_code, 0,
        "decline should exit cleanly. Output:\n{output}"
    );
    assert!(
        output.contains("Commands declined"),
        "Should show decline message. Output:\n{output}"
    );
}
