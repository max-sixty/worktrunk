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
//   - stub/1, stub/100: cache-independent alias at the population endpoints.
//   - with_vars/{warm,cold}/100: neighbors that add rev-parse, default-branch
//     detection, and primary worktree lookup, then isolate their cache cost.
//
// Run examples (Criterion takes a positional substring FILTER; no --skip):
//   cargo bench --bench alias                          # All variants
//   cargo bench --bench alias stub                     # Population endpoints
//   cargo bench --bench alias with_vars                # Cache pair
//   cargo bench --bench alias -- --sample-size 10      # Fast iteration

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{
    CacheState, FixtureRecipe, bench_wt, run_and_check, standard_benchmark_profile, wt_command,
};

const LARGE_WORKTREE_COUNT: usize = 100;

/// Both alias bodies are shell builtins so the wall-clock is dominated by the
/// parent's dispatch rather than by a real subcommand. `with_vars` references
/// variables that drive the expensive paths in
/// `build_hook_context`: `commit` (rev-parse), `default_branch` (cold
/// detection), and `primary_worktree_path` (lookup). The aliases have the same
/// no-output builtin and argument count.
const ALIAS_CONFIG: &str = r#"[aliases]
stub = ": fixed fixed fixed"
with_vars = ": {{ default_branch }} {{ commit }} {{ primary_worktree_path }}"
"#;

/// Build an isolated `wt` invocation pointed at a fixture user config.
fn wt_cmd(binary: &Path, repo: &Path, user_config: &Path, args: &[&str]) -> Command {
    let mut cmd = wt_command(binary, repo, Some(user_config));
    cmd.args(args);
    cmd
}

fn bench_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");
    let binary = &worktrunk::testing::wt_bin();

    // Startup floor: `wt --version` exits before any repo discovery, so the
    // delta between this and the scaling rows is the parent-side dispatch
    // cost (config load, repo open, template context build, fork+exec).
    group.bench_function("wt_version", |b| {
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.arg("--version");
            isolate_subprocess_env(&mut cmd, None);
            run_and_check(&mut cmd);
        });
    });

    for total_worktrees in [1, LARGE_WORKTREE_COUNT] {
        let fixture = FixtureRecipe::Generated {
            linked_worktrees: total_worktrees - 1,
            branchless_branches: 0,
            remote_tracking_refs: 0,
        }
        .create();
        let user_config = fixture.root().join("config.toml");
        std::fs::write(&user_config, ALIAS_CONFIG).unwrap();

        let mut cases = vec![("stub", "stub", CacheState::Warm)];
        // Template-variable and cache effects need one shared population.
        if total_worktrees == LARGE_WORKTREE_COUNT {
            cases.extend([
                ("with_vars/warm", "with_vars", CacheState::Warm),
                ("with_vars/cold", "with_vars", CacheState::Cold),
            ]);
        }

        for (id, alias_name, cache) in cases {
            group.bench_with_input(
                BenchmarkId::new(id, total_worktrees),
                &total_worktrees,
                |b, _| {
                    bench_wt(b, fixture.path(), cache, || {
                        wt_cmd(binary, fixture.path(), &user_config, &[alias_name])
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = standard_benchmark_profile();
    targets = bench_dispatch
}
criterion_main!(benches);
