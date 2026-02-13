//! Integration tests for jj (Jujutsu) workspace support.
//!
//! These tests exercise the `wt` CLI against real jj repositories.
//! They require `jj` to be installed (0.38.0+). Gated behind the
//! `shell-integration-tests` feature flag (alongside shell/PTY tests).
#![cfg(all(unix, feature = "shell-integration-tests"))]

use crate::common::{
    canonicalize, configure_cli_command, configure_directive_file, directive_file,
    setup_snapshot_settings_for_jj, wt_bin,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::{fixture, rstest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

// ============================================================================
// JjTestRepo — test fixture for jj repositories
// ============================================================================

pub struct JjTestRepo {
    _temp_dir: TempDir,
    root: PathBuf,
    workspaces: HashMap<String, PathBuf>,
    /// Snapshot settings guard — keeps insta filters active for this repo's lifetime.
    _snapshot_guard: insta::internals::SettingsBindDropGuard,
}

impl JjTestRepo {
    /// Create a new jj repository with deterministic configuration.
    ///
    /// The repo includes:
    /// - A `jj git init` repository at `{temp}/repo/`
    /// - Deterministic user config (Test User / test@example.com)
    /// - An initial commit with README.md
    /// - A `main` bookmark on trunk so `trunk()` resolves
    pub fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let repo_dir = temp_dir.path().join("repo");

        // jj git init repo
        let output = Command::new("jj")
            .args(["git", "init", "repo"])
            .current_dir(temp_dir.path())
            .output()
            .expect("Failed to run jj git init");
        assert!(
            output.status.success(),
            "jj git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let root = canonicalize(&repo_dir).unwrap();

        // Configure deterministic user identity
        run_jj_in(
            &root,
            &["config", "set", "--repo", "user.name", "Test User"],
        );
        run_jj_in(
            &root,
            &["config", "set", "--repo", "user.email", "test@example.com"],
        );

        // Create initial commit with a file so trunk() resolves
        std::fs::write(root.join("README.md"), "# Test repo\n").unwrap();
        run_jj_in(&root, &["describe", "-m", "Initial commit"]);
        // Create new empty commit on top so @ is separate from trunk
        run_jj_in(&root, &["new"]);
        // Set main bookmark on the initial commit (trunk)
        run_jj_in(&root, &["bookmark", "set", "main", "-r", "@-"]);

        let workspaces = HashMap::new();
        let snapshot_guard = setup_snapshot_settings_for_jj(&root, &workspaces).bind_to_scope();

        Self {
            _temp_dir: temp_dir,
            root,
            workspaces,
            _snapshot_guard: snapshot_guard,
        }
    }

    /// Root path of the default workspace.
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// The temp directory containing the repo (used as HOME in tests).
    fn home_path(&self) -> &Path {
        self._temp_dir.path()
    }

    /// Add a new workspace with the given name.
    ///
    /// Creates the workspace as a sibling directory: `{temp}/repo.{name}`
    pub fn add_workspace(&mut self, name: &str) -> PathBuf {
        if let Some(path) = self.workspaces.get(name) {
            return path.clone();
        }

        let ws_path = self.root.parent().unwrap().join(format!("repo.{name}"));
        let ws_path_str = ws_path.to_str().unwrap();

        run_jj_in(
            &self.root,
            &["workspace", "add", "--name", name, ws_path_str],
        );

        let canonical = canonicalize(&ws_path).unwrap();
        self.workspaces.insert(name.to_string(), canonical.clone());
        canonical
    }

    /// Make a commit in a specific workspace directory.
    pub fn commit_in(&self, dir: &Path, filename: &str, content: &str, message: &str) {
        std::fs::write(dir.join(filename), content).unwrap();
        run_jj_in(dir, &["describe", "-m", message]);
        run_jj_in(dir, &["new"]);
    }

    /// Create a `wt` command pre-configured for this jj test repo.
    pub fn wt_command(&self) -> Command {
        let mut cmd = Command::new(wt_bin());
        self.configure_wt_cmd(&mut cmd);
        cmd.current_dir(&self.root);
        cmd
    }

    /// Configure a wt command with isolated test environment.
    pub fn configure_wt_cmd(&self, cmd: &mut Command) {
        configure_cli_command(cmd);
        // Point to a non-existent config so tests are isolated
        let test_config = self.home_path().join("test-config.toml");
        cmd.env("WORKTRUNK_CONFIG_PATH", &test_config);
        // Set HOME to temp dir so paths normalize
        let home = canonicalize(self.home_path()).unwrap();
        cmd.env("HOME", &home);
        cmd.env("XDG_CONFIG_HOME", home.join(".config"));
        cmd.env("USERPROFILE", &home);
        cmd.env("APPDATA", home.join(".config"));
    }

    /// Write a config file with a mock LLM command and return its path.
    ///
    /// The command just echoes a fixed commit message, ignoring stdin.
    pub fn write_llm_config(&self) -> PathBuf {
        let config_path = self.home_path().join("llm-config.toml");
        std::fs::write(
            &config_path,
            "[commit.generation]\ncommand = \"echo LLM-generated-message\"\n",
        )
        .unwrap();
        config_path
    }

    /// Path to a named workspace.
    pub fn workspace_path(&self, name: &str) -> &Path {
        self.workspaces
            .get(name)
            .unwrap_or_else(|| panic!("Workspace '{}' not found", name))
    }
}

/// Run a jj command in a directory, panicking on failure.
fn run_jj_in(dir: &Path, args: &[&str]) {
    let mut full_args = vec!["--no-pager", "--color", "never"];
    full_args.extend_from_slice(args);

    let output = Command::new("jj")
        .args(&full_args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute jj {}: {}", args.join(" "), e));

    if !output.status.success() {
        panic!(
            "jj {} failed:\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ============================================================================
// Snapshot helpers
// ============================================================================

fn make_jj_snapshot_cmd(
    repo: &JjTestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(wt_bin());
    repo.configure_wt_cmd(&mut cmd);
    cmd.arg(subcommand)
        .args(args)
        .current_dir(cwd.unwrap_or(repo.root_path()));
    cmd
}

/// Like `make_jj_snapshot_cmd` but with a custom config path (e.g., for LLM tests).
fn make_jj_snapshot_cmd_with_config(
    repo: &JjTestRepo,
    subcommand: &str,
    args: &[&str],
    cwd: Option<&Path>,
    config_path: &Path,
) -> Command {
    let mut cmd = make_jj_snapshot_cmd(repo, subcommand, args, cwd);
    cmd.env("WORKTRUNK_CONFIG_PATH", config_path);
    cmd
}

// ============================================================================
// rstest fixtures
// ============================================================================

#[fixture]
fn jj_repo() -> JjTestRepo {
    JjTestRepo::new()
}

/// Repo with one feature workspace containing a commit.
#[fixture]
fn jj_repo_with_feature(mut jj_repo: JjTestRepo) -> JjTestRepo {
    let ws = jj_repo.add_workspace("feature");
    jj_repo.commit_in(&ws, "feature.txt", "feature content", "Add feature");
    jj_repo
}

/// Repo with two feature workspaces.
#[fixture]
fn jj_repo_with_two_features(mut jj_repo: JjTestRepo) -> JjTestRepo {
    let ws_a = jj_repo.add_workspace("feature-a");
    jj_repo.commit_in(&ws_a, "a.txt", "content a", "Add feature A");
    let ws_b = jj_repo.add_workspace("feature-b");
    jj_repo.commit_in(&ws_b, "b.txt", "content b", "Add feature B");
    jj_repo
}

// ============================================================================
// wt list tests
// ============================================================================

#[rstest]
fn test_jj_list_single_workspace(jj_repo: JjTestRepo) {
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

#[rstest]
fn test_jj_list_multiple_workspaces(jj_repo_with_two_features: JjTestRepo) {
    let repo = jj_repo_with_two_features;
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&repo, "list", &[], None));
}

#[rstest]
fn test_jj_list_from_feature_workspace(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&repo, "list", &[], Some(feature_path)));
}

#[rstest]
fn test_jj_list_dirty_workspace(mut jj_repo: JjTestRepo) {
    // Add workspace and write a file without committing (jj auto-snapshots)
    let ws = jj_repo.add_workspace("dirty");
    std::fs::write(ws.join("uncommitted.txt"), "dirty content").unwrap();
    // jj auto-snapshots on next command, so the workspace will show as dirty
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

#[rstest]
fn test_jj_list_workspace_with_no_user_commits(mut jj_repo: JjTestRepo) {
    // A newly created workspace has no user commits — only the jj workspace
    // creation commits (new empty @ on top of trunk). This shows as "ahead"
    // due to jj's workspace mechanics, even though no real work has been done.
    jj_repo.add_workspace("integrated");
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

// ============================================================================
// wt switch tests
// ============================================================================

#[rstest]
fn test_jj_switch_to_existing_workspace(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    // Switch from default to feature workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&repo, "switch", &["feature"], None));
}

#[rstest]
fn test_jj_switch_to_existing_with_directive_file(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let (directive_path, _guard) = directive_file();
    assert_cmd_snapshot!({
        let mut cmd = make_jj_snapshot_cmd(&repo, "switch", &["feature"], None);
        configure_directive_file(&mut cmd, &directive_path);
        cmd
    });
}

#[rstest]
fn test_jj_switch_create_new_workspace(jj_repo: JjTestRepo) {
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "switch",
        &["--create", "new-feature"],
        None
    ));
}

#[rstest]
fn test_jj_switch_create_with_directive_file(jj_repo: JjTestRepo) {
    let (directive_path, _guard) = directive_file();
    assert_cmd_snapshot!({
        let mut cmd = make_jj_snapshot_cmd(&jj_repo, "switch", &["--create", "new-ws"], None);
        configure_directive_file(&mut cmd, &directive_path);
        cmd
    });
}

#[rstest]
fn test_jj_switch_nonexistent_workspace(jj_repo: JjTestRepo) {
    // Without --create, should fail with helpful error
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "switch",
        &["nonexistent"],
        None
    ));
}

#[rstest]
fn test_jj_switch_already_at_workspace(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Switch to feature from within feature workspace — should be no-op
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "switch",
        &["feature"],
        Some(feature_path)
    ));
}

