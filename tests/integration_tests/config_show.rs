use crate::common::{
    TestRepo, set_temp_home_env, setup_home_snapshot_settings, setup_snapshot_settings_with_home,
    wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test `wt config show` with both global and project configs present
#[test]
fn test_config_show_with_project_config() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fake global config at XDG path (used on all platforms with etcetera)
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[[approved-commands]]
project = "test-project"
command = "npm install"
"#,
    )
    .unwrap();

    // Create project config
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-create = "npm install"

[post-start]
server = "npm run dev"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m
        [107m [0m  
        [107m [0m  [1m[36m[[approved-commands]][0m
        [107m [0m  project = [32m"test-project"[0m
        [107m [0m  command = [32m"npm install"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[0m
        [107m [0m  post-create = [32m"npm install"[0m
        [107m [0m  
        [107m [0m  [1m[36m[post-start][0m
        [107m [0m  server = [32m"npm run dev"[0m

        [2m⚪ Skipped bash; ~/.bashrc not found[0m
        [2m⚪ Skipped zsh; ~/.zshrc not found[0m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[0m

        ----- stderr -----
        "#);
    });
}

/// Test `wt config show` when there is no project config
#[test]
fn test_config_show_no_project_config() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create fake global config (but no project config) at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[0m
        💡 [2mNot found[0m

        [2m⚪ Skipped bash; ~/.bashrc not found[0m
        [2m⚪ Skipped zsh; ~/.zshrc not found[0m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[0m

        ----- stderr -----
        "#);
    });
}

/// Test `wt config show` outside a git repository
#[test]
fn test_config_show_outside_git_repo() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_home = TempDir::new().unwrap();

    // Create fake global config at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_home_snapshot_settings(&temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("show").current_dir(temp_dir.path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd, @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ⚪ Global Config: [1m~/.config/worktrunk/config.toml[0m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ [2mProject Config: Not in a git repository[0m

        [2m⚪ Skipped bash; ~/.bashrc not found[0m
        [2m⚪ Skipped zsh; ~/.zshrc not found[0m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[0m

        ----- stderr -----
        "#);
    });
}

/// Test `wt config show` warns when zsh compinit is not enabled
#[test]
fn test_config_show_zsh_compinit_warning() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .zshrc WITHOUT compinit - completions won't work
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# wt integration but no compinit!
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` shows no warning when zsh compinit is enabled
#[test]
fn test_config_show_zsh_compinit_correct_order() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .zshrc with compinit enabled - completions will work
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# compinit enabled
autoload -Uz compinit && compinit

# wt integration
if command -v wt >/dev/null 2>&1; then eval "$(command wt config shell init zsh)"; fi
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.env("WT_ASSUME_COMPINIT", "1"); // Bypass zsh subprocess check (unreliable on CI)

        assert_cmd_snapshot!(cmd);
    });
}
