use crate::common::{TestRepo, repo, repo_with_remote, setup_snapshot_settings, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

// ============================================================================
// PowerShell Directive Tests
// ============================================================================
// These tests verify that --internal=powershell produces correct PowerShell syntax.
// The PowerShell directive mode outputs:
// - `Set-Location 'path'` instead of `cd 'path'`
// - Proper single-quote escaping ('' instead of '\'' for embedded quotes)

/// Test that switch with --internal=powershell outputs PowerShell Set-Location syntax
#[rstest]
fn test_switch_internal_powershell_directive(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize the PowerShell Set-Location path
    settings.add_filter(r"Set-Location '[^']+'", "Set-Location '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal=powershell")
            .arg("switch")
            .arg("feature")
            .current_dir(repo.root_path());

        // Use file-based snapshot since inline snapshots don't handle
        // path normalization and ANSI codes well
        assert_cmd_snapshot!(cmd);
    });
}

/// Test merge with --internal=powershell (switch back to main after merge)
#[rstest]
fn test_merge_internal_powershell_directive(#[from(repo_with_remote)] mut repo: TestRepo) {
    repo.add_main_worktree();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "feature content",
        "Add feature file",
    );

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize the PowerShell Set-Location path
    settings.add_filter(r"Set-Location '[^']+'", "Set-Location '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal=powershell")
            .arg("merge")
            .arg("main")
            .current_dir(&feature_wt);

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that remove with --internal=powershell outputs PowerShell Set-Location syntax
///
/// Note: Skipped on Windows due to platform differences in ANSI escape code handling
/// (Unix doubles ESC[36m, Windows doesn't). The Set-Location syntax itself works correctly
/// on both platforms - this is purely a test snapshot compatibility issue.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_remove_internal_powershell_directive(#[from(repo_with_remote)] mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize the PowerShell Set-Location path
    settings.add_filter(r"Set-Location '[^']+'", "Set-Location '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal=powershell")
            .arg("remove")
            .current_dir(&feature_wt);

        assert_cmd_snapshot!(cmd);
    });
}

// ============================================================================
// POSIX Directive Tests (existing tests)
// ============================================================================

/// Test the directive protocol for switch command
#[rstest]
fn test_switch_internal_directive(repo: TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize the shell script cd path output
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        [0m❌ [31mBranch [1mmy-feature[22m not found[39m

        💡 [2mUse [90m--create[39m to create a new branch, or [90mwt list --branches --remotes[39m for available branches[22m
        ");
    });
}

/// Test switch without internal flag (should show help message)
#[rstest]
fn test_switch_without_internal(repo: TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        ❌ [31mBranch [1mmy-feature[22m not found[39m

        💡 [2mUse [90m--create[39m to create a new branch, or [90mwt list --branches --remotes[39m for available branches[22m
        ");
    });
}

/// Test remove command with internal flag
#[rstest]
fn test_remove_internal_directive(repo: TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize the shell script cd path output
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("remove")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        [0m❌ [31mThe main worktree cannot be removed[39m
        ");
    });
}

/// Test remove without internal flag
#[rstest]
fn test_remove_without_internal(repo: TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("remove").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        ❌ [31mThe main worktree cannot be removed[39m
        ");
    });
}

/// Test merge command with internal flag and --no-remove
#[rstest]
fn test_merge_internal_no_remove(mut repo: TestRepo) {
    repo.add_main_worktree();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "feature content",
        "Add feature file",
    );

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("merge")
            .arg("main")
            .arg("--no-remove")
            .current_dir(&feature_wt);

        // Note: Using file snapshot instead of inline because multiline inline snapshots
        // don't work well with NUL bytes (\0) in the output
        assert_cmd_snapshot!(cmd);
    });
}

/// Test merge command with internal flag (removes worktree, emits cd shell script)
/// This test verifies that the shell script output is correctly formatted
#[rstest]
fn test_merge_internal_remove(mut repo: TestRepo) {
    repo.add_main_worktree();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "feature content",
        "Add feature file",
    );

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("merge")
            .arg("main")
            .current_dir(&feature_wt);

        assert_cmd_snapshot!(cmd);
    });
}
