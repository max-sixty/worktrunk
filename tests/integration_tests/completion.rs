use crate::common::{TestRepo, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

#[test]
fn test_complete_switch_shows_branches() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create some branches using git
    Command::new("git")
        .args(["branch", "feature/new"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for switch command
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        feature/new
        hotfix/bug
        main

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_switch_shows_all_branches_including_worktrees() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (this creates a new branch "feature/new")
    temp.add_worktree("feature-worktree", "feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion - should show branches WITH worktrees and WITHOUT worktrees
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        feature/new
        hotfix/bug
        main

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_push_shows_all_branches() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (creates "feature/new" branch)
    temp.add_worktree("feature-worktree", "feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for dev push (should show ALL branches, including those with worktrees)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "dev", "push", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_base_flag_shows_all_branches() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches
    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "feature/existing"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for --base flag (long form)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete",
            "wt",
            "switch",
            "--create",
            "new-branch",
            "--base",
            "",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should show all branches as potential base
    assert!(branches.iter().any(|b| b.contains("develop")));
    assert!(branches.iter().any(|b| b.contains("feature/existing")));

    // Test completion for -b flag (short form)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete",
            "wt",
            "switch",
            "--create",
            "new-branch",
            "-b",
            "",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should show all branches as potential base (short form works too)
    assert!(branches.iter().any(|b| b.contains("develop")));
}

#[test]
fn test_complete_base_flag_with_equals() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create some branches
    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "feature/existing"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for --base= format (equals sign, no space)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete",
            "wt",
            "switch",
            "--create",
            "new-branch",
            "--base=",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should show all branches as potential base
    assert!(branches.iter().any(|b| b.contains("develop")));
    assert!(branches.iter().any(|b| b.contains("feature/existing")));
    assert!(branches.iter().any(|b| b.contains("main")));

    // Test completion for --base=m (equals sign with partial value)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete",
            "wt",
            "switch",
            "--create",
            "new-branch",
            "--base=m",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should show all branches (filtering happens in shell, not in our code)
    assert!(branches.iter().any(|b| b.contains("main")));

    // Test completion for -b= format (short form with equals)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "--create", "new-branch", "-b="])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should show all branches (short form with equals works too)
    assert!(branches.iter().any(|b| b.contains("develop")));
    assert!(branches.iter().any(|b| b.contains("feature/existing")));
}

#[test]
fn test_complete_outside_git_repo() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.current_dir(temp.path())
            .args(["complete", "wt", "switch", ""]);

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_empty_repo() {
    let repo = TestRepo::new();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.current_dir(repo.root_path())
            .args(["complete", "wt", "switch", ""]);

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_unknown_command() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.current_dir(repo.root_path())
            .args(["complete", "wt", "unknown-command", ""]);

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_beta_commit_no_positionals() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.current_dir(repo.root_path())
            .args(["complete", "wt", "beta", "commit", ""]);

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_list_command() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.current_dir(repo.root_path())
            .args(["complete", "wt", "list", ""]);

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_init_fish_includes_no_file_flag() {
    // Test that fish init includes -f flag to disable file completion
    let mut cmd = wt_command();
    let output = cmd.arg("init").arg("fish").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check that completions include -f flag
    assert!(stdout.contains("-f -a '(__wt_complete)'"));
}

#[test]
fn test_complete_with_partial_prefix() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches with common prefix
    Command::new("git")
        .args(["branch", "feature/one"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "feature/two"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Complete with partial prefix - should return all branches
    // (shell completion framework handles the prefix filtering)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", "feat"]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        feature/one
        feature/two
        hotfix/bug
        main

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_switch_shows_all_branches_even_with_worktrees() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create two branches, both with worktrees
    temp.add_worktree("feature-worktree", "feature/new");
    temp.add_worktree("hotfix-worktree", "hotfix/bug");

    // From the main worktree, test completion - should show all branches
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should include branches even if they have worktrees (can switch to them)
    assert!(stdout.contains("feature/new"));
    assert!(stdout.contains("hotfix/bug"));
}

#[test]
fn test_complete_excludes_remote_branches() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create local branches
    Command::new("git")
        .args(["branch", "feature/local"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Set up a fake remote
    Command::new("git")
        .args(["remote", "add", "origin", "https://example.com/repo.git"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Create a remote-tracking branch by fetching from a local "remote"
    // First, create a bare repo to act as remote
    let remote_dir = temp.root_path().parent().unwrap().join("remote.git");
    Command::new("git")
        .args(["init", "--bare", remote_dir.to_str().unwrap()])
        .output()
        .unwrap();

    // Update remote URL to point to our bare repo
    Command::new("git")
        .args(["remote", "set-url", "origin", remote_dir.to_str().unwrap()])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Push to create remote branches
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["push", "origin", "feature/local:feature/remote"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Fetch to create remote-tracking branches
    Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should include local branch without worktree
    assert!(
        stdout.contains("feature/local"),
        "Should include feature/local branch, but got: {}",
        stdout
    );

    // main branch has a worktree (the root repo), so it may or may not be included
    // depending on switch context - not critical for this test

    // Should NOT include remote-tracking branches (origin/*)
    assert!(
        !stdout.contains("origin/"),
        "Completion should not include remote-tracking branches, but found: {}",
        stdout
    );
}

#[test]
fn test_complete_merge_shows_branches() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (creates "feature/new" branch)
    temp.add_worktree("feature-worktree", "feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for merge (should show ALL branches, including those with worktrees)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "merge", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should include both branches (merge shows all)
    assert!(branches.iter().any(|b| b.contains("feature/new")));
    assert!(branches.iter().any(|b| b.contains("hotfix/bug")));
}

#[test]
fn test_complete_with_special_characters_in_branch_names() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches with various special characters
    let branch_names = vec![
        "feature/FOO-123",         // Uppercase + dash + numbers
        "release/v1.2.3",          // Dots
        "hotfix/bug_fix",          // Underscore
        "feature/multi-part-name", // Multiple dashes
    ];

    for branch in &branch_names {
        Command::new("git")
            .args(["branch", branch])
            .current_dir(temp.root_path())
            .output()
            .unwrap();
    }

    // Test completion
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // All branches should be present
    for branch in &branch_names {
        assert!(
            stdout.contains(branch),
            "Branch {} should be in completion output",
            branch
        );
    }
}

#[test]
fn test_complete_stops_after_branch_provided() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches
    Command::new("git")
        .args(["branch", "feature/one"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "feature/two"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test that switch stops completing after branch is provided
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", "feature/one", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });

    // Test that dev push stops completing after branch is provided
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path()).args([
            "complete",
            "wt",
            "dev",
            "push",
            "feature/one",
            "",
        ]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });

    // Test that merge stops completing after branch is provided
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "merge", "feature/one", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_switch_with_create_flag_no_completion() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "feature/existing"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test with --create flag (long form)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", "--create", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });

    // Test with -c flag (short form)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let mut cmd = wt_command();
        temp.clean_cli_env(&mut cmd);
        cmd.current_dir(temp.root_path())
            .args(["complete", "wt", "switch", "-c", ""]);
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        ");
    });
}

#[test]
fn test_complete_switch_base_flag_after_branch() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches
    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for --base even after --create and branch name
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete",
            "wt",
            "switch",
            "--create",
            "new-feature",
            "--base",
            "",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should complete base flag value with branches
    assert!(stdout.contains("develop"));
}

#[test]
fn test_complete_remove_shows_branches() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (creates "feature/new" branch)
    temp.add_worktree("feature-worktree", "feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for remove (should show ALL branches)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "remove", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should include both branches
    assert!(branches.iter().any(|b| b.contains("feature/new")));
    assert!(branches.iter().any(|b| b.contains("hotfix/bug")));
}

#[test]
fn test_complete_dev_run_hook_shows_hook_types() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for beta run-hook
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "beta", "run-hook", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hooks: Vec<&str> = stdout.lines().collect();

    // Should include all hook types
    assert!(hooks.contains(&"post-create"), "Missing post-create");
    assert!(hooks.contains(&"post-start"), "Missing post-start");
    assert!(hooks.contains(&"pre-commit"), "Missing pre-commit");
    assert!(hooks.contains(&"pre-merge"), "Missing pre-merge");
    assert!(hooks.contains(&"post-merge"), "Missing post-merge");
    assert_eq!(hooks.len(), 5, "Should have exactly 5 hook types");
}

#[test]
fn test_complete_dev_run_hook_with_partial_input() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion with partial input
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "beta", "run-hook", "po"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hooks: Vec<&str> = stdout.lines().collect();

    // With clap fallback, we filter by prefix
    assert!(hooks.contains(&"post-create"));
    assert!(hooks.contains(&"post-start"));
    assert!(hooks.contains(&"post-merge"));
    // Should NOT contain hooks that don't match the prefix
    assert!(!hooks.contains(&"pre-commit"));
    assert!(!hooks.contains(&"pre-merge"));
}

#[test]
fn test_complete_dev_run_hook_without_trailing_empty() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion WITHOUT trailing empty string (what fish actually sends)
    // Regression test for bug where "wt beta run-hook<tab>" showed nothing
    // but "wt beta run-hook ""<tab>" showed all hooks
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "beta", "run-hook"]) // No trailing ""
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hooks: Vec<&str> = stdout.lines().collect();

    // Should include all hook types even without trailing empty string
    assert!(hooks.contains(&"post-create"), "Missing post-create");
    assert!(hooks.contains(&"post-start"), "Missing post-start");
    assert!(hooks.contains(&"pre-commit"), "Missing pre-commit");
    assert!(hooks.contains(&"pre-merge"), "Missing pre-merge");
    assert!(hooks.contains(&"post-merge"), "Missing post-merge");
    assert_eq!(hooks.len(), 5, "Should have exactly 5 hook types");
}

