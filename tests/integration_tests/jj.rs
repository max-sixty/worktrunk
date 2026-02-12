//! Integration tests for jj (Jujutsu) workspace support.
//!
//! These tests exercise the `wt` CLI against real jj repositories.
//! They require `jj` to be installed (0.38.0+). Tests will fail if
//! `jj` is not available.

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
// jj availability gate
// ============================================================================

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Guard for use inside rstest fixtures that return a value.
/// Panics with a skip message if jj is not available.
fn ensure_jj_available() {
    if !jj_available() {
        panic!("jj is not installed — skipping jj integration tests");
    }
}

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

// ============================================================================
// rstest fixtures
// ============================================================================

#[fixture]
fn jj_repo() -> JjTestRepo {
    ensure_jj_available();
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
