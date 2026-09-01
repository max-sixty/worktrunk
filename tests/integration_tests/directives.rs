use crate::common::{
    TestRepo, configure_directive_file, directive_file, repo, repo_with_feature_worktree,
    repo_with_remote, repo_with_remote_and_feature, setup_snapshot_settings, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
use std::path::Path;

// ============================================================================
// CD Directive File Tests
// ============================================================================
// These tests verify the directive-file protocol:
// - WORKTRUNK_DIRECTIVE_CD_FILE: wt writes a raw path (no `cd ` prefix, no quotes).
//   The shell wrapper runs `cd -- "$(< file)"`.

#[rstest]
fn test_switch_retired_directive_file_does_not_block_execute(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let feature_wt = repo.add_worktree("feature");
    let directive_dir = tempfile::TempDir::new().unwrap();
    let directive_path = directive_dir.path().join("directive");
    let execute_marker = feature_wt.join("retired-execute-ran");
    fs::write(&directive_path, "").unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.env("WORKTRUNK_DIRECTIVE_FILE", &directive_path);
        cmd.arg("switch")
            .arg("feature")
            .arg("--execute")
            .arg("sh")
            .args(["--", "-c", "echo spawned > retired-execute-ran"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        assert!(execute_marker.exists(), "--execute should run directly");
        assert_eq!(
            fs::read_to_string(&directive_path).unwrap(),
            "",
            "wt must never write to the retired directive file"
        );
    });
}

/// A current wrapper may inherit retired variables from an older parent shell.
/// The current CD file stays active, execution stays direct, and the stale
/// execute wrapper gets a migration warning.
#[rstest]
fn test_current_wrapper_wins_retired_variables_and_warns(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();
    let retired_dir = tempfile::TempDir::new().unwrap();
    let retired_path = retired_dir.path().join("directive");
    let retired_exec_path = retired_dir.path().join("execute");
    fs::write(&retired_path, "").unwrap();
    fs::write(&retired_exec_path, "").unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = repo.wt_command();
        configure_directive_file(&mut cmd, &cd_path);
        cmd.env("WORKTRUNK_DIRECTIVE_FILE", &retired_path)
            .env("WORKTRUNK_DIRECTIVE_EXEC_FILE", &retired_exec_path)
            .args([
                "switch",
                "feature",
                "--execute",
                "git",
                "--",
                "rev-parse",
                "--abbrev-ref",
                "HEAD",
            ])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        assert!(
            !fs::read_to_string(&cd_path)
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "the current wrapper's CD directive should remain active"
        );
        assert_eq!(
            fs::read_to_string(&retired_path).unwrap(),
            "",
            "wt must never write to the retired directive file"
        );
        assert_eq!(
            fs::read_to_string(&retired_exec_path).unwrap(),
            "",
            "wt must never write to the retired execute directive file"
        );
    });
}

// ============================================================================
// Directive Protocol Tests
// ============================================================================

#[rstest]
fn test_switch_directive_file(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("switch")
            .arg("feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file contains a raw path (no `cd ` prefix, no quotes)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            !cd_content.trim().is_empty(),
            "CD file should contain a path, got: {}",
            cd_content
        );
        assert!(
            !cd_content.contains("cd "),
            "CD file should contain a raw path (no cd prefix), got: {}",
            cd_content
        );
    });
}

/// A failed CD-directive write names the file too, for the same reason.
#[rstest]
fn test_cd_directive_write_failure_names_the_file(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();
    let unwritable_cd = cd_path.parent().unwrap().join("no-such-dir").join("cd");

    let mut cmd = repo.wt_command();
    configure_directive_file(&mut cmd, &unwritable_cd);
    let output = cmd
        .args(["switch", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an unwritable cd directive file must fail the command.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Failed to write the cd directive file"),
        "the error must name the write and the file.\nstderr:\n{stderr}"
    );
}

#[rstest]
fn test_merge_directive_file(mut repo_with_remote_and_feature: TestRepo) {
    let repo = &mut repo_with_remote_and_feature;
    let feature_wt = &repo.worktrees["feature"];
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("merge").arg("main").current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify cd file contains a raw path (back to main)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            !cd_content.trim().is_empty(),
            "CD file should contain a path, got: {}",
            cd_content
        );
        assert!(
            !cd_content.contains("cd "),
            "CD file should contain a raw path (no cd prefix), got: {}",
            cd_content
        );
    });
}

#[rstest]
fn test_remove_directive_file(#[from(repo_with_remote)] mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("remove").current_dir(&feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify cd file contains a raw path (back to main)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            !cd_content.trim().is_empty(),
            "CD file should contain a path, got: {}",
            cd_content
        );
        assert!(
            !cd_content.contains("cd "),
            "CD file should contain a raw path (no cd prefix), got: {}",
            cd_content
        );
    });
}

