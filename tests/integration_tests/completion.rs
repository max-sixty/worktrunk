use crate::common::{TestRepo, wt_command, wt_completion_command};
use insta::Settings;
use std::process::Command;

fn only_option_suggestions(stdout: &str) -> bool {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.starts_with('-'))
}

fn has_any_options(stdout: &str) -> bool {
    stdout.lines().any(|line| line.trim().starts_with('-'))
}

fn value_suggestions(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| {
            if line.is_empty() {
                false
            } else if line.starts_with('-') {
                line.contains('=')
            } else {
                true
            }
        })
        .collect()
}

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
        let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("feature/new"));
        assert!(stdout.contains("hotfix/bug"));
        assert!(stdout.contains("main"));
    });
}

#[test]
fn test_complete_switch_shows_all_branches_including_worktrees() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (this creates a new branch "feature/new")
    temp.add_worktree("feature/new");

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
        let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("feature/new"));
        assert!(stdout.contains("hotfix/bug"));
        assert!(stdout.contains("main"));
    });
}

#[test]
fn test_complete_push_shows_all_branches() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create worktree (creates "feature/new" branch)
    temp.add_worktree("feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for step push (should show ALL branches, including those with worktrees)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let output = temp
            .completion_cmd(&["wt", "step", "push", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let values = value_suggestions(&stdout);
        assert!(
            values.contains(&"feature/new"),
            "values should list feature/new\n{stdout}"
        );
        assert!(values.contains(&"hotfix/bug"));
        assert!(values.contains(&"main"));
    });
}

#[test]
fn test_complete_base_flag_all_formats() {
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

    // Test all base flag formats: --base, -b, --base=, -b=
    // For space-separated (--base ""), cursor is on empty arg after flag
    // For equals (--base=), cursor is completing the value after equals
    let test_cases: &[&[&str]] = &[
        &["wt", "switch", "--create", "new-branch", "--base", ""], // long form with space
        &["wt", "switch", "--create", "new-branch", "-b", ""],     // short form with space
        &["wt", "switch", "--create", "new-branch", "--base="],    // long form with equals
        &["wt", "switch", "--create", "new-branch", "-b="],        // short form with equals
    ];

    for args in test_cases {
        let output = temp.completion_cmd(args).output().unwrap();
        assert!(output.status.success(), "Failed for args: {:?}", args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let branches = value_suggestions(&stdout);

        assert!(
            branches.iter().any(|b| b.contains("develop")),
            "Missing develop for {:?}: {:?}",
            args,
            branches
        );
        assert!(
            branches.iter().any(|b| b.contains("feature/existing")),
            "Missing feature/existing for {:?}: {:?}",
            args,
            branches
        );
    }

    // Test partial completion --base=m (shell handles filtering, we return all)
    let output = temp
        .completion_cmd(&["wt", "switch", "--create", "new-branch", "--base=m"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches = value_suggestions(&stdout);
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_outside_git_repo() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let output = wt_completion_command(&["wt", "switch", ""])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .all(|line| line.starts_with('-')),
            "expected only option suggestions outside git repo, got:\n{stdout}"
        );
    });
}

#[test]
fn test_complete_empty_repo() {
    let repo = TestRepo::new();
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let output = repo.completion_cmd(&["wt", "switch", ""]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .all(|line| line.starts_with('-')),
            "expected only option suggestions in empty repo, got:\n{stdout}"
        );
    });
}

#[test]
fn test_complete_unknown_command() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let output = repo
            .completion_cmd(&["wt", "unknown-command", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let suggestions = value_suggestions(&stdout);
        assert!(
            suggestions.contains(&"config"),
            "should fall back to root completions, got:\n{stdout}"
        );
        assert!(suggestions.contains(&"list"));
    });
}

#[test]
fn test_complete_step_commit_no_positionals() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let output = repo
            .completion_cmd(&["wt", "step", "commit", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .all(|line| line.starts_with('-')),
            "step commit should only suggest flags, got:\n{stdout}"
        );
    });
}

#[test]
fn test_complete_list_command() {
    let repo = TestRepo::new();
    repo.commit("initial");
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let output = repo.completion_cmd(&["wt", "list", ""]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // wt list suggests flags (--format, --branches, etc.) and subcommands (statusline)
        assert!(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .all(|line| line.starts_with('-') || line == "statusline"),
            "wt list should only suggest flags or 'statusline' subcommand, got:\n{stdout}"
        );
    });
}