// ============================================================================
// wt remove tests
// ============================================================================

#[rstest]
fn test_jj_remove_workspace(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Remove feature workspace from within it
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(feature_path)
    ));
}

#[rstest]
fn test_jj_remove_workspace_by_name(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    // Remove by name from default workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&repo, "remove", &["feature"], None));
}

#[rstest]
fn test_jj_remove_default_fails(jj_repo: JjTestRepo) {
    // Cannot remove default workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "remove", &["default"], None));
}

#[rstest]
fn test_jj_remove_current_workspace_cds_to_default(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");

    let (directive_path, _guard) = directive_file();
    assert_cmd_snapshot!({
        let mut cmd = make_jj_snapshot_cmd(&repo, "remove", &[], Some(feature_path));
        configure_directive_file(&mut cmd, &directive_path);
        cmd
    });
}

#[rstest]
fn test_jj_remove_already_on_default(jj_repo: JjTestRepo) {
    // Try to remove when already on default (no workspace name given)
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "remove", &[], None));
}

// ============================================================================
// wt merge tests
// ============================================================================

#[rstest]
fn test_jj_merge_squash(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Merge feature into main (squash is default for jj)
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(feature_path)
    ));
}

#[rstest]
fn test_jj_merge_squash_with_directive_file(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    let (directive_path, _guard) = directive_file();
    assert_cmd_snapshot!({
        let mut cmd = make_jj_snapshot_cmd(&repo, "merge", &["main"], Some(feature_path));
        configure_directive_file(&mut cmd, &directive_path);
        cmd
    });
}

