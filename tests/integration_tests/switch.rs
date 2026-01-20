use crate::common::{
    TestRepo, configure_directive_file, directive_file, make_snapshot_cmd,
    make_snapshot_cmd_with_global_flags, repo, repo_with_remote, set_temp_home_env,
    setup_home_snapshot_settings, setup_snapshot_settings, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

// Snapshot helpers

fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    snapshot_switch_impl(test_name, repo, args, false, None, None);
}

fn snapshot_switch_with_directive_file(test_name: &str, repo: &TestRepo, args: &[&str]) {
    snapshot_switch_impl(test_name, repo, args, true, None, None);
}

fn snapshot_switch_from_dir(test_name: &str, repo: &TestRepo, args: &[&str], cwd: &Path) {
    snapshot_switch_impl(test_name, repo, args, false, Some(cwd), None);
}

#[cfg(not(windows))]
fn snapshot_switch_with_shell(test_name: &str, repo: &TestRepo, args: &[&str], shell: &str) {
    snapshot_switch_impl(test_name, repo, args, false, None, Some(shell));
}

fn snapshot_switch_impl(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    with_directive_file: bool,
    cwd: Option<&Path>,
    shell: Option<&str>,
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        // Directive file guard - declared at closure scope to live through command execution
        let maybe_directive = if with_directive_file {
            Some(directive_file())
        } else {
            None
        };

        let mut cmd = make_snapshot_cmd(repo, "switch", args, cwd);
        if let Some((ref directive_path, ref _guard)) = maybe_directive {
            configure_directive_file(&mut cmd, directive_path);
        }
        if let Some(shell_path) = shell {
            cmd.env("SHELL", shell_path);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}
// Basic switch tests
#[rstest]
fn test_switch_create_new_branch(repo: TestRepo) {
    snapshot_switch("switch_create_new", &repo, &["--create", "feature-x"]);
}

/// Test that delayed streaming shows progress message when threshold is 0.
/// This exercises the streaming code path that normally only triggers for slow operations.
#[rstest]
fn test_switch_create_shows_progress_when_forced(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "feature-progress"], None);
        // Force immediate streaming by setting threshold to 0
        cmd.env("WT_TEST_DELAYED_STREAM_MS", "0");
        assert_cmd_snapshot!("switch_create_with_progress", cmd);
    });
}

#[rstest]
fn test_switch_create_existing_branch_error(mut repo: TestRepo) {
    // Create a branch first
    repo.add_worktree("feature-y");

    // Try to create it again - should error
    snapshot_switch(
        "switch_create_existing_error",
        &repo,
        &["--create", "feature-y"],
    );
}

#[rstest]
fn test_switch_create_with_remote_branch_only(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a branch on the remote only (no local branch)
    repo.run_git(&["branch", "remote-feature"]);
    repo.run_git(&["push", "origin", "remote-feature"]);

    // Delete the local branch
    repo.run_git(&["branch", "-D", "remote-feature"]);

    // Now we have origin/remote-feature but no local remote-feature
    // This should succeed with --create (previously would fail)
    snapshot_switch(
        "switch_create_remote_only",
        &repo,
        &["--create", "remote-feature"],
    );
}

/// Git's DWIM creates local tracking branch from remote when no local branch exists.
/// Should report "Created branch X (tracking remote)" since DWIM actually created the branch.
#[rstest]
fn test_switch_dwim_from_remote(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a branch on the remote only (no local branch)
    repo.run_git(&["branch", "dwim-feature"]);
    repo.run_git(&["push", "origin", "dwim-feature"]);
    repo.run_git(&["branch", "-D", "dwim-feature"]);

    // Now we have origin/dwim-feature but no local dwim-feature
    // DWIM should create local branch from remote
    snapshot_switch("switch_dwim_from_remote", &repo, &["dwim-feature"]);
}

/// When creating a new branch from a remote tracking branch (e.g., origin/main),
/// the new branch should NOT track the remote base branch.
/// This prevents accidental `git push` to the base branch (e.g., pushing to main).
/// This is the bug fix for GitHub issue #713.
#[rstest]
fn test_switch_create_from_remote_base_no_upstream(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a new branch with --base pointing to a remote tracking branch
    let output = repo
        .wt_command()
        .args(["switch", "--create", "my-feature", "--base=origin/main"])
        .output()
        .unwrap();
    assert!(output.status.success(), "switch should succeed");

    // Verify the branch was created
    let branch_output = repo.git_output(&["branch", "--list", "my-feature"]);
    assert!(
        branch_output.contains("my-feature"),
        "branch should be created"
    );

    // Verify the branch does NOT have an upstream (no tracking)
    // Using rev-parse to check for upstream - should fail for untracked branches
    let upstream_check = repo
        .git_command()
        .args(["rev-parse", "--abbrev-ref", "my-feature@{upstream}"])
        .output()
        .unwrap();

    assert!(
        !upstream_check.status.success(),
        "branch should NOT have upstream tracking (to prevent accidental push to origin/main)"
    );
}

/// When local branch already exists and tracks a remote, should report
/// "Created worktree for X" NOT "Created branch X (tracking remote)".
/// This is the bug fix for GitHub issue #656.
#[rstest]
fn test_switch_existing_local_branch_with_upstream(#[from(repo_with_remote)] repo: TestRepo) {
    // Create local branch tracking remote
    repo.run_git(&["checkout", "-b", "tracked-feature"]);
    repo.run_git(&["commit", "--allow-empty", "-m", "feature commit"]);
    repo.run_git(&["push", "-u", "origin", "tracked-feature"]);
    repo.run_git(&["checkout", "main"]);

    // Switch to the existing local branch (should NOT say "Created branch")
    snapshot_switch(
        "switch_existing_local_with_upstream",
        &repo,
        &["tracked-feature"],
    );
}

#[rstest]
fn test_switch_existing_branch(mut repo: TestRepo) {
    repo.add_worktree("feature-z");

    // Switch to it (should find existing worktree)
    snapshot_switch("switch_existing_branch", &repo, &["feature-z"]);
}

///
/// When shell integration is configured in user's rc files (e.g., .zshrc) but the user
/// runs `wt` binary directly (not through the shell wrapper), show a warning that explains
/// the actual situation: shell IS configured, but cd can't happen because we're not
/// running through the shell function.
///
/// Since tests run via `cargo test`, argv[0] contains a path (`target/debug/wt`), which
/// triggers the "explicit path" code path. The warning explains that shell integration
/// won't intercept explicit paths.
///
/// Skipped on Windows: the binary is `wt.exe` so a different (more targeted) warning is
/// shown ("use wt without .exe"). Windows-specific behavior is tested in unit tests.
#[rstest]
#[cfg(not(windows))]
fn test_switch_existing_with_shell_integration_configured(mut repo: TestRepo) {
    use std::fs;

    // Create a worktree first
    repo.add_worktree("shell-configured");

    // Simulate shell integration configured in user's shell rc files
    // (repo.home_path() is automatically set as HOME by configure_wt_cmd)
    let zshrc_path = repo.home_path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing user zsh config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    // Switch to existing worktree - should show warning about binary invoked directly
    // (different from "no shell integration" warning when shell is not configured at all)
    // Note: Must set SHELL=/bin/zsh so scan_shell_configs() looks for .zshrc
    snapshot_switch_with_shell(
        "switch_existing_with_shell_configured",
        &repo,
        &["shell-configured"],
        "/bin/zsh",
    );
}

///
/// When git runs a subcommand, it sets `GIT_EXEC_PATH` in the environment.
/// Shell integration cannot work in this case because cd directives cannot
/// propagate through git's subprocess to the parent shell.
#[rstest]
fn test_switch_existing_as_git_subcommand(mut repo: TestRepo) {
    // Create a worktree first
    repo.add_worktree("git-subcommand-test");

    // Switch with GIT_EXEC_PATH set (simulating `git wt switch ...`)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["git-subcommand-test"], None);
        cmd.env("GIT_EXEC_PATH", "/usr/lib/git-core");
        assert_cmd_snapshot!("switch_as_git_subcommand", cmd);
    });
}