// ============================================================================
// Subdirectory Preservation Tests
// ============================================================================
// These tests verify that switching preserves the user's subdirectory position

#[rstest]
fn test_switch_preserves_subdir(#[from(repo_with_remote)] mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create the same subdirectory in both worktrees
    let subdir = "apps/gateway";
    fs::create_dir_all(repo.root_path().join(subdir)).unwrap();
    fs::create_dir_all(feature_wt.join(subdir)).unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.arg("switch")
        .arg("feature")
        .current_dir(repo.root_path().join(subdir));

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt switch failed: {:?}", output);

    // Verify cd file contains path to the subdirectory, not the root.
    // Use Path::join for each component so separators are native on Windows.
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let expected_subdir = feature_wt.join(Path::new("apps").join("gateway"));
    let expected_str = expected_subdir.to_string_lossy();
    assert!(
        cd_content.contains(&*expected_str),
        "CD file should contain subdirectory path {}, got: {}",
        expected_str,
        cd_content
    );
}

#[rstest]
fn test_switch_falls_back_to_root_when_subdir_missing(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create subdirectory only in the source worktree, not in the target
    let subdir = "apps/gateway";
    fs::create_dir_all(repo.root_path().join(subdir)).unwrap();
    // Intentionally NOT creating the subdir in feature_wt

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.arg("switch")
        .arg("feature")
        .current_dir(repo.root_path().join(subdir));

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt switch failed: {:?}", output);

    // Verify cd file contains path to worktree root (not the missing subdir)
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let feature_str = feature_wt.to_string_lossy();
    assert!(
        cd_content.contains(&*feature_str),
        "CD file should contain worktree root {}, got: {}",
        feature_str,
        cd_content
    );
    // Make sure it doesn't contain the subdir path.
    // Use Path::join for each component so separators are native on Windows.
    let subdir_path = feature_wt.join(Path::new("apps").join("gateway"));
    let subdir_str = subdir_path.to_string_lossy();
    assert!(
        !cd_content.contains(&*subdir_str),
        "CD file should NOT contain missing subdirectory path {}, got: {}",
        subdir_str,
        cd_content
    );
}

#[rstest]
fn test_switch_create_preserves_subdir(#[from(repo_with_remote)] repo: TestRepo) {
    let (cd_path, _guard) = directive_file();

    // Create a subdirectory in the source worktree and commit it so it appears in the new branch
    let subdir = "apps/gateway";
    fs::create_dir_all(repo.root_path().join(subdir)).unwrap();
    // Add a file so git tracks the directory
    fs::write(repo.root_path().join(subdir).join(".gitkeep"), "").unwrap();
    repo.commit("Add apps/gateway");

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.args(["switch", "--create", "new-feature"])
        .current_dir(repo.root_path().join(subdir));

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt switch failed: {:?}", output);

    // The subdirectory was committed, so the new worktree should have it.
    // Use Path to construct the expected substring so separators match on Windows.
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let subdir_suffix = Path::new("apps").join("gateway");
    let subdir_str = subdir_suffix.to_string_lossy();
    assert!(
        cd_content.contains(&*subdir_str),
        "CD file should contain preserved subdirectory path, got: {}",
        cd_content
    );
}

#[rstest]
fn test_remove_preserves_subdir(#[from(repo_with_remote)] mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create the same subdirectory in both the feature worktree (where the
    // user is) and the main worktree (where removal lands).
    let subdir = "apps/gateway";
    fs::create_dir_all(repo.root_path().join(subdir)).unwrap();
    fs::create_dir_all(feature_wt.join(subdir)).unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.arg("remove").current_dir(feature_wt.join(subdir));

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt remove failed: {:?}", output);

    // Verify cd file lands in the equivalent subdirectory of the main
    // worktree, not at its root — mirroring `wt switch` (issue #3343).
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let expected_subdir = repo.root_path().join(Path::new("apps").join("gateway"));
    let expected_str = expected_subdir.to_string_lossy();
    assert!(
        cd_content.contains(&*expected_str),
        "CD file should contain subdirectory path {}, got: {}",
        expected_str,
        cd_content
    );
}