#[rstest]
fn test_jj_merge_no_remove(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Merge but keep the workspace (--no-remove)
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-remove"],
        Some(feature_path)
    ));
}

#[rstest]
fn test_jj_merge_workspace_with_no_user_commits(mut jj_repo: JjTestRepo) {
    // Workspace has only jj's workspace creation commits (no real work).
    // Squash merge is a no-op in terms of content, but still cleans up.
    let ws = jj_repo.add_workspace("integrated");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "merge",
        &["main"],
        Some(&ws)
    ));
}

#[rstest]
fn test_jj_merge_from_default_fails(jj_repo: JjTestRepo) {
    // Cannot merge the default workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "merge", &["main"], None));
}

#[rstest]
fn test_jj_merge_multi_commit(mut jj_repo: JjTestRepo) {
    // Feature with multiple commits
    let ws = jj_repo.add_workspace("multi");
    jj_repo.commit_in(&ws, "file1.txt", "content 1", "Add file 1");
    jj_repo.commit_in(&ws, "file2.txt", "content 2", "Add file 2");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "merge",
        &["main"],
        Some(&ws)
    ));
}

// ============================================================================
// Edge cases
// ============================================================================

#[rstest]
fn test_jj_switch_create_and_then_list(jj_repo: JjTestRepo) {
    // Create a workspace via wt switch --create, then verify it appears in list
    let mut cmd = jj_repo.wt_command();
    cmd.args(["switch", "--create", "via-switch"]);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt switch --create failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // List should show the new workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

#[rstest]
fn test_jj_multiple_operations(mut jj_repo: JjTestRepo) {
    // Create workspace, commit, remove — full lifecycle
    let ws = jj_repo.add_workspace("lifecycle");
    jj_repo.commit_in(&ws, "life.txt", "content", "Lifecycle commit");

    // Verify it exists in list output
    let output = jj_repo.wt_command().arg("list").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("lifecycle"),
        "Expected 'lifecycle' in list output: {stdout}"
    );

    // Merge it
    let mut cmd = jj_repo.wt_command();
    cmd.args(["merge", "main"]).current_dir(&ws);
    let merge_output = cmd.output().unwrap();
    assert!(
        merge_output.status.success(),
        "merge failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&merge_output.stdout),
        String::from_utf8_lossy(&merge_output.stderr)
    );
}

