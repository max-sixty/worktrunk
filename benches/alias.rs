// Benchmarks for `wt <alias>` parent-side dispatch overhead
//
// Isolates the wall-clock cost of running an alias *before* the alias body
// does anything: config load, repo open, template context build, and the
// fork+exec of the child shell. Issue #2322 reports `wt <alias>` being
// dramatically slower than the equivalent subcommand; these benchmarks give
// that cost a regression-free measurement harness.
//
// Benchmark groups:
//   - noop_alias:        baseline `sh -c 'true'` and `wt --version` vs. `noop = "true"`
//   - worktree_scaling:  noop-ish alias (`echo hello`) at 1 and 4 worktrees (GH #2322)
//   - branch_scaling:    noop alias at 1, 10, 100 branches (regresses commit 4f9bd575a)
//
// Run examples:
//   cargo bench --bench alias                        # All groups
//   cargo bench --bench alias noop_alias             # Just parent-side overhead
//   cargo bench --bench alias branch_scaling         # Verify O(1) in branch count
//   cargo bench --bench alias -- --sample-size 10    # Fast iteration

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use wt_perf::{RepoConfig, create_repo, isolate_cmd};

/// Alias body is a shell builtin so the wall-clock is dominated by the
/// parent's dispatch — not by running a real subcommand.
const NOOP_CONFIG: &str = "[aliases]\nnoop = \"echo hello\"\n";

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

/// Baseline: raw shell fork/exec of `true` and `wt --version` startup, vs.
/// a noop alias (`true`) — the purest measurement of parent-side alias
/// dispatch (config load + repo open + template context build + fork+exec).
fn bench_noop_alias(c: &mut Criterion) {
    let mut group = c.benchmark_group("noop_alias");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    let temp = create_repo(&RepoConfig::typical(1));
    let repo_path = temp.path().join("repo");
    let user_config = temp.path().join("user-config.toml");
    std::fs::write(&user_config, NOOP_CONFIG).unwrap();

    group.bench_function("sh_true", |b| {
        b.iter(|| {
            Command::new("sh").args(["-c", "true"]).output().unwrap();
        });
    });

    group.bench_function("wt_version", |b| {
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.arg("--version");
            isolate_cmd(&mut cmd, Some(&user_config));
            run_and_check(cmd, "wt_version");
        });
    });

    group.bench_function("noop", |b| {
        b.iter(|| {
            run_and_check(wt_cmd(binary, &repo_path, &user_config, &["noop"]), "noop");
        });
    });

    group.finish();
}

/// Parent-side alias dispatch across worktree counts, from GH #2322. The
/// alias body is still a shell builtin — a real passthrough like `wt list`
/// would conflate parent-side dispatch with the child command's own cost.
/// 1 and 4 worktrees match `list.rs`'s scaling points so the two can be
/// read side-by-side.
fn bench_worktree_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("worktree_scaling");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for worktrees in [1, 4] {
        let temp = create_repo(&RepoConfig::typical(worktrees));
        let repo_path = temp.path().join("repo");
        let user_config = temp.path().join("user-config.toml");
        std::fs::write(&user_config, NOOP_CONFIG).unwrap();

        group.bench_with_input(
            BenchmarkId::from_parameter(worktrees),
            &worktrees,
            |b, _| {
                b.iter(|| {
                    run_and_check(
                        wt_cmd(binary, &repo_path, &user_config, &["noop"]),
                        "worktree_scaling",
                    );
                });
            },
        );
    }

    group.finish();
}

/// Regression test for commit 4f9bd575a. Before the fix, `fetch_all_upstreams`
/// ran a bulk `for-each-ref` in `build_hook_context`, making parent-side alias
/// setup O(branches). Post-fix, `Branch::upstream_single` reads a single
/// config value, so the curve across 1/10/100 branches should be flat.
fn bench_branch_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("branch_scaling");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for branches in [1usize, 10, 100] {
        // `RepoConfig::branches(N, 1)` creates N feature branches plus main,
        // matching list.rs's "many_branches" labeling convention.
        let temp = create_repo(&RepoConfig::branches(branches, 1));
        let repo_path = temp.path().join("repo");
        let user_config = temp.path().join("user-config.toml");
        std::fs::write(&user_config, NOOP_CONFIG).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(branches), &branches, |b, _| {
            b.iter(|| {
                run_and_check(
                    wt_cmd(binary, &repo_path, &user_config, &["noop"]),
                    "branch_scaling",
                );
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_noop_alias, bench_worktree_scaling, bench_branch_scaling
}
criterion_main!(benches);