#[test]
fn test_init_fish_no_inline_completions() {
    // Test that fish init does NOT have inline completions (they're in a separate file)
    let mut cmd = wt_command();
    let output = cmd
        .arg("config")
        .arg("shell")
        .arg("init")
        .arg("fish")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify completions are NOT inline - they go to ~/.config/fish/completions/wt.fish
    assert!(
        !stdout.contains("complete --keep-order --exclusive --command wt --arguments"),
        "Fish init should NOT have inline completions (they go to separate file)"
    );
    // But should reference where completions are
    assert!(
        stdout.contains("Completions are in"),
        "Fish init should mention where completions are"
    );
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

    // Complete with partial prefix - shell does prefix filtering, we return all branches
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let output = temp
            .completion_cmd(&["wt", "switch", "feat"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("feature/one"));
        assert!(stdout.contains("feature/two"));
    });
}

#[test]
fn test_complete_switch_shows_all_branches_even_with_worktrees() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create two branches, both with worktrees
    temp.add_worktree("feature/new");
    temp.add_worktree("hotfix/bug");

    // From the main worktree, test completion - should show all branches
    let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();

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
    let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();

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
    temp.add_worktree("feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for merge (should show ALL branches, including those with worktrees)
    let output = temp.completion_cmd(&["wt", "merge", ""]).output().unwrap();

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
    let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let values = value_suggestions(&stdout);

    // All branches should be present
    for branch in &branch_names {
        assert!(
            values.contains(branch),
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
        let output = temp
            .completion_cmd(&["wt", "switch", "feature/one", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            only_option_suggestions(&stdout),
            "expected only option suggestions after positional provided, got:\n{stdout}"
        );
    });

    // Test that step push stops completing after branch is provided
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let output = temp
            .completion_cmd(&["wt", "step", "push", "feature/one", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            only_option_suggestions(&stdout),
            "expected only option suggestions after positional provided, got:\n{stdout}"
        );
    });

    // Test that merge stops completing after branch is provided
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let output = temp
            .completion_cmd(&["wt", "merge", "feature/one", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            only_option_suggestions(&stdout),
            "expected only option suggestions after positional provided, got:\n{stdout}"
        );
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
        let output = temp
            .completion_cmd(&["wt", "switch", "--create", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            only_option_suggestions(&stdout),
            "should not suggest branches when --create is present, got:\n{stdout}"
        );
    });

    // Test with -c flag (short form)
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(|| {
        let output = temp
            .completion_cmd(&["wt", "switch", "-c", ""])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            only_option_suggestions(&stdout),
            "should not suggest branches when -c is present, got:\n{stdout}"
        );
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
    let output = temp
        .completion_cmd(&["wt", "switch", "--create", "new-feature", "--base", ""])
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
    temp.add_worktree("feature/new");

    // Create another branch without worktree
    Command::new("git")
        .args(["branch", "hotfix/bug"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test completion for remove (should show ALL branches)
    let output = temp.completion_cmd(&["wt", "remove", ""]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();

    // Should include both branches
    assert!(branches.iter().any(|b| b.contains("feature/new")));
    assert!(branches.iter().any(|b| b.contains("hotfix/bug")));
}

#[test]
fn test_complete_step_subcommands() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test 1: No input - shows all step subcommands
    let output = temp.completion_cmd(&["wt", "step", ""]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let subcommands = value_suggestions(&stdout);
    // Operations
    assert!(subcommands.contains(&"commit"), "Missing commit");
    assert!(subcommands.contains(&"squash"), "Missing squash");
    assert!(subcommands.contains(&"push"), "Missing push");
    assert!(subcommands.contains(&"rebase"), "Missing rebase");
    // Hook types
    assert!(subcommands.contains(&"post-create"), "Missing post-create");
    assert!(subcommands.contains(&"post-start"), "Missing post-start");
    assert!(subcommands.contains(&"pre-commit"), "Missing pre-commit");
    assert!(subcommands.contains(&"pre-merge"), "Missing pre-merge");
    assert!(subcommands.contains(&"post-merge"), "Missing post-merge");
    assert!(subcommands.contains(&"pre-remove"), "Missing pre-remove");
    assert_eq!(
        subcommands.len(),
        10,
        "Should have exactly 10 step subcommands"
    );

    // Test 2: Partial input "po" - filters to post-* subcommands
    let output = temp.completion_cmd(&["wt", "step", "po"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let subcommands = value_suggestions(&stdout);
    assert!(subcommands.contains(&"post-create"));
    assert!(subcommands.contains(&"post-start"));
    assert!(subcommands.contains(&"post-merge"));
    assert!(!subcommands.contains(&"pre-commit"));
    assert!(!subcommands.contains(&"pre-merge"));
}

#[test]
fn test_complete_init_shell_all_variations() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test 1: No input - shows all supported shells
    let output = temp
        .completion_cmd(&["wt", "config", "shell", "init", ""])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells = value_suggestions(&stdout);
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
    assert!(!shells.contains(&"elvish"));
    assert!(!shells.contains(&"nushell"));

    // Test 2: Partial input "fi" - filters to fish
    let output = temp
        .completion_cmd(&["wt", "config", "shell", "init", "fi"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells = value_suggestions(&stdout);
    assert!(shells.contains(&"fish"));
    assert!(!shells.contains(&"bash"));

    // Test 3: Partial input "z" - filters to zsh
    let output = temp
        .completion_cmd(&["wt", "config", "shell", "init", "z"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells = value_suggestions(&stdout);
    assert!(shells.contains(&"zsh"));
    assert!(!shells.contains(&"bash"));
    assert!(!shells.contains(&"fish"));

    // Test 4: With --source flag - same behavior
    let output = temp
        .completion_cmd(&["wt", "--source", "config", "shell", "init", ""])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let shells = value_suggestions(&stdout);
    assert!(shells.contains(&"bash"));
    assert!(shells.contains(&"fish"));
    assert!(shells.contains(&"zsh"));
}

// test_complete_init_shell_all_with_source removed - duplicate of test_complete_init_shell_with_source_flag

#[test]
fn test_complete_list_format_flag() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test completion for list --format flag
    let output = temp
        .completion_cmd(&["wt", "list", "--format", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Each line is "name\tdescription" (fish format)
    // Just check that both format names appear
    let values = value_suggestions(&stdout);
    assert!(values.contains(&"table"));
    assert!(values.contains(&"json"));
}

#[test]
fn test_complete_switch_execute_all_formats() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "feature"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test all execute flag formats: --execute with space, --execute=, -xvalue
    // All should complete branches after the execute value is provided
    let test_cases: &[&[&str]] = &[
        &["wt", "switch", "--execute", "code .", ""], // --execute with space
        &["wt", "switch", "--execute=code .", ""],    // --execute= with equals
        &["wt", "switch", "-xcode", ""],              // -x fused short form
    ];

    for args in test_cases {
        let output = temp.completion_cmd(args).output().unwrap();
        assert!(output.status.success(), "Failed for args: {:?}", args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let branches: Vec<&str> = stdout.lines().collect();
        assert!(
            branches.iter().any(|b| b.contains("feature")),
            "Missing feature for {:?}: {:?}",
            args,
            branches
        );
        assert!(
            branches.iter().any(|b| b.contains("main")),
            "Missing main for {:?}: {:?}",
            args,
            branches
        );
    }
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
    let output = temp
        .completion_cmd(&["wt", "switch", "--", ""])
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
    let output = temp
        .completion_cmd(&["wt", "switch", "existing", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        only_option_suggestions(&stdout),
        "expected only option suggestions, got:\n{stdout}"
    );
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
    let output = temp
        .completion_cmd(&["wt", "switch", "--execute", ""])
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
    let output = temp
        .completion_cmd(&["wt", "merge", "--no-remove", "--force", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<&str> = stdout.lines().collect();
    assert!(branches.iter().any(|b| b.contains("hotfix")));
    assert!(branches.iter().any(|b| b.contains("main")));
}

#[test]
fn test_complete_switch_base_after_execute_equals() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches
    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "production"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch --create --execute=claude --base <cursor>
    // This is the reported failing case - should complete branches for --base
    let output = temp
        .completion_cmd(&["wt", "switch", "--create", "--execute=claude", "--base", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches = value_suggestions(&stdout);

    // Should show all branches as potential base
    assert!(
        branches.iter().any(|b| b.contains("develop")),
        "Should complete develop branch for --base flag, got:\n{stdout}"
    );
    assert!(
        branches.iter().any(|b| b.contains("production")),
        "Should complete production branch for --base flag, got:\n{stdout}"
    );
    assert!(
        branches.iter().any(|b| b.contains("main")),
        "Should complete main branch for --base flag, got:\n{stdout}"
    );
}

#[test]
fn test_complete_switch_flexible_argument_ordering() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "develop"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test that .last(true) allows positional before flags
    // wt switch feature --base <cursor>
    let output = temp
        .completion_cmd(&["wt", "switch", "feature", "--base", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches = value_suggestions(&stdout);

    // Should complete --base value even when positional comes first
    assert!(
        branches.iter().any(|b| b.contains("develop")),
        "Should complete branches for --base even after positional arg, got:\n{stdout}"
    );
    assert!(
        branches.iter().any(|b| b.contains("main")),
        "Should complete branches for --base even after positional arg, got:\n{stdout}"
    );
}

#[test]
fn test_complete_remove_flexible_argument_ordering() {
    let mut temp = TestRepo::new();
    temp.commit("initial");

    // Create two worktrees
    temp.add_worktree("feature");
    temp.add_worktree("bugfix");

    // Test that .last(true) allows positional before flags
    // wt remove feature --no-delete-branch <cursor>
    // Since remove accepts multiple worktrees, should suggest more worktrees
    let output = temp
        .completion_cmd(&["wt", "remove", "feature", "--no-delete-branch", ""])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let suggestions = value_suggestions(&stdout);

    // Should suggest additional worktrees (remove accepts Vec<String>)
    assert!(
        suggestions.iter().any(|s| s.contains("bugfix")),
        "Should suggest additional worktrees after positional and flag, got:\n{stdout}"
    );
}

#[test]
fn test_complete_filters_options_when_positionals_exist() {
    let temp = TestRepo::new();
    temp.commit("initial");

    Command::new("git")
        .args(["branch", "feature"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch <cursor>
    // Should show branches but NOT options like --config, --verbose, -C
    let output = temp.completion_cmd(&["wt", "switch", ""]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should have branch completions
    assert!(stdout.contains("feature"), "Should contain feature branch");
    assert!(stdout.contains("main"), "Should contain main branch");

    // Should NOT have options (they're filtered out when positionals exist)
    assert!(
        !has_any_options(&stdout),
        "Options should be filtered out when positional completions exist, got:\n{stdout}"
    );
}

#[test]
fn test_complete_subcommands_filter_options() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Test: wt <cursor>
    // Should show subcommands but NOT global options
    let output = temp.completion_cmd(&["wt", ""]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let suggestions = value_suggestions(&stdout);

    // Should have subcommands
    assert!(suggestions.contains(&"switch"), "Should contain switch");
    assert!(suggestions.contains(&"list"), "Should contain list");
    assert!(suggestions.contains(&"merge"), "Should contain merge");

    // Should NOT have global options
    assert!(
        !has_any_options(&stdout),
        "Global options should be filtered out at subcommand position, got:\n{stdout}"
    );

    // Test: wt --<cursor>
    // Now options SHOULD appear
    let output = temp.completion_cmd(&["wt", "--"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        has_any_options(&stdout),
        "Options should appear when explicitly completing with --, got:\n{stdout}"
    );
}

#[test]
fn test_complete_switch_option_prefix_shows_options_not_branches() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create branches that happen to contain "-c" in the name
    Command::new("git")
        .args(["branch", "fish-switch-complete"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["branch", "zsh-bash-complete"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch --c<cursor>
    // Should show options starting with --c (like --create), NOT branches containing "-c"
    let output = temp
        .completion_cmd(&["wt", "switch", "--c"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should NOT show branches (user is typing an option)
    assert!(
        !stdout.contains("fish-switch-complete"),
        "Should not show branches when completing options, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("zsh-bash-complete"),
        "Should not show branches when completing options, got:\n{stdout}"
    );

    // Should show options (--create, --config, etc.)
    assert!(
        only_option_suggestions(&stdout),
        "Should only show options when input starts with --, got:\n{stdout}"
    );
}

#[test]
fn test_complete_switch_single_dash_shows_options_not_branches() {
    let temp = TestRepo::new();
    temp.commit("initial");

    // Create a branch that contains "-" in the name
    Command::new("git")
        .args(["branch", "feature-branch"])
        .current_dir(temp.root_path())
        .output()
        .unwrap();

    // Test: wt switch -<cursor>
    // Should show short options, NOT branches containing "-"
    let output = temp
        .completion_cmd(&["wt", "switch", "-"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should NOT show branches
    assert!(
        !stdout.contains("feature-branch"),
        "Should not show branches when completing options, got:\n{stdout}"
    );

    // Should show options
    assert!(
        only_option_suggestions(&stdout),
        "Should only show options when input starts with -, got:\n{stdout}"
    );
}