#[rstest]
fn test_jj_remove_nonexistent_workspace(jj_repo: JjTestRepo) {
    // Try to remove a workspace that doesn't exist
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "remove",
        &["nonexistent"],
        None
    ));
}

#[rstest]
fn test_jj_switch_to_default(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Switch from feature back to default
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "switch",
        &["default"],
        Some(feature_path)
    ));
}

#[rstest]
fn test_jj_list_after_remove(mut jj_repo: JjTestRepo) {
    // Create a workspace, then remove it, then list
    let ws = jj_repo.add_workspace("temp");
    jj_repo.commit_in(&ws, "temp.txt", "content", "Temp commit");

    // Remove by name
    let mut cmd = jj_repo.wt_command();
    cmd.args(["remove", "temp"]);
    let output = cmd.output().unwrap();
    assert!(output.status.success());

    // List should only show default workspace
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

#[rstest]
fn test_jj_merge_with_no_squash(jj_repo_with_feature: JjTestRepo) {
    let repo = jj_repo_with_feature;
    let feature_path = repo.workspace_path("feature");
    // Merge without squash (rebase mode)
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-squash"],
        Some(feature_path)
    ));
}

// ============================================================================
// wt step commit tests
// ============================================================================

#[rstest]
fn test_jj_step_commit_with_changes(jj_repo: JjTestRepo) {
    // Write a file (jj auto-snapshots, so @ will have content)
    std::fs::write(jj_repo.root_path().join("new.txt"), "content\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_nothing_to_commit(jj_repo: JjTestRepo) {
    // @ is empty (fresh workspace), so step commit should fail
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_in_feature_workspace(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("feat");
    std::fs::write(ws.join("feat.txt"), "feature content\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["commit"],
        Some(&ws)
    ));
}

#[rstest]
fn test_jj_step_commit_reuses_existing_description(jj_repo: JjTestRepo) {
    // Write a file, then manually describe @ — step commit should reuse that description
    std::fs::write(jj_repo.root_path().join("described.txt"), "content\n").unwrap();
    run_jj_in(
        jj_repo.root_path(),
        &["describe", "-m", "My custom message"],
    );

    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_multiple_files(jj_repo: JjTestRepo) {
    // Write 4 files — should generate "Changes to 4 files"
    for name in &["a.txt", "b.txt", "c.txt", "d.txt"] {
        std::fs::write(jj_repo.root_path().join(name), "content\n").unwrap();
    }
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_two_files(jj_repo: JjTestRepo) {
    // 2 files — should generate "Changes to X & Y"
    std::fs::write(jj_repo.root_path().join("alpha.txt"), "a\n").unwrap();
    std::fs::write(jj_repo.root_path().join("beta.txt"), "b\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_three_files(jj_repo: JjTestRepo) {
    // 3 files — should generate "Changes to X, Y & Z"
    std::fs::write(jj_repo.root_path().join("alpha.txt"), "a\n").unwrap();
    std::fs::write(jj_repo.root_path().join("beta.txt"), "b\n").unwrap();
    std::fs::write(jj_repo.root_path().join("gamma.txt"), "c\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["commit"], None));
}

#[rstest]
fn test_jj_step_commit_show_prompt(jj_repo: JjTestRepo) {
    // --show-prompt with no LLM configured
    std::fs::write(jj_repo.root_path().join("prompt.txt"), "content\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["commit", "--show-prompt"],
        None
    ));
}

#[rstest]
fn test_jj_step_commit_with_llm(jj_repo: JjTestRepo) {
    // Commit with a mock LLM command configured
    let config = jj_repo.write_llm_config();
    std::fs::write(jj_repo.root_path().join("llm.txt"), "content\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd_with_config(
        &jj_repo,
        "step",
        &["commit"],
        None,
        &config
    ));
}

#[rstest]
fn test_jj_step_commit_show_prompt_with_llm(jj_repo: JjTestRepo) {
    // --show-prompt with LLM configured — should print the actual prompt
    let config = jj_repo.write_llm_config();
    std::fs::write(jj_repo.root_path().join("llm.txt"), "content\n").unwrap();
    assert_cmd_snapshot!(make_jj_snapshot_cmd_with_config(
        &jj_repo,
        "step",
        &["commit", "--show-prompt"],
        None,
        &config
    ));
}

// ============================================================================
// wt step squash tests
// ============================================================================

#[rstest]
fn test_jj_step_squash_multiple_commits(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("squash-test");
    jj_repo.commit_in(&ws, "a.txt", "content a", "First commit");
    jj_repo.commit_in(&ws, "b.txt", "content b", "Second commit");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["squash"],
        Some(&ws)
    ));
}

#[rstest]
fn test_jj_step_squash_already_single_commit(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("single");
    jj_repo.commit_in(&ws, "only.txt", "only content", "Only commit");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["squash"],
        Some(&ws)
    ));
}