#[rstest]
fn test_switch_with_base_branch(repo: TestRepo) {
    repo.commit("Initial commit on main");

    snapshot_switch(
        "switch_with_base",
        &repo,
        &["--create", "--base", "main", "feature-with-base"],
    );
}

#[rstest]
fn test_switch_base_without_create_warning(repo: TestRepo) {
    snapshot_switch(
        "switch_base_without_create",
        &repo,
        &["--base", "main", "main"],
    );
}

#[rstest]
fn test_switch_create_with_invalid_base(repo: TestRepo) {
    // Issue #562: Error message should identify the invalid base branch,
    // not the target branch being created
    snapshot_switch(
        "switch_create_invalid_base",
        &repo,
        &["--create", "new-feature", "--base", "nonexistent-base"],
    );
}

#[rstest]
fn test_switch_base_accepts_commitish(repo: TestRepo) {
    // Issue #630: --base should accept any commit-ish, not just branch names
    // Test HEAD as base (common use case: branch from current HEAD)
    repo.commit("Initial commit on main");
    snapshot_switch(
        "switch_base_commitish_head",
        &repo,
        &["--create", "feature-from-head", "--base", "HEAD"],
    );
}

// Internal mode tests
#[rstest]
fn test_switch_internal_mode(repo: TestRepo) {
    snapshot_switch_with_directive_file(
        "switch_internal_mode",
        &repo,
        &["--create", "internal-test"],
    );
}

#[rstest]
fn test_switch_existing_worktree_internal(mut repo: TestRepo) {
    repo.add_worktree("existing-wt");

    snapshot_switch_with_directive_file("switch_existing_internal", &repo, &["existing-wt"]);
}

#[rstest]
fn test_switch_internal_with_execute(repo: TestRepo) {
    let execute_cmd = "echo 'line1'\necho 'line2'";

    snapshot_switch_with_directive_file(
        "switch_internal_with_execute",
        &repo,
        &["--create", "exec-internal", "--execute", execute_cmd],
    );
}
// Error tests
#[rstest]
fn test_switch_error_missing_worktree_directory(mut repo: TestRepo) {
    let wt_path = repo.add_worktree("missing-wt");

    // Remove the worktree directory (but leave it registered in git)
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Try to switch to the missing worktree (should fail)
    snapshot_switch("switch_error_missing_directory", &repo, &["missing-wt"]);
}

/// Test error when target path is registered to a worktree whose directory is missing.
///
/// Scenario: branch "feature/collision" has a worktree at "repo.feature-collision",
/// but the directory was deleted. Trying to create "feature-collision" (which maps
/// to the same path) should error about the missing worktree, not try to overwrite.
#[rstest]
fn test_switch_error_path_occupied_by_missing_worktree(mut repo: TestRepo) {
    // Create a worktree for "feature/collision" -> path "repo.feature-collision"
    let wt_path = repo.add_worktree("feature/collision");

    // Delete the worktree directory (but leave it registered in git)
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Try to create "feature-collision" which maps to the same path
    // Should fail because the path is registered to a missing worktree
    snapshot_switch(
        "switch_error_path_occupied_missing",
        &repo,
        &["--create", "feature-collision"],
    );
}

#[rstest]
fn test_switch_error_path_occupied(repo: TestRepo) {
    // Calculate where the worktree would be created
    // Default path pattern is {repo_name}.{branch}
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.occupied-branch", repo_name));

    // Create a non-worktree directory at that path
    std::fs::create_dir_all(&expected_path).unwrap();
    std::fs::write(expected_path.join("some_file.txt"), "occupant content").unwrap();

    // Try to create a worktree with a branch that would use that path
    // Should fail with worktree_path_occupied error
    snapshot_switch(
        "switch_error_path_occupied",
        &repo,
        &["--create", "occupied-branch"],
    );

    // Cleanup
    std::fs::remove_dir_all(&expected_path).ok();
}
// Execute flag tests
#[rstest]
fn test_switch_execute_success(repo: TestRepo) {
    snapshot_switch(
        "switch_execute_success",
        &repo,
        &["--create", "exec-test", "--execute", "echo 'test output'"],
    );
}

#[rstest]
fn test_switch_execute_creates_file(repo: TestRepo) {
    let create_file_cmd = "echo 'test content' > test.txt";

    snapshot_switch(
        "switch_execute_creates_file",
        &repo,
        &["--create", "file-test", "--execute", create_file_cmd],
    );
}

#[rstest]
fn test_switch_execute_failure(repo: TestRepo) {
    snapshot_switch(
        "switch_execute_failure",
        &repo,
        &["--create", "fail-test", "--execute", "exit 1"],
    );
}

#[rstest]
fn test_switch_execute_with_existing_worktree(mut repo: TestRepo) {
    repo.add_worktree("existing-exec");

    let create_file_cmd = "echo 'existing worktree' > existing.txt";

    snapshot_switch(
        "switch_execute_existing",
        &repo,
        &["existing-exec", "--execute", create_file_cmd],
    );
}

#[rstest]
fn test_switch_execute_multiline(repo: TestRepo) {
    let multiline_cmd = "echo 'line1'\necho 'line2'\necho 'line3'";

    snapshot_switch(
        "switch_execute_multiline",
        &repo,
        &["--create", "multiline-test", "--execute", multiline_cmd],
    );
}

// Execute template expansion tests
#[rstest]
fn test_switch_execute_template_branch(repo: TestRepo) {
    // Test that {{ branch }} is expanded in --execute command
    snapshot_switch(
        "switch_execute_template_branch",
        &repo,
        &[
            "--create",
            "template-test",
            "--execute",
            "echo 'branch={{ branch }}'",
        ],
    );
}

#[rstest]
fn test_switch_execute_template_base(repo: TestRepo) {
    // Test that {{ base }} is available when creating with --create
    snapshot_switch(
        "switch_execute_template_base",
        &repo,
        &[
            "--create",
            "from-main",
            "--base",
            "main",
            "--execute",
            "echo 'base={{ base }}'",
        ],
    );
}

#[rstest]
fn test_switch_execute_template_base_without_create(mut repo: TestRepo) {
    // Test that {{ base }} errors when switching to existing worktree (no --create)
    // The `base` variable is only available during branch creation
    repo.add_worktree("existing");
    snapshot_switch(
        "switch_execute_template_base_without_create",
        &repo,
        &["existing", "--execute", "echo 'base={{ base }}'"],
    );
}

#[rstest]
fn test_switch_execute_template_with_filter(repo: TestRepo) {
    // Test that filters work ({{ branch | sanitize }})
    snapshot_switch(
        "switch_execute_template_with_filter",
        &repo,
        &[
            "--create",
            "feature/with-slash",
            "--execute",
            "echo 'sanitized={{ branch | sanitize }}'",
        ],
    );
}

#[rstest]
fn test_switch_execute_template_shell_escape(repo: TestRepo) {
    // Test that shell metacharacters in branch names are escaped
    // Without escaping, this would execute `id` as a separate command
    snapshot_switch(
        "switch_execute_template_shell_escape",
        &repo,
        &["--create", "feat;id", "--execute", "echo {{ branch }}"],
    );
}

#[rstest]
fn test_switch_execute_template_worktree_path(repo: TestRepo) {
    // Test that {{ worktree_path }} is expanded
    snapshot_switch(
        "switch_execute_template_worktree_path",
        &repo,
        &[
            "--create",
            "path-test",
            "--execute",
            "echo 'path={{ worktree_path }}'",
        ],
    );
}

