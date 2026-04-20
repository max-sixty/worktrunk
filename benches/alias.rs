// Benchmarks for `wt <alias>` parent-side dispatch overhead
//
// Isolates the wall-clock cost of running an alias *before* the alias body
// does anything: config load, repo open, template context build, and the
// fork+exec of the child shell. Issue #2322 reports `wt <alias>` being
// dramatically slower than the equivalent subcommand; these benchmarks give
// that cost a regression-free measurement harness.
//
// One group (`dispatch`), five variants:
//   - wt_version:  `wt --version` startup floor (no repo discovery)
//   - warm/1, warm/100, cold/1, cold/100: noop alias at 1 and 100 worktrees,
//     warm and cold caches. Each worktree has its own branch, so 100
//     worktrees ≈ 101 branches — this doubles as the regression guard for
//     the O(1) upstream lookup from 4f9bd575a. The cold/100 variant is
//     where a regression to the pre-fix bulk `for-each-ref` would hurt
//     most (packed-refs scan dominates).
//
// Run examples:
//   cargo bench --bench alias                          # All variants
//   cargo bench --bench alias -- --skip cold           # Warm only, faster
//   cargo bench --bench alias -- --sample-size 10      # Fast iteration

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use wt_perf::{RepoConfig, create_repo, invalidate_caches_auto, isolate_cmd};

/// Alias body is a shell builtin so the wall-clock is dominated by the
/// parent's dispatch — not by running a real subcommand.
const NOOP_CONFIG: &str = "[aliases]\nnoop = \"echo hello\"\n";

/// Lean repo config for the scaling rows — alias dispatch doesn't care
/// about commit history depth, so minimal everything keeps setup under
/// 10s at 100 worktrees (vs. ~60s for `RepoConfig::typical(100)`).
const fn lean_worktrees(worktrees: usize) -> RepoConfig {
    RepoConfig {
        commits_on_main: 1,
        files: 1,
        branches: 0,
        commits_per_branch: 0,
        worktrees,
        worktree_commits_ahead: 0,
        worktree_uncommitted_files: 0,
    }
}

/// Build an isolated `wt` invocation pointed at a fixture user config.
fn wt_cmd(binary: &Path, repo: &Path, user_config: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(args).current_dir(repo);
    isolate_cmd(&mut cmd, Some(user_config));
    cmd
}

/// Run a benchmark command and assert success, surfacing stderr on failure.
fn run_and_check(mut cmd: Command, label: &str) {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{label} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn bench_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    // Startup floor: `wt --version` exits before any repo discovery, so the
    // delta between this and the scaling rows is the parent-side dispatch
    // cost (config load, repo open, template context build, fork+exec).
    group.bench_function("wt_version", |b| {
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.arg("--version");
            isolate_cmd(&mut cmd, None);
            run_and_check(cmd, "wt_version");
        });
    });

    for worktrees in [1usize, 100] {
        let temp = create_repo(&lean_worktrees(worktrees));
        let repo_path = temp.path().join("repo");
        let user_config = temp.path().join("user-config.toml");
        std::fs::write(&user_config, NOOP_CONFIG).unwrap();

        for cold in [false, true] {
            let label = if cold { "cold" } else { "warm" };

            group.bench_with_input(BenchmarkId::new(label, worktrees), &worktrees, |b, _| {
                let run = || {
                    run_and_check(
                        wt_cmd(binary, &repo_path, &user_config, &["noop"]),
                        "dispatch",
                    );
                };
                if cold {
                    b.iter_batched(
                        || invalidate_caches_auto(&repo_path),
                        |_| run(),
                        criterion::BatchSize::SmallInput,
                    );
                } else {
                    b.iter(run);
                }
            });
        }
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_dispatch
}
criterion_main!(benches);