#[rstest]
fn test_jj_step_squash_no_commits_ahead(jj_repo: JjTestRepo) {
    // Default workspace with no feature commits — nothing to squash
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["squash"], None));
}

#[rstest]
fn test_jj_step_squash_already_integrated(mut jj_repo: JjTestRepo) {
    // Feature that has already been squash-merged into trunk via wt merge
    let ws = jj_repo.add_workspace("integrated");
    jj_repo.commit_in(&ws, "i.txt", "content", "Feature commit");

    // Merge it into trunk first
    let mut merge_cmd = jj_repo.wt_command();
    configure_cli_command(&mut merge_cmd);
    merge_cmd
        .current_dir(&ws)
        .args(["merge", "main", "--no-remove"]);
    let merge_result = merge_cmd.output().unwrap();
    assert!(
        merge_result.status.success(),
        "merge failed: {}",
        String::from_utf8_lossy(&merge_result.stderr)
    );

    // Now step squash should say "nothing to squash" (already integrated)
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["squash"],
        Some(&ws)
    ));
}

// ============================================================================
// wt step rebase tests
// ============================================================================

#[rstest]
fn test_jj_step_rebase_already_up_to_date(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("rebased");
    jj_repo.commit_in(&ws, "r.txt", "content", "Feature commit");

    // Feature is already on trunk — should be up to date
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["rebase"],
        Some(&ws)
    ));
}