#[rstest]
fn test_switch_execute_template_in_args(repo: TestRepo) {
    // Test that templates are expanded in trailing args (after --)
    snapshot_switch(
        "switch_execute_template_in_args",
        &repo,
        &[
            "--create",
            "args-test",
            "--execute",
            "echo",
            "--",
            "branch={{ branch }}",
            "repo={{ repo }}",
        ],
    );
}

#[rstest]
fn test_switch_execute_template_error(repo: TestRepo) {
    // Test that invalid templates are rejected with a clear error
    snapshot_switch(
        "switch_execute_template_error",
        &repo,
        &["--create", "error-test", "--execute", "echo {{ unclosed"],
    );
}

#[rstest]
fn test_switch_execute_arg_template_error(repo: TestRepo) {
    // Test that invalid templates in trailing args (after --) are rejected
    snapshot_switch(
        "switch_execute_arg_template_error",
        &repo,
        &[
            "--create",
            "arg-error-test",
            "--execute",
            "echo",
            "--",
            "valid={{ branch }}",
            "invalid={{ unclosed",
        ],
    );
}

// Verbose mode tests
#[rstest]
fn test_switch_execute_verbose_template_expansion(repo: TestRepo) {
    // Test that -v shows template expansion details
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(
            &repo,
            "switch",
            &[
                "--create",
                "verbose-test",
                "--execute",
                "echo 'branch={{ branch }}'",
            ],
            None,
            &["-v"],
        );
        assert_cmd_snapshot!("switch_execute_verbose_template", cmd);
    });
}

#[rstest]
fn test_switch_execute_verbose_multiline_template(repo: TestRepo) {
    // Test that -v shows multiline template expansion with proper formatting
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        // Multiline template with conditional
        let multiline_template = r#"{% if branch %}
echo 'branch={{ branch }}'
echo 'repo={{ repo }}'
{% endif %}"#;

        let mut cmd = make_snapshot_cmd_with_global_flags(
            &repo,
            "switch",
            &[
                "--create",
                "multiline-test",
                "--execute",
                multiline_template,
            ],
            None,
            &["-v"],
        );
        assert_cmd_snapshot!("switch_execute_verbose_multiline_template", cmd);
    });
}

// --no-verify flag tests
#[rstest]
fn test_switch_no_config_commands_execute_still_runs(repo: TestRepo) {
    snapshot_switch(
        "switch_no_hooks_execute_still_runs",
        &repo,
        &[
            "--create",
            "no-hooks-test",
            "--execute",
            "echo 'execute command runs'",
            "--no-verify",
        ],
    );
}

#[rstest]
fn test_switch_no_config_commands_skips_post_start_commands(repo: TestRepo) {
    use std::fs;

    // Create project config with a command that would create a file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    let create_file_cmd = "echo 'marker' > marker.txt";

    fs::write(
        config_dir.join("wt.toml"),
        format!(r#"post-starts = ["{}"]"#, create_file_cmd),
    )
    .unwrap();

    repo.commit("Add config");

    // Pre-approve the command (repo.home_path() is automatically set as HOME)
    let user_config_dir = repo.home_path().join(".config/worktrunk");
    fs::create_dir_all(&user_config_dir).unwrap();
    fs::write(
        user_config_dir.join("config.toml"),
        format!(
            r#"worktree-path = "../{{{{ repo }}}}.{{{{ branch }}}}"

[projects."main"]
approved-commands = ["{}"]
"#,
            create_file_cmd
        ),
    )
    .unwrap();

    // With --no-verify, the post-start command should be skipped
    snapshot_switch(
        "switch_no_hooks_skips_post_start",
        &repo,
        &["--create", "no-post-start", "--no-verify"],
    );
}

#[rstest]
fn test_switch_no_config_commands_with_existing_worktree(mut repo: TestRepo) {
    repo.add_worktree("existing-no-hooks");

    // With --no-verify, the --execute command should still run
    snapshot_switch(
        "switch_no_hooks_existing",
        &repo,
        &[
            "existing-no-hooks",
            "--execute",
            "echo 'execute still runs'",
            "--no-verify",
        ],
    );
}

#[rstest]
fn test_switch_no_config_commands_with_yes(repo: TestRepo) {
    use std::fs;

    // Create project config with a command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-starts = ["echo 'test'"]"#,
    )
    .unwrap();

    repo.commit("Add config");

    // With --no-verify, even --yes shouldn't execute config commands
    // (HOME is automatically set to repo.home_path() by configure_wt_cmd)
    snapshot_switch(
        "switch_no_hooks_with_yes",
        &repo,
        &["--create", "yes-no-hooks", "--yes", "--no-verify"],
    );
}
// Branch inference and special branch tests
#[rstest]
fn test_switch_create_no_remote(repo: TestRepo) {
    // Deliberately NOT calling setup_remote to test local branch inference
    // Create a branch without specifying base - should infer default branch locally
    snapshot_switch("switch_create_no_remote", &repo, &["--create", "feature"]);
}

#[rstest]
fn test_switch_primary_on_different_branch(mut repo: TestRepo) {
    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    // Create a feature worktree using the default branch (main)
    // This should work fine even though primary is on develop
    snapshot_switch(
        "switch_primary_on_different_branch",
        &repo,
        &["--create", "feature-from-main"],
    );

    // Also test switching to an existing branch
    repo.add_worktree("existing-branch");
    snapshot_switch(
        "switch_to_existing_primary_on_different_branch",
        &repo,
        &["existing-branch"],
    );
}

#[rstest]
fn test_switch_previous_branch_no_history(repo: TestRepo) {
    // No checkout history, so wt switch - should fail with helpful error
    snapshot_switch("switch_previous_branch_no_history", &repo, &["-"]);
}

#[rstest]
fn test_switch_main_branch(repo: TestRepo) {
    // Create a feature branch (use unique name to avoid fixture conflicts)
    repo.run_git(&["branch", "test-feat-x"]);

    // Switch to test-feat-x first
    snapshot_switch("switch_main_branch_to_feature", &repo, &["test-feat-x"]);

    // Now wt switch ^ should resolve to main
    snapshot_switch("switch_main_branch", &repo, &["^"]);
}

#[rstest]
fn test_create_with_base_main(repo: TestRepo) {
    // Create new branch from main using ^
    snapshot_switch(
        "create_with_base_main",
        &repo,
        &["--create", "new-feature", "--base", "^"],
    );
}

#[rstest]
fn test_switch_no_warning_when_branch_matches(mut repo: TestRepo) {
    // Create a worktree for "feature" branch (normal case)
    repo.add_worktree("feature");

    // Switch to feature with shell integration - should NOT show any warning
    snapshot_switch_with_directive_file(
        "switch_no_warning_when_branch_matches",
        &repo,
        &["feature"],
    );
}

#[rstest]
fn test_switch_branch_worktree_mismatch_shows_hint(repo: TestRepo) {
    // Create a worktree at a non-standard path (sibling to repo, not following template)
    let wrong_path = repo.root_path().parent().unwrap().join("wrong-path");
    repo.run_git(&[
        "worktree",
        "add",
        wrong_path.to_str().unwrap(),
        "-b",
        "feature",
    ]);

    // Switch to feature - should show hint about branch-worktree mismatch
    snapshot_switch_with_directive_file(
        "switch_branch_worktree_mismatch_shows_hint",
        &repo,
        &["feature"],
    );
}

///
/// When shell integration is not active, the branch-worktree mismatch warning should appear
/// alongside the "cannot change directory" warning.
#[rstest]
fn test_switch_worktree_mismatch_no_shell_integration(repo: TestRepo) {
    // Create a worktree at a non-standard path
    let wrong_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("wrong-path-no-shell");
    repo.run_git(&[
        "worktree",
        "add",
        wrong_path.to_str().unwrap(),
        "-b",
        "feature-mismatch",
    ]);

    // Switch without directive file (no shell integration) - should show both warnings
    snapshot_switch(
        "switch_branch_worktree_mismatch_no_shell",
        &repo,
        &["feature-mismatch"],
    );
}