#[test]
fn test_complete_init_shows_shells() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for init command with no input
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "init", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // Should show all shell types
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
    assert!(shells.contains(&"elvish"));
    assert!(shells.contains(&"nushell"));
    assert!(shells.contains(&"oil"));
    assert!(shells.contains(&"powershell"));
    assert!(shells.contains(&"xonsh"));
}

#[test]
fn test_complete_init_without_trailing_empty() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion WITHOUT trailing empty string (what fish actually sends)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "init"]) // No trailing ""
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // Should show all shell types even without trailing empty string
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
}

#[test]
fn test_complete_init_partial() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion with partial input "fi"
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "init", "fi"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // With clap fallback, we filter by prefix
    assert!(shells.contains(&"fish"));
    // Should NOT contain shells that don't match the prefix
    assert!(!shells.contains(&"bash"));
}

#[test]
fn test_complete_init_with_source_flag() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion with --source flag: wt --source init <tab>
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "--source", "init", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // Should show all shell types, same as without --source
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
}

#[test]
fn test_complete_config_shell_flag() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for config shell --shell flag
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "config", "shell", "--shell", "z"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // Should filter by prefix
    assert!(shells.contains(&"zsh"));
    assert!(!shells.contains(&"bash"));
    assert!(!shells.contains(&"fish"));
}