#[rstest]
fn test_jj_step_rebase_onto_advanced_trunk(mut jj_repo: JjTestRepo) {
    // Create feature workspace
    let ws = jj_repo.add_workspace("rebase-feat");
    jj_repo.commit_in(&ws, "feat.txt", "feature", "Feature work");

    // Advance trunk in default workspace
    std::fs::write(jj_repo.root_path().join("trunk.txt"), "trunk advance\n").unwrap();
    run_jj_in(jj_repo.root_path(), &["describe", "-m", "Advance trunk"]);
    run_jj_in(jj_repo.root_path(), &["new"]);
    run_jj_in(
        jj_repo.root_path(),
        &["bookmark", "set", "main", "-r", "@-"],
    );

    // Now rebase feature onto the advanced trunk
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["rebase"],
        Some(&ws)
    ));
}

// ============================================================================
// wt step push tests
// ============================================================================

#[rstest]
fn test_jj_step_push_no_remote(mut jj_repo: JjTestRepo) {
    // Push without a remote — should complete (bookmark set) but push fails silently
    let ws = jj_repo.add_workspace("push-test");
    jj_repo.commit_in(&ws, "p.txt", "push content", "Push commit");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["push"], Some(&ws)));
}

#[rstest]
fn test_jj_step_push_nothing_to_push(jj_repo: JjTestRepo) {
    // Default workspace — feature tip IS trunk, nothing to push
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["push"], None));
}

#[rstest]
fn test_jj_step_push_behind_trunk(mut jj_repo: JjTestRepo) {
    // Create feature workspace with a commit
    let ws = jj_repo.add_workspace("push-behind");
    jj_repo.commit_in(&ws, "feat.txt", "feature", "Feature work");

    // Advance trunk past the feature (so feature is behind)
    std::fs::write(jj_repo.root_path().join("trunk.txt"), "trunk advance\n").unwrap();
    run_jj_in(jj_repo.root_path(), &["describe", "-m", "Advance trunk"]);
    run_jj_in(jj_repo.root_path(), &["new"]);
    run_jj_in(
        jj_repo.root_path(),
        &["bookmark", "set", "main", "-r", "@-"],
    );

    // Advance trunk again so it's strictly ahead
    std::fs::write(jj_repo.root_path().join("trunk2.txt"), "more trunk\n").unwrap();
    run_jj_in(jj_repo.root_path(), &["describe", "-m", "More trunk"]);
    run_jj_in(jj_repo.root_path(), &["new"]);
    run_jj_in(
        jj_repo.root_path(),
        &["bookmark", "set", "main", "-r", "@-"],
    );

    // Push from feature — should detect feature is not ahead and fail
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["push"], Some(&ws)));
}

// ============================================================================
// wt step squash edge cases
// ============================================================================

#[rstest]
fn test_jj_step_squash_single_commit_with_wc_content(mut jj_repo: JjTestRepo) {
    // Feature workspace with one commit AND uncommitted content in working copy
    let ws = jj_repo.add_workspace("squash-wc");
    jj_repo.commit_in(&ws, "first.txt", "first", "First commit");

    // Add more content without committing (jj auto-snapshots into @)
    std::fs::write(ws.join("extra.txt"), "uncommitted content\n").unwrap();

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["squash"],
        Some(&ws)
    ));
}

// ============================================================================
// wt step squash --show-prompt (jj routing)
// ============================================================================

#[rstest]
fn test_jj_step_squash_show_prompt(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("squash-prompt");
    jj_repo.commit_in(&ws, "p.txt", "content", "Commit for prompt");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "step",
        &["squash", "--show-prompt"],
        Some(&ws)
    ));
}

// ============================================================================
// Multi-step workflow tests
// ============================================================================

// ============================================================================
// Coverage gap tests — exercising uncovered code paths
// ============================================================================

/// Clean workspace should report as not dirty (workspace/jj.rs `is_dirty` clean path).
#[rstest]
fn test_jj_list_clean_workspace(jj_repo: JjTestRepo) {
    // Default workspace has an empty @ on top of trunk — is_dirty should return false
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

/// Switch to existing workspace without --cd should succeed silently
/// (handle_switch_jj.rs line 28: early return when change_dir is false).
#[rstest]
fn test_jj_switch_existing_no_cd(jj_repo_with_feature: JjTestRepo) {
    let mut cmd = jj_repo_with_feature.wt_command();
    cmd.args(["switch", "feature", "--no-cd"]);
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    // Should succeed without error, no directory change
}

/// Remove workspace by running `wt remove` from inside the workspace (no name arg)
/// (handle_remove_jj.rs line 19: empty names path).
#[rstest]
fn test_jj_remove_current_workspace_no_name(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("removeme");
    jj_repo.commit_in(&ws, "x.txt", "x", "commit");

    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "remove", &[], Some(&ws)));
}