///
/// When already in a worktree whose path doesn't match the branch name,
/// switching to that branch should show the branch-worktree mismatch warning.
#[rstest]
fn test_switch_already_at_with_branch_worktree_mismatch(repo: TestRepo) {
    // Create a worktree at a non-standard path
    let wrong_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("wrong-path-already");
    repo.run_git(&[
        "worktree",
        "add",
        wrong_path.to_str().unwrap(),
        "-b",
        "feature-already",
    ]);

    // Switch from within the worktree with branch-worktree mismatch (AlreadyAt case)
    snapshot_switch_from_dir(
        "switch_already_at_branch_worktree_mismatch",
        &repo,
        &["feature-already"],
        &wrong_path,
    );
}

///
/// With branch-first lookup, if a worktree was created for "feature" but then switched to
/// "bugfix", `wt switch feature` can't find it (since it looks by branch name). When it
/// tries to create a new worktree, it fails because the path exists. The hint shows what
/// branch currently occupies the path.
#[rstest]
fn test_switch_error_path_occupied_different_branch(repo: TestRepo) {
    // Create a worktree for "feature" branch at expected path
    let feature_path = repo.root_path().parent().unwrap().join("repo.feature");
    repo.run_git(&[
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature",
    ]);

    // Switch that worktree to a different branch "bugfix"
    repo.run_git_in(&feature_path, &["switch", "-c", "bugfix"]);

    // Switch to feature - should error since path is occupied by bugfix worktree
    snapshot_switch_with_directive_file(
        "switch_error_path_occupied_different_branch",
        &repo,
        &["feature"],
    );
}

#[rstest]
fn test_switch_error_path_occupied_detached(repo: TestRepo) {
    // Create a worktree for "feature" branch at expected path
    let feature_path = repo.root_path().parent().unwrap().join("repo.feature");
    repo.run_git(&[
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature",
    ]);

    // Get the HEAD commit and detach
    let output = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    repo.run_git_in(&feature_path, &["checkout", "--detach", &commit]);

    // Switch to feature - should error since path is occupied by detached worktree
    snapshot_switch_with_directive_file("switch_error_path_occupied_detached", &repo, &["feature"]);
}

///
/// When the main worktree (repo root) has been switched to a feature branch via
/// `git checkout feature`, `wt switch main` should error with a helpful message
/// explaining how to get there. This matches GitHub issue #327.
#[rstest]
fn test_switch_main_worktree_on_different_branch(repo: TestRepo) {
    // Switch the main worktree to a different branch
    repo.run_git(&["checkout", "-b", "feature"]);

    // Now try to switch to main - should error since main worktree is on different branch
    snapshot_switch_with_directive_file(
        "switch_main_worktree_on_different_branch",
        &repo,
        &["main"],
    );
}

///
/// This reproduces GitHub issue #327: user is in a feature worktree, main worktree has been
/// switched to a different branch, and user runs `wt switch <default-branch>`.
#[rstest]
fn test_switch_default_branch_from_feature_worktree(mut repo: TestRepo) {
    // Create a feature worktree to work from
    let feature_a_path = repo.add_worktree("feature-a");

    // Switch main worktree to a different branch (simulates user running git checkout there)
    repo.run_git(&["checkout", "-b", "feature-rpa"]);

    // From feature-a worktree, try to switch to main (default branch)
    // This should error because main worktree is now on feature-rpa
    snapshot_switch_from_dir(
        "switch_default_branch_from_feature_worktree",
        &repo,
        &["main"],
        &feature_a_path,
    );
}

// Execute tests with directive file
/// The shell wrapper sources this file and propagates the exit code.
#[rstest]
fn test_switch_internal_execute_exit_code(repo: TestRepo) {
    // wt succeeds (exit 0), but shell script contains "exit 42"
    // Shell wrapper will eval and return 42
    snapshot_switch_with_directive_file(
        "switch_internal_execute_exit_code",
        &repo,
        &["--create", "exit-code-test", "--execute", "exit 42"],
    );
}

/// When wt succeeds but the execute script would fail, wt still exits 0.
/// The shell wrapper handles the execute command's exit code.
#[rstest]
fn test_switch_internal_execute_with_output_before_exit(repo: TestRepo) {
    // Execute command outputs then exits with code
    let cmd = "echo 'doing work'\nexit 7";

    snapshot_switch_with_directive_file(
        "switch_internal_execute_output_then_exit",
        &repo,
        &["--create", "output-exit-test", "--execute", cmd],
    );
}
// History and ping-pong tests
///
/// Bug scenario: If user changes worktrees without using `wt switch` (e.g., cd directly),
/// history becomes stale. The fix ensures we always use the actual current branch
/// when recording new history, not any previously stored value.
#[rstest]
fn test_switch_previous_with_stale_history(repo: TestRepo) {
    // Create branches with worktrees
    for branch in ["branch-a", "branch-b", "branch-c"] {
        repo.run_git(&["branch", branch]);
    }

    // Switch to branch-a, then branch-b to establish history
    snapshot_switch("switch_stale_history_to_a", &repo, &["branch-a"]);
    snapshot_switch("switch_stale_history_to_b", &repo, &["branch-b"]);

    // Now manually set history to simulate user changing worktrees without wt switch.
    // History stores just the previous branch (branch-a from the earlier switches).
    // If user manually cd'd to branch-c's worktree, history would still say branch-a.
    repo.run_git(&["config", "worktrunk.history", "branch-a"]);

    // Run wt switch - from branch-b's worktree.
    // Should go to branch-a (what history says), and record actual current branch as new previous.
    snapshot_switch("switch_stale_history_first_dash", &repo, &["-"]);

    // Run wt switch - again.
    // Should go back to wherever we actually were (recorded as new previous in step above)
    snapshot_switch("switch_stale_history_second_dash", &repo, &["-"]);
}

///
/// This simulates real usage with shell integration, where each `wt switch` actually
/// changes the working directory before the next command runs.
#[rstest]
fn test_switch_ping_pong_realistic(repo: TestRepo) {
    // Create ping-pong branch (unique name to avoid fixture conflicts)
    repo.run_git(&["branch", "ping-pong"]);

    // Step 1: From main worktree, switch to ping-pong (creates worktree)
    // History: current=ping-pong, previous=main
    snapshot_switch_from_dir(
        "ping_pong_1_main_to_feature",
        &repo,
        &["ping-pong"],
        repo.root_path(),
    );

    // Calculate ping-pong worktree path
    let ping_pong_path = repo.root_path().parent().unwrap().join(format!(
        "{}.ping-pong",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));

    // Step 2: From ping-pong worktree, switch back to main
    // History: current=main, previous=ping-pong
    snapshot_switch_from_dir(
        "ping_pong_2_feature_to_main",
        &repo,
        &["main"],
        &ping_pong_path,
    );

    // Step 3: From main worktree, wt switch - should go to ping-pong
    // History: current=ping-pong, previous=main
    snapshot_switch_from_dir(
        "ping_pong_3_dash_to_feature",
        &repo,
        &["-"],
        repo.root_path(),
    );

    // Step 4: From ping-pong worktree, wt switch - should go back to main
    // History: current=main, previous=ping-pong
    snapshot_switch_from_dir("ping_pong_4_dash_to_main", &repo, &["-"], &ping_pong_path);

    // Step 5: From main worktree, wt switch - should go to ping-pong again (ping-pong!)
    // History: current=ping-pong, previous=main
    snapshot_switch_from_dir(
        "ping_pong_5_dash_to_feature_again",
        &repo,
        &["-"],
        repo.root_path(),
    );
}

#[rstest]
fn test_switch_missing_argument_shows_hints(repo: TestRepo) {
    // Run switch with no arguments - should show clap error plus hints
    snapshot_switch("switch_missing_argument_hints", &repo, &[]);
}

