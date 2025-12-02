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

[projects."test-project"]
approved-commands = ["npm install"]
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

        ----- stderr -----
        ⚪ User Config: [1m~/.config/worktrunk/config.toml[22m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m
        [107m [0m  
        [107m [0m  [1m[36m[projects."test-project"][0m
        [107m [0m  approved-commands = [[32m"npm install"[0m]

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[22m
        [107m [0m  post-create = [32m"npm install"[0m
        [107m [0m  
        [107m [0m  [1m[36m[post-start][0m
        [107m [0m  server = [32m"npm run dev"[0m

        [2m⚪ Skipped bash; ~/.bashrc not found[22m
        [2m⚪ Skipped zsh; ~/.zshrc not found[22m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[22m
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

        ----- stderr -----
        ⚪ User Config: [1m~/.config/worktrunk/config.toml[22m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[22m
        💡 [2mNot found[22m

        [2m⚪ Skipped bash; ~/.bashrc not found[22m
        [2m⚪ Skipped zsh; ~/.zshrc not found[22m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[22m
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

        ----- stderr -----
        ⚪ User Config: [1m~/.config/worktrunk/config.toml[22m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ [2mProject Config: Not in a git repository[22m

        [2m⚪ Skipped bash; ~/.bashrc not found[22m
        [2m⚪ Skipped zsh; ~/.zshrc not found[22m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[22m
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
        // Force compinit warning for deterministic tests across environments
        cmd.env("WT_ASSUME_NO_COMPINIT", "1");
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test `wt config show` shows hint when some shells configured, some not
#[test]
fn test_config_show_partial_shell_config_shows_hint() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(global_config_dir.join("config.toml"), "").unwrap();

    // Create .bashrc WITHOUT wt integration
    fs::write(
        temp_home.path().join(".bashrc"),
        r#"# Some bash config
export PATH="$HOME/bin:$PATH"
"#,
    )
    .unwrap();

    // Create .zshrc WITH wt integration
    fs::write(
        temp_home.path().join(".zshrc"),
        r#"# wt integration
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
        cmd.env("WT_ASSUME_COMPINIT", "1"); // Bypass zsh subprocess check

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

/// Test `wt config show` warns about unknown/misspelled keys in project config
#[test]
fn test_config_show_warns_unknown_project_keys() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create global config
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"",
    )
    .unwrap();

    // Create project config with typo: post-merge-command instead of post-merge
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "[post-merge-command]\ndeploy = \"task deploy\"",
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

        ----- stderr -----
        ⚪ User Config: [1m~/.config/worktrunk/config.toml[22m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[22m
        🟡 [33mUnknown key [1mpost-merge-command[22m will be ignored[39m
        [107m [0m  [1m[36m[post-merge-command][0m
        [107m [0m  deploy = [32m"task deploy"[0m

        [2m⚪ Skipped bash; ~/.bashrc not found[22m
        [2m⚪ Skipped zsh; ~/.zshrc not found[22m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[22m
        "#);
    });
}

/// Test `wt config show` warns about unknown keys in user config
#[test]
fn test_config_show_warns_unknown_user_keys() {
    let repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Create global config with typo: commit-gen instead of commit-generation
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"\n\n[commit-gen]\ncommand = \"llm\"",
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

        ----- stderr -----
        ⚪ User Config: [1m~/.config/worktrunk/config.toml[22m
        🟡 [33mUnknown key [1mcommit-gen[22m will be ignored[39m
        [107m [0m  worktree-path = [32m"../{{ main_worktree }}.{{ branch }}"[0m
        [107m [0m  
        [107m [0m  [1m[36m[commit-gen][0m
        [107m [0m  command = [32m"llm"[0m

        ⚪ Project Config: [1m[REPO]/.config/wt.toml[22m
        💡 [2mNot found[22m

        [2m⚪ Skipped bash; ~/.bashrc not found[22m
        [2m⚪ Skipped zsh; ~/.zshrc not found[22m
        [2m⚪ Skipped fish; ~/.config/fish/conf.d not found[22m
        "#);
    });
}