#[rstest]
fn test_remove_falls_back_to_root_when_subdir_missing(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create the subdirectory only in the feature worktree (where the user
    // is), not in the main worktree where removal lands.
    let subdir = "apps/gateway";
    fs::create_dir_all(feature_wt.join(subdir)).unwrap();
    // Intentionally NOT creating the subdir in the main worktree.

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.arg("remove").current_dir(feature_wt.join(subdir));

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt remove failed: {:?}", output);

    // Verify cd file lands at the main worktree root (the subdir is absent
    // there, so preservation falls back).
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let root_str = repo.root_path().to_string_lossy();
    assert!(
        cd_content.contains(&*root_str),
        "CD file should contain main worktree root {}, got: {}",
        root_str,
        cd_content
    );
    let subdir_path = repo.root_path().join(Path::new("apps").join("gateway"));
    let subdir_str = subdir_path.to_string_lossy();
    assert!(
        !cd_content.contains(&*subdir_str),
        "CD file should NOT contain missing subdirectory path {}, got: {}",
        subdir_str,
        cd_content
    );
}

// ============================================================================
// --no-cd Tests
// ============================================================================
// These tests verify that --no-cd suppresses directory changes

#[rstest]
fn test_switch_no_cd_suppresses_directive(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.args(["switch", "feature", "--no-cd"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file is empty (no path written with --no-cd)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            cd_content.trim().is_empty(),
            "CD file should be empty with --no-cd, got: {}",
            cd_content
        );
    });
}

#[rstest]
fn test_switch_no_cd_create_suppresses_directive(#[from(repo_with_remote)] repo: TestRepo) {
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.args(["switch", "--create", "new-feature", "--no-cd"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file is empty (no path written with --no-cd)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            cd_content.trim().is_empty(),
            "CD file should be empty with --no-cd, got: {}",
            cd_content
        );
    });
}

#[rstest]
fn test_switch_no_cd_hooks_show_path_annotation(#[from(repo_with_remote)] repo: TestRepo) {
    let (cd_path, _guard) = directive_file();

    // Create project config with a post-switch hook
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "post-switch = \"echo switched\"\n",
    )
    .unwrap();

    repo.commit("Add config");

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        // Use --yes to auto-approve the hook command
        cmd.args(["switch", "--create", "hook-test", "--no-cd", "--yes"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file is empty (no path written with --no-cd)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            cd_content.trim().is_empty(),
            "CD file should be empty with --no-cd, got: {}",
            cd_content
        );
    });
}

#[rstest]
fn test_switch_no_cd_execute_does_not_emit_cd(#[from(repo_with_remote)] repo: TestRepo) {
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        // The wrapper will launch pwd from the invoking directory because
        // --no-cd deliberately leaves its shell there.
        cmd.args([
            "switch",
            "--create",
            "exec-test",
            "--no-cd",
            "--execute",
            "pwd",
        ])
        .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file is empty (no path written with --no-cd)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            cd_content.trim().is_empty(),
            "CD file should be empty with --no-cd, got: {}",
            cd_content
        );
    });
}

/// Config-driven no-cd suppresses the cd directive (same as --no-cd flag)
#[rstest]
fn test_switch_no_cd_config_suppresses_directive(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Set up config with cd = false
    repo.write_test_config(
        r#"worktree-path = "../{{ repo }}.{{ branch }}"

[switch]
cd = false
"#,
    );

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.args(["switch", "feature"])
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);

        // Verify cd file is empty (no path written with cd=false config)
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            cd_content.trim().is_empty(),
            "CD file should be empty with no-cd config, got: {}",
            cd_content
        );
    });
}

// ============================================================================
// Non-Directive Mode Tests (no CD directive env var)
// ============================================================================

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

#[rstest]
fn test_merge_directive_no_remove(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("merge")
            .arg("main")
            .arg("--no-remove")
            .current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);
    });
}

#[rstest]
fn test_merge_directive_remove(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];
    let (cd_path, _guard) = directive_file();

    let settings = setup_snapshot_settings(repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        configure_directive_file(&mut cmd, &cd_path);
        cmd.arg("merge").arg("main").current_dir(feature_wt);

        assert_cmd_snapshot!(cmd);

        // Verify cd file contains a raw path
        let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
        assert!(
            !cd_content.trim().is_empty(),
            "CD file should contain a path, got: {}",
            cd_content
        );
        assert!(
            !cd_content.contains("cd "),
            "CD file should contain a raw path (no cd prefix), got: {}",
            cd_content
        );
    });
}

// ============================================================================
// Symlink Path Preservation Tests
// ============================================================================
// These tests verify that cd directives use the logical (symlink) path
// instead of the canonical path when the user navigates via symlinks.