///
/// This verifies the fix for non-Unix platforms where stdin was incorrectly
/// set to Stdio::null() instead of Stdio::inherit(), breaking interactive
/// programs like `vim`, `python -i`, or `claude`.
///
/// The test pipes input to `wt switch --execute "cat"` and verifies the
/// cat command receives and outputs that input, proving stdin was inherited.
#[rstest]
fn test_switch_execute_stdin_inheritance(repo: TestRepo) {
    use std::io::Write;
    use std::process::Stdio;

    let test_input = "stdin_inheritance_test_content\n";

    let mut cmd = repo.wt_command();
    cmd.args(["switch", "--create", "stdin-test", "--execute", "cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn wt");

    // Write test input to stdin and close it to signal EOF
    {
        let stdin = child.stdin.as_mut().expect("failed to get stdin");
        stdin
            .write_all(test_input.as_bytes())
            .expect("failed to write to stdin");
    }

    let output = child.wait_with_output().expect("failed to wait for child");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The cat command should have received our input via inherited stdin
    // and echoed it to stdout
    assert!(
        stdout.contains("stdin_inheritance_test_content"),
        "Expected cat to receive piped stdin. Got stdout: {}\nstderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

// Error context tests

#[rstest]
fn test_switch_outside_git_repo(temp_home: TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();

    // Run wt switch --create outside a git repo - should show "Failed to switch worktree" context
    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("switch")
            .arg("--create")
            .arg("feature")
            .current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

// Clobber flag path backup tests

#[rstest]
fn test_switch_clobber_backs_up_stale_directory(repo: TestRepo) {
    // Calculate where the worktree would be created
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.clobber-dir-test", repo_name));

    // Create a stale directory at that path (not a worktree)
    std::fs::create_dir_all(&expected_path).unwrap();
    std::fs::write(expected_path.join("stale_file.txt"), "stale content").unwrap();

    // With --clobber, should move the directory to .bak and create the worktree
    snapshot_switch(
        "switch_clobber_removes_stale_dir",
        &repo,
        &["--create", "--clobber", "clobber-dir-test"],
    );

    // Verify the worktree was created
    assert!(expected_path.exists());
    assert!(expected_path.is_dir());

    // Verify the backup was created (TEST_EPOCH=1735776000 -> 2025-01-02 00:00:00 UTC)
    let backup_path = repo.root_path().parent().unwrap().join(format!(
        "{}.clobber-dir-test.bak.20250102-000000",
        repo_name
    ));
    assert!(
        backup_path.exists(),
        "Backup should exist at {:?}",
        backup_path
    );
    assert!(backup_path.is_dir());

    // Verify stale content is preserved in backup
    let stale_file = backup_path.join("stale_file.txt");
    assert!(stale_file.exists(), "Stale file should be in backup");
    assert_eq!(
        std::fs::read_to_string(&stale_file).unwrap(),
        "stale content"
    );
}

#[rstest]
fn test_switch_clobber_backs_up_stale_file(repo: TestRepo) {
    // Calculate where the worktree would be created
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.clobber-file-test", repo_name));

    // Create a file (not directory) at that path
    std::fs::write(&expected_path, "stale file content").unwrap();

    // With --clobber, should move the file to .bak and create the worktree
    snapshot_switch(
        "switch_clobber_removes_stale_file",
        &repo,
        &["--create", "--clobber", "clobber-file-test"],
    );

    // Verify the worktree was created (should be a directory now)
    assert!(expected_path.is_dir());

    // Verify the backup was created (TEST_EPOCH=1735776000 -> 2025-01-02 00:00:00 UTC)
    let backup_path = repo.root_path().parent().unwrap().join(format!(
        "{}.clobber-file-test.bak.20250102-000000",
        repo_name
    ));
    assert!(
        backup_path.exists(),
        "Backup should exist at {:?}",
        backup_path
    );
    assert!(backup_path.is_file());
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        "stale file content"
    );
}

#[rstest]
fn test_switch_clobber_error_backup_exists(repo: TestRepo) {
    // Calculate where the worktree would be created
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.clobber-backup-exists", repo_name));

    // Create a stale directory at the target path
    std::fs::create_dir_all(&expected_path).unwrap();

    // Also create the backup path that would be generated
    // TEST_EPOCH=1735776000 -> 2025-01-02 00:00:00 UTC
    let backup_path = repo.root_path().parent().unwrap().join(format!(
        "{}.clobber-backup-exists.bak.20250102-000000",
        repo_name
    ));
    std::fs::create_dir_all(&backup_path).unwrap();

    // With --clobber, should error because backup path exists
    snapshot_switch(
        "switch_clobber_error_backup_exists",
        &repo,
        &["--create", "--clobber", "clobber-backup-exists"],
    );

    // Both paths should still exist (nothing was moved)
    assert!(expected_path.exists());
    assert!(backup_path.exists());
}

///
/// When the user runs `wt` directly (not through shell wrapper), their shell won't
/// cd to the worktree directory. Hooks should show "@ path" to clarify where they run.
#[rstest]
fn test_switch_post_hook_shows_path_without_shell_integration(repo: TestRepo) {
    use std::fs;

    // Create project config with a post-switch hook
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "post-switch = \"echo switched\"\n",
    )
    .unwrap();

    repo.commit("Add config");

    // Run switch WITHOUT directive file (shell integration not active)
    // Use --yes to auto-approve the hook command
    // The hook output should show "@ path" annotation
    snapshot_switch(
        "switch_post_hook_path_annotation",
        &repo,
        &["--create", "post-hook-test", "--yes"],
    );
}

///
/// When running through the shell wrapper (directive file set), the user's shell will
/// actually cd to the worktree. Hooks don't need the path annotation.
#[rstest]
fn test_switch_post_hook_no_path_with_shell_integration(repo: TestRepo) {
    use std::fs;

    // Create project config with a post-switch hook
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "post-switch = \"echo switched\"\n",
    )
    .unwrap();

    repo.commit("Add config");

    // Run switch WITH directive file (shell integration active)
    // The hook output should NOT show "@ path" annotation
    snapshot_switch_with_directive_file(
        "switch_post_hook_no_path_with_shell_integration",
        &repo,
        &["--create", "post-hook-shell-test", "--yes"],
    );
}

#[rstest]
fn test_switch_clobber_path_with_extension(repo: TestRepo) {
    // Calculate where the worktree would be created
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.clobber-ext.txt", repo_name));

    // Create a file with an extension at that path
    std::fs::write(&expected_path, "file with extension").unwrap();

    // With --clobber, should move the file preserving extension in backup name
    snapshot_switch(
        "switch_clobber_path_with_extension",
        &repo,
        &["--create", "--clobber", "clobber-ext.txt"],
    );

    // Verify the worktree was created
    assert!(expected_path.is_dir());

    // Verify backup path includes the original extension
    // file.txt -> file.txt.bak.TIMESTAMP
    let backup_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.clobber-ext.txt.bak.20250102-000000", repo_name));
    assert!(
        backup_path.exists(),
        "Backup should exist at {:?}",
        backup_path
    );
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        "file with extension"
    );
}

