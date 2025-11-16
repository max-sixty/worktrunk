use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Create a test git repository with specified number of branches and worktrees
fn setup_test_repo(num_branches: usize, num_worktrees: usize) -> TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path();

    // Initialize git repo
    Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // Configure git
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // Create initial commit
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // Create branches
    for i in 0..num_branches {
        Command::new("git")
            .args(["branch", &format!("branch-{}", i)])
            .current_dir(repo_path)
            .output()
            .unwrap();
    }

    // Create worktrees on separate branches
    for i in 0..num_worktrees {
        let worktree_path = repo_path.join(format!("wt-{}", i));
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &format!("wt-branch-{}", i),
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(repo_path)
            .output()
            .unwrap();
    }

    temp_dir
}

fn run_completion(repo_path: &Path, words: &[&str]) {
    let index = words.len().saturating_sub(1);
    Command::new("cargo")
        .args(["run", "--"])
        .env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("_CLAP_COMPLETE_COMP_TYPE", "9")
        .env("_CLAP_COMPLETE_SPACE", "true")
        .env("_CLAP_IFS", "\n")
        .arg("--")
        .args(words)
        .current_dir(repo_path)
        .output()
        .unwrap();
}

fn bench_completion_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch");

    // Benchmark with 10 branches
    group.bench_function("10_branches", |b| {
        let temp = setup_test_repo(10, 0);
        b.iter(|| run_completion(temp.path(), &["wt", "switch", ""]));
    });

    // Benchmark with 50 branches
    group.bench_function("50_branches", |b| {
        let temp = setup_test_repo(50, 0);
        b.iter(|| run_completion(temp.path(), &["wt", "switch", ""]));
    });

    // Benchmark with 100 branches
    group.bench_function("100_branches", |b| {
        let temp = setup_test_repo(100, 0);
        b.iter(|| run_completion(temp.path(), &["wt", "switch", ""]));
    });

    group.finish();
}

fn bench_completion_switch_with_worktrees(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch_filtered");

    // Benchmark with 50 branches, 10 with worktrees (tests filtering performance)
    group.bench_function("50_branches_10_worktrees", |b| {
        let temp = setup_test_repo(50, 10);
        b.iter(|| run_completion(temp.path(), &["wt", "switch", ""]));
    });

    group.finish();
}

fn bench_completion_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_push");

    // Benchmark push completion (shows all branches, no filtering)
    group.bench_function("100_branches", |b| {
        let temp = setup_test_repo(100, 0);
        b.iter(|| run_completion(temp.path(), &["wt", "push", ""]));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_completion_switch,
    bench_completion_switch_with_worktrees,
    bench_completion_push
);
criterion_main!(benches);
