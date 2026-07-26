//! Integration tests for the experimental `worktrunk.config.*` git-config
//! project-config source (#3454).
//!
//! Covers: source selection (keys present → git config wins, file ignored),
//! the supersession warning firing iff a file would otherwise load, scope
//! precedence (local over global) resolved by git itself, `include.path`
//! resolution, loud failure on unexpressible values, and approval gating of
//! git-config-sourced hooks.

use crate::common::{
    TestRepo, repo, set_temp_home_env, setup_snapshot_settings_with_home, temp_home, wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use tempfile::TempDir;

fn write_user_config(temp_home: &TempDir) {
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ repo }}.{{ branch }}"
"#,
    )
    .unwrap();
}

/// Keys present AND a project config file present: the git-config source
/// wins, the heading names it, and the supersession warning fires.
#[rstest]
fn test_hook_show_git_config_supersedes_project_file(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Keys present, no project config file: same selection, but nothing is
/// superseded so no warning appears.
#[rstest]
fn test_hook_show_git_config_source_without_file(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// A value the schema cannot accept as a string fails the load loudly; the
/// project config file is NOT silently used instead (all-or-nothing).
#[rstest]
fn test_git_config_source_invalid_value_fails_loudly(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    // `step.copy-ignored.exclude` is an array in the schema; a string leaf
    // cannot satisfy it.
    repo.run_git(&[
        "config",
        "worktrunk.config.step.copy-ignored.exclude",
        "target",
    ]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        cmd.arg("hook").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// `wt config show` names the git-config source, warns about the superseded
/// file, and dumps the mapped TOML.
#[rstest]
fn test_config_show_git_config_source(mut repo: TestRepo, temp_home: TempDir) {
    repo.setup_mock_ci_tools_unauthenticated();
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);
    repo.run_git(&[
        "config",
        "worktrunk.config.list.url",
        "http://localhost:3000",
    ]);

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.configure_wt_cmd(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        cmd.arg("config").arg("show").current_dir(repo.root_path());
        set_temp_home_env(&mut cmd, temp_home.path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Git resolves scope precedence before worktrunk reads the keys: a local
/// key overrides its global twin, and global-only keys still merge in.
#[rstest]
fn test_git_config_scope_precedence_local_over_global(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&[
        "config",
        "--global",
        "worktrunk.config.list.url",
        "http://global:9999",
    ]);
    repo.run_git(&[
        "config",
        "--global",
        "worktrunk.config.forge.platform",
        "gitlab",
    ]);
    repo.run_git(&["config", "worktrunk.config.list.url", "http://local:3000"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let project = &json["project"];
    assert_eq!(project["source"], "git-config");
    assert_eq!(project["exists"], true);
    assert_eq!(project["path"], serde_json::Value::Null);
    assert_eq!(project["config"]["list"]["url"], "http://local:3000");
    assert_eq!(project["config"]["forge"]["platform"], "gitlab");
}

/// Keys reachable only through `include.path` resolve like any other git
/// config — git processes includes before worktrunk sees the merged list.
#[rstest]
fn test_git_config_source_include_path(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    let fragment = temp_home.path().join("wt-private.gitconfig");
    fs::write(
        &fragment,
        r#"[worktrunk "config.list"]
	url = http://from-include:1234
"#,
    )
    .unwrap();
    repo.run_git(&["config", "include.path", fragment.to_str().unwrap()]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["project"]["source"], "git-config");
    assert_eq!(
        json["project"]["config"]["list"]["url"],
        "http://from-include:1234"
    );
}

/// A `WORKTRUNK_PROJECT_CONFIG_PATH` override names the config source
/// outright: with the override set, git keys are ignored and the override
/// file loads.
#[rstest]
fn test_project_config_path_override_beats_git_source(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "from git config"]);
    let override_path = temp_home.path().join("override-wt.toml");
    fs::write(&override_path, "post-start = \"from override file\"\n").unwrap();

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env("WORKTRUNK_PROJECT_CONFIG_PATH", &override_path)
        .args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["project"]["source"], "file");
    assert_eq!(
        json["project"]["config"]["post-start"],
        "from override file"
    );
}

/// The empty-override kill switch stays authoritative: no project config at
/// all, even with git keys present.
#[rstest]
fn test_empty_override_disables_git_source(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "from git config"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.env("WORKTRUNK_PROJECT_CONFIG_PATH", "")
        .args(["config", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["project"]["exists"], false);
    assert_eq!(json["project"]["source"], serde_json::Value::Null);
}

/// Alias discovery reflects the selected source: a git-config alias appears
/// in the listing, the superseded file's alias does not — discovery and
/// dispatch must name the same command set.
#[rstest]
fn test_alias_listing_reflects_git_config_source(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(
        r#"[aliases]
file-alias = "echo from file"
"#,
    );
    repo.commit("Add project config");
    repo.run_git(&[
        "config",
        "worktrunk.config.aliases.git-alias",
        "echo from git",
    ]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "alias", "show"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("git-alias"),
        "git-config alias missing from listing:\n{stdout}"
    );
    assert!(
        !stdout.contains("file-alias"),
        "superseded file alias must not be listed:\n{stdout}"
    );
}

/// `wt config create --project` warns when the file it creates is born
/// superseded by existing git keys.
#[rstest]
fn test_config_create_project_warns_when_born_superseded(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["config", "create", "--project"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(repo.root_path().join(".config/wt.toml").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("will be ignored until they are removed")
            && stderr.contains("worktrunk.config."),
        "born-superseded warning missing:\n{stderr}"
    );
}

/// Declining approval for a git-config hook keeps it from running — the gate
/// blocks execution, not just reporting. Piped stdin is non-interactive, so
/// the prompt path lists the commands and refuses; the hook artifact must
/// not exist afterward.
#[rstest]
fn test_git_config_hook_declined_does_not_run(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&[
        "config",
        "worktrunk.config.pre-start",
        "echo ran > git-config-hook-artifact.txt",
    ]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["switch", "--create", "gated-feature"])
        .current_dir(repo.root_path())
        .stdin(std::process::Stdio::piped());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("needs approval") && stderr.contains("pre-start"),
        "git-config hook should enter the project approval path:\n{stderr}"
    );
    assert!(
        !repo
            .root_path()
            .join("git-config-hook-artifact.txt")
            .exists(),
        "declined git-config hook must not run"
    );
}

/// At a bare root with no worktrees, the git-config source still supplies
/// project config — `wt config approvals clear --stale` frames its answer
/// from it instead of erroring "no worktree found".
#[rstest]
fn test_bare_root_approvals_clear_stale_uses_git_source(_repo: TestRepo) {
    let bare = crate::common::BareRepoTest::new();
    let status = std::process::Command::new("git")
        .args(["-C", bare.bare_repo_path().to_str().unwrap()])
        .args(["config", "worktrunk.config.post-start", "npm install"])
        .status()
        .unwrap();
    assert!(status.success());

    let mut cmd = bare.wt_command();
    cmd.args(["config", "approvals", "clear", "--stale"])
        .current_dir(bare.bare_repo_path());

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected git-config source to satisfy the project-config requirement:\n{stderr}"
    );
    assert!(
        stderr.contains("No stale approvals to clear"),
        "unexpected output:\n{stderr}"
    );
}

/// The diagnostic report names the git-config source and its key names, but
/// never the values — diagnose output is routinely pasted into public bug
/// reports, and this source exists for private configuration.
#[rstest]
fn test_diagnostic_report_omits_git_config_values(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.write_project_config(r#"pre-merge = "cargo test""#);
    repo.commit("Add project config");
    repo.run_git(&[
        "config",
        "worktrunk.config.post-start",
        "echo diag-private-value",
    ]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["list", "-vv"]).current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());
    cmd.output().unwrap();

    let report = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .expect("-vv run writes a diagnostic report");

    // Scope the value-absence assertion to the config section: the -vv trace
    // section embeds raw subprocess output, which includes the whole
    // `git config --list -z` listing — a pre-existing disclosure surface for
    // ALL git config values, not something this feature introduces or can
    // fix here. The contract under test is that the *config section* names
    // keys without values.
    let section_start = report
        .find("Project config: git config (worktrunk.config.*)")
        .unwrap_or_else(|| panic!("report should name the git-config source:\n{report}"));
    let section = &report[section_start..];
    let section = &section[..section.find("\n\n").unwrap_or(section.len())];
    assert!(
        section.contains("worktrunk.config.post-start"),
        "config section should name the active keys:\n{section}"
    );
    assert!(
        section.contains("values omitted"),
        "config section should state values are omitted:\n{section}"
    );
    assert!(
        !section.contains("diag-private-value"),
        "private hook body leaked into the config section:\n{section}"
    );
}

/// Git-config-sourced hooks pass through the same approval gate as
/// file-based project config — nothing about the source is trusted.
#[rstest]
fn test_git_config_source_hooks_require_approval(repo: TestRepo, temp_home: TempDir) {
    write_user_config(&temp_home);
    repo.run_git(&["config", "worktrunk.config.post-start", "npm install"]);

    let mut cmd = wt_command();
    repo.configure_wt_cmd(&mut cmd);
    cmd.args(["hook", "show", "--format=json"])
        .current_dir(repo.root_path());
    set_temp_home_env(&mut cmd, temp_home.path());

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hook = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["source"] == "project")
        .expect("project hook present");
    assert_eq!(hook["template"], "npm install");
    assert_eq!(hook["needs_approval"], true);
}