#[rstest]
fn test_switch_create_no_hint_with_custom_worktree_path(repo: TestRepo) {
    // Set up custom worktree-path in user config
    repo.write_test_config(r#"worktree-path = ".worktrees/{{ branch | sanitize }}""#);

    let output = repo
        .wt_command()
        .args(["switch", "--create", "test-no-hint"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Customize worktree locations"),
        "Hint should be suppressed when user has custom worktree-path config"
    );
}

// ============================================================================
// PR Syntax Tests (pr:<number>)
// ============================================================================

use crate::common::mock_commands::{MockConfig, MockResponse, copy_mock_binary};

/// Helper to set up mock gh for PR tests with custom PR response.
///
/// The response should be in `gh api repos/{owner}/{repo}/pulls/{number}` format:
/// - `head.ref`, `head.repo.owner.login`, `head.repo.name`
/// - `base.repo.owner.login`, `base.repo.name`
/// - `html_url`
fn setup_mock_gh_for_pr(repo: &TestRepo, gh_response: Option<&str>) -> std::path::PathBuf {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    // Copy mock-stub binary as "gh"
    copy_mock_binary(&mock_bin, "gh");

    // Write PR response file if provided
    if let Some(response) = gh_response {
        fs::write(mock_bin.join("pr_response.json"), response).unwrap();

        MockConfig::new("gh")
            .version("gh version 2.0.0 (mock)")
            .command("api", MockResponse::file("pr_response.json"))
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);
    }

    mock_bin
}

/// Configure command environment for mock gh.
fn configure_mock_gh_env(cmd: &mut std::process::Command, mock_bin: &Path) {
    // Tell mock-stub where to find config files
    cmd.env("MOCK_CONFIG_DIR", mock_bin);

    // Build PATH with mock binary first
    let (path_var_name, current_path) = std::env::vars_os()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), Some(v)))
        .unwrap_or(("PATH".to_string(), None));

    let mut paths: Vec<std::path::PathBuf> = current_path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    paths.insert(0, mock_bin.to_path_buf());
    let new_path = std::env::join_paths(&paths)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    cmd.env(path_var_name, new_path);
}

/// Test that --create flag conflicts with pr: syntax
#[rstest]
fn test_switch_pr_create_conflict(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "pr:101"], None);
        assert_cmd_snapshot!("switch_pr_create_conflict", cmd);
    });
}

/// Test same-repo PR checkout (base.repo == head.repo)
#[rstest]
fn test_switch_pr_same_repo(#[from(repo_with_remote)] mut repo: TestRepo) {
    // Create a feature branch and push it to the remote
    repo.add_worktree("feature-auth");
    repo.run_git(&["push", "origin", "feature-auth"]);

    // Get the bare remote's actual URL before we modify origin
    let bare_url = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["config", "remote.origin.url"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Set origin URL to GitHub-style so find_remote_for_repo() can match owner/test-repo
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    // Configure git to redirect github.com URLs to the local bare remote.
    // This is necessary because:
    // 1. origin must have a GitHub URL for find_remote_for_repo() to match owner/repo
    // 2. But we need git fetch to actually succeed using the local bare remote
    repo.run_git(&[
        "config",
        &format!("url.{}.insteadOf", bare_url),
        "https://github.com/owner/test-repo.git",
    ]);

    // gh api repos/{owner}/{repo}/pulls/{number} format
    let gh_response = r#"{
        "head": {
            "ref": "feature-auth",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/101"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_same_repo", cmd);
    });
}

/// Test same-repo PR when origin points to a different repo (no remote for PR's repo)
///
/// User scenario:
/// 1. User has origin pointing to their fork (contributor/test-repo)
/// 2. PR #101 is a same-repo PR on the upstream (owner/test-repo)
/// 3. No remote exists for owner/test-repo -> error with hint to add upstream
#[rstest]
fn test_switch_pr_same_repo_no_remote(#[from(repo_with_remote)] repo: TestRepo) {
    // Set origin to point to a DIFFERENT repo than where the PR is
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/contributor/test-repo.git",
    ]);

    // gh api response says base.repo and head.repo are both owner/test-repo (same-repo PR)
    // but origin points to contributor/test-repo (different repo)
    // So find_remote_for_repo("owner", "test-repo") will fail
    let gh_response = r#"{
        "head": {
            "ref": "feature-auth",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/101"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_same_repo_no_remote", cmd);
    });
}

/// Test fork PR checkout (base.repo != head.repo)
#[rstest]
fn test_switch_pr_fork(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a PR ref on the remote that can be fetched
    // First, create a commit that represents the PR head
    repo.run_git(&["checkout", "-b", "pr-source"]);
    fs::write(repo.root_path().join("pr-file.txt"), "PR content").unwrap();
    repo.run_git(&["add", "pr-file.txt"]);
    repo.run_git(&["commit", "-m", "PR commit"]);

    // Get the commit SHA and push to remote as refs/pull/42/head
    let commit_sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&commit_sha.stdout)
        .trim()
        .to_string();

    // Push the ref to the bare remote
    repo.run_git(&["push", "origin", &format!("{}:refs/pull/42/head", sha)]);

    // Go back to main
    repo.run_git(&["checkout", "main"]);

    // Get the bare remote's actual URL before we modify origin
    let bare_url = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["config", "remote.origin.url"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Set origin URL to GitHub-style so find_remote_for_repo() can match owner/test-repo
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    // Configure git to redirect github.com URLs to the local bare remote.
    // This is necessary because:
    // 1. origin must have a GitHub URL for find_remote_for_repo() to match owner/repo
    // 2. But we need git fetch to actually succeed using the local bare remote
    // Git's url.<base>.insteadOf transparently rewrites the fetch URL.
    repo.run_git(&[
        "config",
        &format!("url.{}.insteadOf", bare_url),
        "https://github.com/owner/test-repo.git",
    ]);

    // gh api repos/{owner}/{repo}/pulls/{number} format
    // head.repo is the fork (contributor/test-repo), base.repo is the upstream (owner/test-repo)
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_fork", cmd);
    });
}

/// Test fork PR when origin points to fork (no remote for base repo)
///
/// User scenario:
/// 1. User forked upstream-owner/repo to contributor/repo
/// 2. User cloned their fork, so origin points to contributor/repo
/// 3. User tries to checkout PR from upstream-owner/repo
/// 4. No remote exists for the base repo -> error with hint to add upstream
#[rstest]
fn test_switch_pr_fork_no_upstream_remote(#[from(repo_with_remote)] repo: TestRepo) {
    // Set origin to point to the FORK (contributor's repo), not the base repo
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/contributor/test-repo.git",
    ]);

    // gh api response says base.repo is owner/test-repo (the upstream)
    // but origin points to contributor/test-repo (the fork)
    // So find_remote_for_repo("owner", "test-repo") will fail
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_fork_no_upstream", cmd);
    });
}

/// Test error when PR is not found
#[rstest]
fn test_switch_pr_not_found(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    // Copy mock-stub binary as "gh"
    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return error for PR not found (errors go to stderr)
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command(
            "api",
            MockResponse::stderr("gh api: Not Found (HTTP 404)").with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:9999"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_not_found", cmd);
    });
}

/// Test error when fork was deleted (head.repo is null)
#[rstest]
fn test_switch_pr_deleted_fork(#[from(repo_with_remote)] repo: TestRepo) {
    // gh api repos/{owner}/{repo}/pulls/{number} format with null head.repo
    // This happens when the fork that the PR was opened from has been deleted
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": null
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_deleted_fork", cmd);
    });
}

/// Test that --base flag conflicts with pr: syntax
#[rstest]
fn test_switch_pr_base_conflict(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--base", "main", "pr:101"], None);
        assert_cmd_snapshot!("switch_pr_base_conflict", cmd);
    });
}

/// Test fork PR where branch already exists and tracks same PR (should reuse)
#[rstest]
fn test_switch_pr_fork_existing_same_pr(#[from(repo_with_remote)] repo: TestRepo) {
    // First, manually create the branch with correct tracking config
    // Branch name matches headRefName (no owner prefix) so git push works
    let branch_name = "feature-fix";
    repo.run_git(&["branch", branch_name, "main"]);
    repo.run_git(&[
        "config",
        &format!("branch.{}.remote", branch_name),
        "origin",
    ]);
    repo.run_git(&[
        "config",
        &format!("branch.{}.merge", branch_name),
        "refs/pull/42/head",
    ]);

    // gh api repos/{owner}/{repo}/pulls/{number} format
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_fork_existing_same_pr", cmd);
    });
}

