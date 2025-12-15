//! Helper binary to setup a test environment for debugging `wt select`
//!
//! This creates a temporary git repository with a known state:
//! - main branch with 3 commits
//! - feature branch with 15 commits ahead of main (100 lines per file)
//! - Multiple worktrees with varying amounts of work
//! - Some uncommitted changes in the feature worktree
//!
//! Run with: cargo run --bin setup-select-test
//! Then cd into the printed path and run: wt select

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Canonicalize path without Windows `\\?\` prefix.
fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

fn main() {
    println!("Setting up test environment for `wt select`...\n");

    // Create temp directory in /tmp
    let temp_base = std::env::temp_dir().join("wt-select-test");
    if temp_base.exists() {
        fs::remove_dir_all(&temp_base).unwrap();
    }
    fs::create_dir(&temp_base).unwrap();

    let root = temp_base.join("repo");
    fs::create_dir(&root).unwrap();
    let root = canonicalize(&root).unwrap();

    // Initialize git repo
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Test User"]);
    git(&root, &["config", "user.email", "test@example.com"]);

    // Create commits on main
    for i in 1..=3 {
        fs::write(
            root.join(format!("file{}.txt", i)),
            format!("content {}", i),
        )
        .unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", &format!("Main commit {}", i)]);
    }

    // Create feature branch with commits ahead - MORE COMMITS AND BIGGER FILES
    git(&root, &["switch", "-c", "feature"]);
    for i in 1..=15 {
        // Create larger files with more content
        let mut content = String::new();
        for line in 1..=100 {
            content.push_str(&format!("Line {} of feature file {}\n", line, i));
        }
        fs::write(root.join(format!("feature{}.txt", i)), content).unwrap();
        git(&root, &["add", "."]);
        git(
            &root,
            &[
                "commit",
                "-m",
                &format!("Feature commit {}: Add feature{}.txt with 100 lines", i, i),
            ],
        );
    }

    // Switch back to main in primary worktree BEFORE creating worktrees
    git(&root, &["switch", "main"]);

    // Create multiple worktrees to simulate realistic scenario

    // Feature worktree (15 commits ahead)
    let feature_wt = temp_base.join("feature-wt");
    git(
        &root,
        &["worktree", "add", feature_wt.to_str().unwrap(), "feature"],
    );

    // Create more branches with different amounts of work
    for (i, num_commits) in [(2, 5), (3, 3), (4, 8), (5, 2)].iter() {
        let branch_name = format!("branch{}", i);
        git(&root, &["branch", &branch_name, "main"]);

        let wt_path = temp_base.join(format!("branch{}-wt", i));
        git(
            &root,
            &["worktree", "add", wt_path.to_str().unwrap(), &branch_name],
        );

        // Add commits to this branch via its worktree
        for j in 1..=*num_commits {
            let file_content = format!("Content for {} commit {}\n", branch_name, j);
            fs::write(wt_path.join(format!("file{}.txt", j)), file_content).unwrap();
            let mut cmd = Command::new("git");
            git_env(&mut cmd);
            cmd.args(["add", "."])
                .current_dir(&wt_path)
                .output()
                .unwrap();

            let mut cmd = Command::new("git");
            git_env(&mut cmd);
            cmd.args(["commit", "-m", &format!("{} commit {}", branch_name, j)])
                .current_dir(&wt_path)
                .output()
                .unwrap();
        }
    }

    // Add uncommitted changes to feature worktree
    let feature_wt = canonicalize(&feature_wt).unwrap();
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted changes").unwrap();

    // Create some branches without worktrees
    git(&root, &["branch", "no-worktree-1", "main"]);
    git(&root, &["branch", "no-worktree-2", "main"]);

    println!("✅ Test environment ready!\n");
    println!("Repository path: {}", root.display());
    println!();
    println!("Worktrees created:");
    println!("  - main (primary): {}", root.display());
    println!("  - feature: 15 commits ahead, 1 uncommitted file");
    println!("  - branch2: 5 commits ahead");
    println!("  - branch3: 3 commits ahead");
    println!("  - branch4: 8 commits ahead");
    println!("  - branch5: 2 commits ahead");
    println!();
    println!("Branches without worktrees:");
    println!("  - no-worktree-1, no-worktree-2");
    println!();
    println!("Total: 6 worktrees + 2 branches = 8 items in list");
    println!();
    println!("To test:");
    println!();
    println!("Method 1 - Using cd:");
    println!("  cd {}", root.display());
    println!("  cargo run --quiet -- select");
    println!();
    println!("Method 2 - Using -C flag:");
    println!("  cargo run --quiet -- -C {} select", root.display());
    println!();
    println!("Expected behavior:");
    println!("  - Navigate with arrow keys through 8 items");
    println!("  - Press 3 on 'feature': Shows LARGE diff (15 files, 1500 lines)");
    println!("  - Press 3 on other branches: Shows smaller diffs");
    println!();
    println!("Press Enter to clean up and exit (or Ctrl+C to leave the test repo)...");

    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    println!("Cleaning up...");
    if let Err(e) = fs::remove_dir_all(&temp_base) {
        eprintln!("Warning: Failed to clean up test directory: {}", e);
        eprintln!("You may need to manually remove: {}", temp_base.display());
    } else {
        println!("Test environment cleaned up.");
    }
}

fn git_env(cmd: &mut Command) {
    // Set git config for test environment
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
    cmd.env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z");
    cmd.env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z");
}

fn git(repo: &PathBuf, args: &[&str]) {
    let mut cmd = Command::new("git");
    git_env(&mut cmd);
    let output = cmd.args(args).current_dir(repo).output().unwrap();

    if !output.status.success() {
        eprintln!("Git command failed: git {}", args.join(" "));
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }
}
