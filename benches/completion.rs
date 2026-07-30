use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use wt_perf::{RepoConfig, create_repo, run_and_check, wt_command};

fn run_completion(binary: &Path, repo_path: &Path, words: &[&str]) {
    let index = words.len().saturating_sub(1);
    let mut cmd = wt_command(binary, repo_path, None);
    cmd.arg("--").args(words);
    cmd.env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("_CLAP_COMPLETE_COMP_TYPE", "9")
        .env("_CLAP_COMPLETE_SPACE", "true")
        .env("_CLAP_IFS", "\n");
    run_and_check(&mut cmd);
}

fn bench_completion_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    // Without worktrees: all branches are candidates
    group.bench_function("branches_only", |b| {
        let config = RepoConfig::branches(50, 0);
        let fixture = create_repo(&config);
        b.iter(|| run_completion(binary, fixture.path(), &["wt", "switch", ""]));
    });

    // With worktrees: filters out branches that already have worktrees
    group.bench_function("with_worktrees", |b| {
        let config = RepoConfig {
            worktrees: 10,
            ..RepoConfig::branches(50, 0)
        };
        let fixture = create_repo(&config);
        b.iter(|| run_completion(binary, fixture.path(), &["wt", "switch", ""]));
    });

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