/// Test fork PR where branch already exists but tracks different PR (should error)
#[rstest]
fn test_switch_pr_fork_existing_different_pr(#[from(repo_with_remote)] repo: TestRepo) {
    // Create the branch with tracking config for a DIFFERENT PR
    // Branch name matches headRefName (no owner prefix) so git push works
    let branch_name = "feature-fix";
    repo.run_git(&["branch", branch_name, "main"]);
    repo.run_git(&[
        "config",
        &format!("branch.{}.remote", branch_name),
        "origin",
    ]);
    repo.run_git(&[
        "config",
        &format!("branch.{}.merge", branch_name),
        "refs/pull/99/head", // Different PR!
    ]);

    // gh api repos/{owner}/{repo}/pulls/{number} format
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_fork_existing_different_pr", cmd);
    });
}

/// Test fork PR where branch exists but has no tracking config
#[rstest]
fn test_switch_pr_fork_existing_no_tracking(#[from(repo_with_remote)] repo: TestRepo) {
    // Create the branch without any tracking config
    // Branch name matches headRefName (no owner prefix) so git push works
    let branch_name = "feature-fix";
    repo.run_git(&["branch", branch_name, "main"]);
    // No config set - branch exists but doesn't track anything

    // gh api repos/{owner}/{repo}/pulls/{number} format
    let gh_response = r#"{
        "head": {
            "ref": "feature-fix",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/42"
    }"#;

    let mock_bin = setup_mock_gh_for_pr(&repo, Some(gh_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:42"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_fork_existing_no_tracking", cmd);
    });
}

/// Test pr: when gh is not authenticated
#[rstest]
fn test_switch_pr_not_authenticated(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return auth error
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command(
            "api",
            MockResponse::stderr(
                "To use GitHub CLI in a non-interactive context, please run gh auth login",
            )
            .with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_not_authenticated", cmd);
    });
}

/// Test pr: when hitting GitHub rate limit
#[rstest]
fn test_switch_pr_rate_limit(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return rate limit error (HTTP 403)
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command(
            "api",
            MockResponse::stderr("gh api: API rate limit exceeded for user (HTTP 403)")
                .with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_rate_limit", cmd);
    });
}

/// Test pr: when gh returns invalid JSON
#[rstest]
fn test_switch_pr_invalid_json(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return invalid JSON
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command("api", MockResponse::output("not valid json {{{"))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_invalid_json", cmd);
    });
}

/// Test pr: when network error occurs
#[rstest]
fn test_switch_pr_network_error(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return network error
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command(
            "api",
            MockResponse::stderr("connection refused: network is unreachable").with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_network_error", cmd);
    });
}

/// Test pr: when gh returns unknown error
#[rstest]
fn test_switch_pr_unknown_error(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh api to return an unrecognized multi-line error
    // (realistic errors from gh often include context on multiple lines)
    let error_message = "error: unexpected API response\n\
                         code: 500\n\
                         message: Internal server error";
    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command("api", MockResponse::stderr(error_message).with_exit_code(1))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_unknown_error", cmd);
    });
}

/// Test pr: when PR has empty branch name
#[rstest]
fn test_switch_pr_empty_branch(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "gh");

    // Configure gh to return valid JSON but with empty branch name (head.ref is "")
    let gh_response = r#"{
        "head": {
            "ref": "",
            "repo": {"name": "test-repo", "owner": {"login": "contributor"}}
        },
        "base": {
            "ref": "main",
            "repo": {"name": "test-repo", "owner": {"login": "owner"}}
        },
        "html_url": "https://github.com/owner/test-repo/pull/101"
    }"#;

    MockConfig::new("gh")
        .version("gh version 2.0.0 (mock)")
        .command("api", MockResponse::output(gh_response))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["pr:101"], None);
        configure_mock_gh_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_pr_empty_branch", cmd);
    });
}

// ============================================================================
// MR Syntax Tests (mr:<number>) - GitLab
// ============================================================================

/// Helper to set up mock glab for MR tests with custom MR response.
///
/// The response should be in `glab mr view <number> --output json` format:
/// - `source_branch`, `source_project_id`, `target_project_id`
/// - `web_url`
fn setup_mock_glab_for_mr(repo: &TestRepo, glab_response: Option<&str>) -> std::path::PathBuf {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    // Copy mock-stub binary as "glab"
    copy_mock_binary(&mock_bin, "glab");

    // Write MR response file if provided
    if let Some(response) = glab_response {
        fs::write(mock_bin.join("mr_response.json"), response).unwrap();

        MockConfig::new("glab")
            .version("glab version 1.40.0 (mock)")
            .command("mr", MockResponse::file("mr_response.json"))
            .command("_default", MockResponse::exit(1))
            .write(&mock_bin);
    }

    mock_bin
}

/// Configure command environment for mock glab.
fn configure_mock_glab_env(cmd: &mut std::process::Command, mock_bin: &Path) {
    // Tell mock-stub where to find config files
    cmd.env("MOCK_CONFIG_DIR", mock_bin);

    // Build PATH with mock binary first
    let (path_var_name, current_path) = std::env::vars_os()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), Some(v)))
        .unwrap_or(("PATH".to_string(), None));

    let mut paths: Vec<std::path::PathBuf> = current_path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    paths.insert(0, mock_bin.to_path_buf());
    let new_path = std::env::join_paths(&paths)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    cmd.env(path_var_name, new_path);
}

/// Test that --create flag conflicts with mr: syntax
#[rstest]
fn test_switch_mr_create_conflict(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "mr:101"], None);
        assert_cmd_snapshot!("switch_mr_create_conflict", cmd);
    });
}

/// Test that --base flag conflicts with mr: syntax
#[rstest]
fn test_switch_mr_base_conflict(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--base", "main", "mr:101"], None);
        assert_cmd_snapshot!("switch_mr_base_conflict", cmd);
    });
}

/// Test same-repo MR checkout (source_project_id == target_project_id)
#[rstest]
fn test_switch_mr_same_repo(#[from(repo_with_remote)] mut repo: TestRepo) {
    // Create a feature branch and push it
    repo.add_worktree("feature-auth");

    // glab mr view <number> --output json format
    let glab_response = r#"{
        "source_branch": "feature-auth",
        "source_project_id": 123,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/101"
    }"#;

    let mock_bin = setup_mock_glab_for_mr(&repo, Some(glab_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:101"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_same_repo", cmd);
    });
}

/// Test error when MR is not found
#[rstest]
fn test_switch_mr_not_found(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    // Copy mock-stub binary as "glab"
    copy_mock_binary(&mock_bin, "glab");

    // Configure glab mr to return error for MR not found
    MockConfig::new("glab")
        .version("glab version 1.40.0 (mock)")
        .command(
            "mr",
            MockResponse::stderr(
                "GET https://gitlab.com/api/v4/projects/123/merge_requests/9999: 404 Not Found",
            )
            .with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:9999"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_not_found", cmd);
    });
}

/// Test mr: when glab is not authenticated
#[rstest]
fn test_switch_mr_not_authenticated(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "glab");

    // Configure glab mr to return auth error
    MockConfig::new("glab")
        .version("glab version 1.40.0 (mock)")
        .command(
            "mr",
            MockResponse::stderr(
                "glab: To use GitLab CLI in a non-interactive context, please run `glab auth login`",
            )
            .with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:101"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_not_authenticated", cmd);
    });
}

/// Test mr: when glab returns invalid JSON
#[rstest]
fn test_switch_mr_invalid_json(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "glab");

    // Configure glab mr to return invalid JSON
    MockConfig::new("glab")
        .version("glab version 1.40.0 (mock)")
        .command("mr", MockResponse::output("not valid json {{{"))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:101"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_invalid_json", cmd);
    });
}

