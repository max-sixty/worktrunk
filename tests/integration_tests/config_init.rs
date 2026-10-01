use crate::common::{
    TestRepo, make_snapshot_cmd, repo, set_temp_home_env, set_xdg_config_path,
    setup_home_snapshot_settings, setup_snapshot_settings, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

#[rstest]
fn test_config_init_already_exists(temp_home: TempDir) {
    // Create fake global config at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("create");
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m User config already exists: [1m~/.config/worktrunk/config.toml[22m
        [2m↳[22m [2mTo view, run [4mwt config show[24m. To create a project config, run [4mwt config create --project[24m[22m
        ");
    });
}

#[rstest]
fn test_config_init_creates_file(temp_home: TempDir) {
    // Don't create config file - let create create it
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("create");
        set_temp_home_env(&mut cmd, temp_home.path());
        set_xdg_config_path(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mCreated user config: [1m~/.config/worktrunk/config.toml[22m[39m
        [2m↳[22m [2mEdit this file to customize worktree paths and LLM settings[22m
        ");
    });

    // Verify file was actually created
    let config_path = global_config_dir.join("config.toml");
    assert!(config_path.exists());
}

#[rstest]
fn test_config_create_project_creates_file(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "config", &["create", "--project"], None);
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [32m✓[39m [32mCreated project config: [1m_REPO_/.config/wt.toml[22m[39m
        [2m↳[22m [2mEdit this file to configure hooks for this repository[22m
        [2m↳[22m [2mSee https://worktrunk.dev/hook/ for hook documentation[22m
        ");
    });

    // Verify file was actually created
    let config_path = repo.root_path().join(".config/wt.toml");
    assert!(
        config_path.exists(),
        "Project config file should be created"
    );
}

#[rstest]
fn test_config_create_project_already_exists(repo: TestRepo) {
    // Create project config file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"[[project.pre-start]]
run = "echo hello"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "config", &["create", "--project"], None);
        assert_cmd_snapshot!(cmd, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [2m○[22m Project config already exists: [1m_REPO_/.config/wt.toml[22m
        [2m↳[22m [2mTo view, run [4mwt config show[24m. To create a user config, run [4mwt config create[24m[22m
        ");
    });
}

/// A dangling symlink occupies the path while `path.exists()` reads false, so
/// the create reaches its write with the path looking absent. It must refuse
/// rather than replace a dotfile manager's link with a regular file, and the
/// error has to name what it found — "File exists" right after the existence
/// check said otherwise explains nothing.
#[cfg(unix)]
#[rstest]
fn test_config_create_project_refuses_a_dangling_symlink(repo: TestRepo) {
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    let link = config_dir.join("wt.toml");
    std::os::unix::fs::symlink(config_dir.join("synced/wt.toml"), &link).unwrap();

    let output = repo
        .wt_command()
        .args(["config", "create", "--project"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dangling symlink"),
        "the error should name what occupies the path:\n{stderr}"
    );
    assert!(
        link.is_symlink() && !link.exists(),
        "the link should survive unreplaced"
    );
}

/// A write that fails for any other reason names the path it was writing,
/// rather than the bare I/O error.
#[cfg(unix)]
#[rstest]
fn test_config_create_project_write_failure_names_the_path(repo: TestRepo) {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    // Read-only directory: the temp file the write stages beside the target
    // can't be created.
    fs::set_permissions(&config_dir, Permissions::from_mode(0o555)).unwrap();

    // Skip when running as root — permissions don't restrict.
    let probe = config_dir.join("__probe");
    if fs::write(&probe, "").is_ok() {
        let _ = fs::remove_file(&probe);
        fs::set_permissions(&config_dir, Permissions::from_mode(0o755)).unwrap();
        eprintln!("Skipping - running with elevated privileges");
        return;
    }

    let output = repo
        .wt_command()
        .args(["config", "create", "--project"])
        .output()
        .unwrap();

    // Restore permissions so TempDir cleanup succeeds even if assertions fail.
    fs::set_permissions(&config_dir, Permissions::from_mode(0o755)).unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to write") && stderr.contains(".config/wt.toml"),
        "the error should name the path being written:\n{stderr}"
    );
    assert!(!config_dir.join("wt.toml").exists());
}

/// Running `wt config create --project` from inside a repo's `.git` directory
/// (not inside a worktree, not a bare repo) must fail with the generic
/// "no worktree found" error rather than the bare-repo-specific message.
#[rstest]
fn test_config_create_project_from_git_dir_errors(repo: TestRepo) {
    let git_dir = repo.path().join(".git");
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "config", &["create", "--project"], Some(&git_dir));
        assert_cmd_snapshot!(cmd);
    });
}