/// Switch --create with --base creates workspace at specific revision
/// (workspace/jj.rs create_workspace with base parameter, line 290).
#[rstest]
fn test_jj_switch_create_with_base(jj_repo: JjTestRepo) {
    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "switch",
        &["based-ws", "--create", "--base", "main"],
        None,
    ));
}

/// List workspace with committed changes (exercises branch_diff_stats and ahead/behind).
#[rstest]
fn test_jj_list_workspace_with_commits(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("with-commits");
    jj_repo.commit_in(&ws, "a.txt", "a", "First change");
    jj_repo.commit_in(&ws, "b.txt", "b", "Second change");

    // List from default workspace — feature workspace should show commits ahead
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "list", &[], None));
}

/// Switch --create when target path already exists should error
/// (handle_switch_jj.rs lines 50-54).
#[rstest]
fn test_jj_switch_create_path_exists(jj_repo: JjTestRepo) {
    // Create the directory that would conflict
    let conflict_dir = jj_repo.root_path().parent().unwrap().join("repo.conflict");
    std::fs::create_dir_all(&conflict_dir).unwrap();

    assert_cmd_snapshot!(make_jj_snapshot_cmd(
        &jj_repo,
        "switch",
        &["conflict", "--create"],
        None,
    ));
}

/// List workspaces in JSON format (covers handle_list_jj JSON output path).
#[rstest]
fn test_jj_list_json(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("json-test");
    jj_repo.commit_in(&ws, "x.txt", "x content", "json test commit");

    let mut cmd = jj_repo.wt_command();
    configure_cli_command(&mut cmd);
    cmd.current_dir(jj_repo.root_path())
        .args(["list", "--format=json"]);
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt list --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = parsed.as_array().unwrap();
    // At least 2 workspaces: default + json-test
    assert!(arr.len() >= 2, "Expected at least 2 workspaces, got {}", arr.len());
    // Each item should have the 'kind' and 'branch' fields
    for item in arr {
        assert!(item.get("kind").is_some(), "Missing 'kind' field");
        assert!(item.get("branch").is_some(), "Missing 'branch' field");
    }
}

/// Remove workspace whose directory was already deleted externally
/// (handle_remove_jj.rs lines 68-75: "already removed" warning path).
#[rstest]
fn test_jj_remove_already_deleted_directory(mut jj_repo: JjTestRepo) {
    let ws = jj_repo.add_workspace("deleted");
    jj_repo.commit_in(&ws, "d.txt", "d", "commit");

    // Delete the directory externally before running wt remove
    std::fs::remove_dir_all(&ws).unwrap();

    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "remove", &["deleted"], None));
}

// ============================================================================
// wt step push tests (continued)
// ============================================================================

#[rstest]
fn test_jj_step_squash_then_push(mut jj_repo: JjTestRepo) {
    // The primary workflow: commit -> squash -> push
    let ws = jj_repo.add_workspace("sq-push");
    jj_repo.commit_in(&ws, "a.txt", "a", "First");
    jj_repo.commit_in(&ws, "b.txt", "b", "Second");

    // Squash
    let mut squash_cmd = jj_repo.wt_command();
    configure_cli_command(&mut squash_cmd);
    squash_cmd.current_dir(&ws).args(["step", "squash"]);
    let squash_result = squash_cmd.output().unwrap();
    assert!(
        squash_result.status.success(),
        "squash failed: {}",
        String::from_utf8_lossy(&squash_result.stderr)
    );

    // Push should still work (not say "nothing to push")
    assert_cmd_snapshot!(make_jj_snapshot_cmd(&jj_repo, "step", &["push"], Some(&ws)));
}
