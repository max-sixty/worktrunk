use crate::common::{
    TestRepo, configure_directive_file, directive_file, repo, repo_with_feature_worktree,
    repo_with_remote, repo_with_remote_and_feature, setup_snapshot_settings, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

// ============================================================================
// Directive File Tests
// ============================================================================
// These tests verify that WORKTRUNK_DIRECTIVE_FILE env var causes directives to be
// written to the file. The shell wrapper sources this file after wt exits.

/// Test that switch with directive file writes cd command to file
#[rstest]
fn test_switch_directive_file(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (directive_path, _guard) = directive_file();

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize the directive file cd path
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.arg("switch")
            .arg("feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify directive file contains cd command
        let directives = std::fs::read_to_string(&directive_path).unwrap_or_default();
        assert!(
            directives.contains("cd '"),
            "Directive file should contain cd command, got: {}",
            directives
        );
    });
}

/// Test merge with directive file (switch back to main after merge)
#[rstest]
fn test_merge_directive_file(mut repo_with_remote_and_feature: TestRepo) {
    let repo = &mut repo_with_remote_and_feature;
    let feature_wt = &repo.worktrees["feature"];
    let (directive_path, _guard) = directive_file();

    let mut settings = setup_snapshot_settings(repo);
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.arg("merge").arg("main").current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify directive file contains cd command (back to main)
        let directives = std::fs::read_to_string(&directive_path).unwrap_or_default();
        assert!(
            directives.contains("cd '"),
            "Directive file should contain cd command, got: {}",
            directives
        );
    });
}

/// Test that remove with directive file writes cd command to file
#[rstest]
fn test_remove_directive_file(#[from(repo_with_remote)] mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");
    let (directive_path, _guard) = directive_file();

    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.arg("remove").current_dir(&feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify directive file contains cd command (back to main)
        let directives = std::fs::read_to_string(&directive_path).unwrap_or_default();
        assert!(
            directives.contains("cd '"),
            "Directive file should contain cd command, got: {}",
            directives
        );
    });
}

// ============================================================================
// Non-Directive Mode Tests (no WORKTRUNK_DIRECTIVE_FILE)
// ============================================================================

/// Test switch without directive file (error case - branch not found)
#[rstest]
fn test_switch_without_directive_file(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test remove without directive file (error case - main worktree)
#[rstest]
fn test_remove_without_directive_file(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("remove").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test merge with directive file and --no-remove
#[rstest]
fn test_merge_directive_no_remove(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];
    let (directive_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.arg("merge")
            .arg("main")
            .arg("--no-remove")
            .current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);
    });
}

/// Test merge with directive file (removes worktree, writes cd to file)
#[rstest]
fn test_merge_directive_remove(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];
    let (directive_path, _guard) = directive_file();

    let mut settings = setup_snapshot_settings(repo);
    settings.add_filter(r"cd '[^']+'", "cd '[PATH]'");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &directive_path);
        cmd.arg("merge").arg("main").current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify directive file contains cd command
        let directives = std::fs::read_to_string(&directive_path).unwrap_or_default();
        assert!(
            directives.contains("cd '"),
            "Directive file should contain cd command, got: {}",
            directives
        );
    });
}