#[cfg(unix)]
#[rstest]
fn test_switch_preserves_symlink_path(#[from(repo_with_remote)] mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create a symlink to the repo's parent directory
    let real_parent = repo.root_path().parent().unwrap();
    let symlink_dir = tempfile::tempdir().unwrap();
    let symlink_path = symlink_dir.path().join("link");
    unix_fs::symlink(real_parent, &symlink_path).unwrap();

    // Construct the symlinked path to the repo
    let repo_dir_name = repo.root_path().file_name().unwrap();
    let logical_cwd = symlink_path.join(repo_dir_name);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    // Set PWD to the logical (symlink) path — this is what the shell sets
    cmd.env("PWD", &logical_cwd);
    cmd.arg("switch").arg("feature").current_dir(&logical_cwd);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt switch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The cd file should use the logical (symlink) path, not the canonical one
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();

    // The symlink prefix should appear in the cd path
    let symlink_prefix = symlink_path.to_string_lossy();
    assert!(
        cd_content.contains(&*symlink_prefix),
        "CD file should use symlink path (containing {}), got: {}",
        symlink_prefix,
        cd_content
    );

    // The canonical (real) parent path should NOT appear
    let real_prefix = real_parent.to_string_lossy();
    assert!(
        !cd_content.contains(&*real_prefix),
        "CD file should NOT contain canonical path {}, got: {}",
        real_prefix,
        cd_content
    );

    // Display messages (stderr) should also use the logical path
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&*symlink_prefix),
        "Display message should contain logical path {}, got: {}",
        symlink_prefix,
        stderr
    );
    assert!(
        !stderr.contains(&*real_prefix),
        "Display message should NOT contain canonical path {}, got: {}",
        real_prefix,
        stderr
    );
}

#[cfg(unix)]
#[rstest]
fn test_switch_create_preserves_symlink_path(#[from(repo_with_remote)] repo: TestRepo) {
    let (cd_path, _guard) = directive_file();

    // Create a symlink to the repo's parent directory
    let real_parent = repo.root_path().parent().unwrap();
    let symlink_dir = tempfile::tempdir().unwrap();
    let symlink_path = symlink_dir.path().join("link");
    unix_fs::symlink(real_parent, &symlink_path).unwrap();

    let repo_dir_name = repo.root_path().file_name().unwrap();
    let logical_cwd = symlink_path.join(repo_dir_name);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.env("PWD", &logical_cwd);
    cmd.args(["switch", "--create", "new-feature"])
        .current_dir(&logical_cwd);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt switch --create failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    let symlink_prefix = symlink_path.to_string_lossy();
    assert!(
        cd_content.contains(&*symlink_prefix),
        "CD file should use symlink path (containing {}), got: {}",
        symlink_prefix,
        cd_content
    );

    // Display messages (stderr) should also use the logical path
    let real_prefix = real_parent.to_string_lossy();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&*symlink_prefix),
        "Display message should contain logical path {}, got: {}",
        symlink_prefix,
        stderr
    );
    assert!(
        !stderr.contains(&*real_prefix),
        "Display message should NOT contain canonical path {}, got: {}",
        real_prefix,
        stderr
    );
}

#[cfg(unix)]
#[rstest]
fn test_switch_preserves_symlink_path_from_subdirectory(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    // Create subdirectory in both worktrees
    let subdir = "apps/gateway";
    fs::create_dir_all(repo.root_path().join(subdir)).unwrap();
    fs::create_dir_all(feature_wt.join(subdir)).unwrap();

    // Create a symlink to the repo's parent directory
    let real_parent = repo.root_path().parent().unwrap();
    let symlink_dir = tempfile::tempdir().unwrap();
    let symlink_path = symlink_dir.path().join("link");
    unix_fs::symlink(real_parent, &symlink_path).unwrap();

    let repo_dir_name = repo.root_path().file_name().unwrap();
    let logical_cwd = symlink_path.join(repo_dir_name).join(subdir);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    cmd.env("PWD", &logical_cwd);
    cmd.arg("switch").arg("feature").current_dir(&logical_cwd);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt switch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();

    // Should use symlink prefix AND preserve subdirectory
    let symlink_prefix = symlink_path.to_string_lossy();
    assert!(
        cd_content.contains(&*symlink_prefix),
        "CD file should use symlink path (containing {}), got: {}",
        symlink_prefix,
        cd_content
    );

    let subdir_suffix = Path::new("apps").join("gateway");
    let subdir_str = subdir_suffix.to_string_lossy();
    assert!(
        cd_content.contains(&*subdir_str),
        "CD file should preserve subdirectory {}, got: {}",
        subdir_str,
        cd_content
    );
}

#[cfg(unix)]
#[rstest]
fn test_switch_no_symlink_uses_canonical(#[from(repo_with_remote)] mut repo: TestRepo) {
    // When PWD matches current_dir (no symlink), canonical path is used as before
    let _feature_wt = repo.add_worktree("feature");
    let (cd_path, _guard) = directive_file();

    let canonical_cwd = dunce::canonicalize(repo.root_path()).unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    configure_directive_file(&mut cmd, &cd_path);
    // Set PWD to canonical (same as current_dir — no symlink)
    cmd.env("PWD", &canonical_cwd);
    cmd.arg("switch").arg("feature").current_dir(&canonical_cwd);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt switch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Should still have a path in the cd file (just with canonical path)
    let cd_content = fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        !cd_content.trim().is_empty(),
        "CD file should contain a path, got: {}",
        cd_content
    );
}
