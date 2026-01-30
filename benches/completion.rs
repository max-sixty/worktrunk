use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use wt_perf::{RepoConfig, create_repo};

fn run_completion(binary: &Path, repo_path: &Path, words: &[&str]) {
    let index = words.len().saturating_sub(1);
    Command::new(binary)
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
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    // Without worktrees: all branches are candidates
    group.bench_function("branches_only", |b| {
        let config = RepoConfig {
            commits_on_main: 1,
            files: 1,
            branches: 50,
            commits_per_branch: 0,
            worktrees: 0,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        };
        let temp = create_repo(&config);
        let repo = temp.path().join("repo");
        b.iter(|| run_completion(binary, &repo, &["wt", "switch", ""]));
    });

    // With worktrees: filters out branches that already have worktrees
    group.bench_function("with_worktrees", |b| {
        let config = RepoConfig {
            commits_on_main: 1,
            files: 1,
            branches: 50,
            commits_per_branch: 0,
            worktrees: 10,
            worktree_commits_ahead: 0,
            worktree_uncommitted_files: 0,
        };
        let temp = create_repo(&config);
        let repo = temp.path().join("repo");
        b.iter(|| run_completion(binary, &repo, &["wt", "switch", ""]));
    });

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