/// Test mr: when MR has empty branch name
#[rstest]
fn test_switch_mr_empty_branch(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "glab");

    // Configure glab to return valid JSON but with empty branch name
    let glab_response = r#"{
        "source_branch": "",
        "source_project_id": 456,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/101"
    }"#;

    MockConfig::new("glab")
        .version("glab version 1.40.0 (mock)")
        .command("mr", MockResponse::output(glab_response))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:101"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_empty_branch", cmd);
    });
}

/// Test fork MR checkout (source_project_id != target_project_id)
#[rstest]
fn test_switch_mr_fork(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a MR ref on the remote that can be fetched
    // First, create a commit that represents the MR head
    repo.run_git(&["checkout", "-b", "mr-source"]);
    fs::write(repo.root_path().join("mr-file.txt"), "MR content").unwrap();
    repo.run_git(&["add", "mr-file.txt"]);
    repo.run_git(&["commit", "-m", "MR commit"]);

    // Get the commit SHA and push to remote as refs/merge-requests/42/head
    let commit_sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&commit_sha.stdout)
        .trim()
        .to_string();

    // Push the ref to the bare remote
    repo.run_git(&[
        "push",
        "origin",
        &format!("{}:refs/merge-requests/42/head", sha),
    ]);

    // Go back to main
    repo.run_git(&["checkout", "main"]);

    // Get the bare remote's actual URL before we modify origin
    let bare_url = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["config", "remote.origin.url"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Set origin URL to GitLab-style so find_remote_by_url() can match
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);

    // Configure git to redirect gitlab.com URLs to the local bare remote.
    // This is necessary because:
    // 1. origin must have a GitLab URL for find_remote_by_url() to match target project
    // 2. But we need git fetch to actually succeed using the local bare remote
    // Git's url.<base>.insteadOf transparently rewrites the fetch URL.
    repo.run_git(&[
        "config",
        &format!("url.{}.insteadOf", bare_url),
        "https://gitlab.com/owner/test-repo.git",
    ]);

    // glab mr view <number> --output json format
    // source_project is the fork (contributor/test-repo), target_project is the upstream (owner/test-repo)
    let glab_response = r#"{
        "source_branch": "feature-fix",
        "source_project_id": 456,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/42",
        "source_project": {
            "ssh_url_to_repo": "git@gitlab.com:contributor/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/contributor/test-repo.git"
        },
        "target_project": {
            "ssh_url_to_repo": "git@gitlab.com:owner/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/owner/test-repo.git"
        }
    }"#;

    let mock_bin = setup_mock_glab_for_mr(&repo, Some(glab_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:42"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_fork", cmd);
    });
}

/// Test fork MR checkout when branch already exists and tracks the MR
#[rstest]
fn test_switch_mr_fork_existing_branch_tracks_mr(#[from(repo_with_remote)] repo: TestRepo) {
    // Create the branch that will track the MR
    repo.run_git(&["checkout", "-b", "feature-fix"]);
    fs::write(repo.root_path().join("mr-file.txt"), "MR content").unwrap();
    repo.run_git(&["add", "mr-file.txt"]);
    repo.run_git(&["commit", "-m", "MR commit"]);

    // Get the commit SHA and push to remote as refs/merge-requests/42/head
    let commit_sha = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&commit_sha.stdout)
        .trim()
        .to_string();

    repo.run_git(&[
        "push",
        "origin",
        &format!("{}:refs/merge-requests/42/head", sha),
    ]);

    // Configure branch to track the MR ref (as our code would set it up)
    repo.run_git(&["config", "branch.feature-fix.remote", "origin"]);
    repo.run_git(&[
        "config",
        "branch.feature-fix.merge",
        "refs/merge-requests/42/head",
    ]);

    // Go back to main
    repo.run_git(&["checkout", "main"]);

    // Set origin URL to GitLab-style
    let bare_url = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["config", "remote.origin.url"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);
    repo.run_git(&[
        "config",
        &format!("url.{}.insteadOf", bare_url),
        "https://gitlab.com/owner/test-repo.git",
    ]);

    // Fork MR response
    let glab_response = r#"{
        "source_branch": "feature-fix",
        "source_project_id": 456,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/42",
        "source_project": {
            "ssh_url_to_repo": "git@gitlab.com:contributor/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/contributor/test-repo.git"
        },
        "target_project": {
            "ssh_url_to_repo": "git@gitlab.com:owner/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/owner/test-repo.git"
        }
    }"#;

    let mock_bin = setup_mock_glab_for_mr(&repo, Some(glab_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:42"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_fork_existing_branch_tracks_mr", cmd);
    });
}

/// Test fork MR checkout when branch exists but tracks something else
#[rstest]
fn test_switch_mr_fork_existing_branch_tracks_different(#[from(repo_with_remote)] repo: TestRepo) {
    // Create the branch that tracks a different ref
    repo.run_git(&["checkout", "-b", "feature-fix"]);
    fs::write(repo.root_path().join("mr-file.txt"), "MR content").unwrap();
    repo.run_git(&["add", "mr-file.txt"]);
    repo.run_git(&["commit", "-m", "MR commit"]);

    // Configure branch to track a different MR
    repo.run_git(&["config", "branch.feature-fix.remote", "origin"]);
    repo.run_git(&[
        "config",
        "branch.feature-fix.merge",
        "refs/merge-requests/99/head", // Different MR number
    ]);

    // Go back to main
    repo.run_git(&["checkout", "main"]);

    // Set origin URL to GitLab-style
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);

    // Fork MR response for MR 42, but branch tracks MR 99
    let glab_response = r#"{
        "source_branch": "feature-fix",
        "source_project_id": 456,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/42",
        "source_project": {
            "ssh_url_to_repo": "git@gitlab.com:contributor/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/contributor/test-repo.git"
        },
        "target_project": {
            "ssh_url_to_repo": "git@gitlab.com:owner/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/owner/test-repo.git"
        }
    }"#;

    let mock_bin = setup_mock_glab_for_mr(&repo, Some(glab_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:42"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_fork_existing_branch_tracks_different", cmd);
    });
}

/// Test fork MR checkout when branch exists but has no tracking config
#[rstest]
fn test_switch_mr_fork_existing_no_tracking(#[from(repo_with_remote)] repo: TestRepo) {
    // Create the branch without any tracking config
    repo.run_git(&["branch", "feature-fix", "main"]);
    // No config set - branch exists but doesn't track anything

    // Set origin URL to GitLab-style
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);

    // Fork MR response
    let glab_response = r#"{
        "source_branch": "feature-fix",
        "source_project_id": 456,
        "target_project_id": 123,
        "web_url": "https://gitlab.com/owner/test-repo/-/merge_requests/42",
        "source_project": {
            "ssh_url_to_repo": "git@gitlab.com:contributor/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/contributor/test-repo.git"
        },
        "target_project": {
            "ssh_url_to_repo": "git@gitlab.com:owner/test-repo.git",
            "http_url_to_repo": "https://gitlab.com/owner/test-repo.git"
        }
    }"#;

    let mock_bin = setup_mock_glab_for_mr(&repo, Some(glab_response));

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:42"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_fork_existing_no_tracking", cmd);
    });
}

/// Test mr: with unknown glab error (falls through to general error handler)
#[rstest]
fn test_switch_mr_unknown_error(#[from(repo_with_remote)] repo: TestRepo) {
    let mock_bin = repo.root_path().join("mock-bin");
    fs::create_dir_all(&mock_bin).unwrap();

    copy_mock_binary(&mock_bin, "glab");

    // Configure glab mr to return an unknown error
    MockConfig::new("glab")
        .version("glab version 1.40.0 (mock)")
        .command(
            "mr",
            MockResponse::stderr("glab: unexpected internal error: something went wrong")
                .with_exit_code(1),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["mr:101"], None);
        configure_mock_glab_env(&mut cmd, &mock_bin);
        assert_cmd_snapshot!("switch_mr_unknown_error", cmd);
    });
}