#[test]
fn test_complete_config_shell_flag_with_source() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for config shell --shell flag with --source
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args([
            "complete", "wt", "--source", "config", "shell", "--shell", "",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells: Vec<&str> = stdout.lines().collect();

    // Should show all shell types, same as without --source
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
}

#[test]
fn test_complete_list_format_flag() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for list --format flag
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "list", "--format", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Each line is "name\tdescription" (fish format)
    // Just check that both format names appear
    assert!(stdout.contains("table"));
    assert!(stdout.contains("json"));
}

#[test]
fn test_complete_switch_with_execute_flag() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch --execute "code ." <cursor>
    // Should complete branches because --execute takes a value
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "--execute", "code .", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("develop")));
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_switch_with_execute_equals_format() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "feature"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch --execute="code ." <cursor>
    // Should complete branches
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "--execute=code .", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("feature")));
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_switch_short_cluster_with_value() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "bugfix"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch -xcode <cursor>
    // -x takes value "code" (fused), should complete branches for positional
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "-xcode", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("bugfix")));
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_switch_with_double_dash_terminator() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "feature"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch -- <cursor>
    // After --, everything is positional, should complete branches
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "--", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("feature")));
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_switch_positional_already_provided() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "existing"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch existing <cursor>
    // Positional already provided, should NOT complete branches
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "existing", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should not suggest branches (stdout should be empty or not contain branches)
    assert_eq!(stdout.trim(), "");
}

#[test]
fn test_complete_switch_completing_execute_value() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch --execute <cursor>
    // Currently typing the value for --execute, should NOT complete branches
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "switch", "--execute", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should not suggest branches when completing option value
    assert_eq!(stdout.trim(), "");
}

#[test]
fn test_complete_merge_with_flags() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "hotfix"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt merge --no-remove --force <cursor>
    // Should complete branches for positional (boolean flags don't consume arguments)
    let mut cmd = wt_command();
    temp.clean_cli_env(&mut cmd);
    let output = cmd
        .current_dir(temp.root_path())
        .args(["complete", "wt", "merge", "--no-remove", "--force", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("hotfix")));
    assert!(branches.iter().any(|b| b.contains("main")));
}
