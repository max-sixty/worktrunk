// Benchmarks for `wt <alias>` parent-side dispatch overhead
//
// Isolates the wall-clock cost of running an alias *before* the alias body
// does anything: config load, repo open, template context build, and the
// fork+exec of the child shell. Issue #2322 reports `wt <alias>` being
// dramatically slower than the equivalent subcommand; these benchmarks give
// that cost a regression-free measurement harness.
//
// One group (`dispatch`), nine variants:
//   - wt_version:  `wt --version` startup floor (no repo discovery)
//   - warm/1, warm/100, cold/1, cold/100: stub alias at 1 and 100 worktrees,
//     warm and cold caches. Each worktree has its own branch, so 100
//     worktrees ≈ 101 branches — this doubles as the regression guard for
//     the O(1) upstream lookup from 4f9bd575a. The cold/100 variant is
//     where a regression to the pre-fix bulk `for-each-ref` would hurt
//     most (packed-refs scan dominates).
//   - with_vars/{warm,cold}/{1,100}: same axes, but the alias body
//     references several template vars that drive expensive paths inside
//     `build_hook_context` (rev-parse for `commit`, `default_branch`
//     detection, `primary_worktree` lookup, upstream lookup). Pairs with
//     stub to bracket the dispatch curve once a future filter scopes
//     context build to referenced vars only — until then with_vars tracks
//     stub within noise, since context build is unconditional.
//
// Run examples (Criterion takes a positional substring FILTER; no --skip):
//   cargo bench --bench alias                          # All variants
//   cargo bench --bench alias warm                     # Warm only (incl. with_vars/warm)
//   cargo bench --bench alias dispatch/warm            # Stub-warm only (excludes with_vars)
//   cargo bench --bench alias dispatch/cold            # Stub-cold only (excludes with_vars)
//   cargo bench --bench alias -- --sample-size 10      # Fast iteration

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{
    CacheState, FixtureRecipe, bench_wt, run_and_check, standard_benchmark_profile, wt_command,
};

/// Alias body is a shell builtin so the wall-clock is dominated by the
/// parent's dispatch — not by running a real subcommand.
const STUB_CONFIG: &str = "[aliases]\nstub = \": fixed fixed fixed fixed fixed fixed\"\n";

/// Alias body references vars that drive the expensive paths in
/// `build_hook_context`: `commit` / `short_commit` (`rev-parse <branch>`
/// fork), `default_branch` (cold detection), `primary_worktree_path`
/// (lookup). `branch` and `worktree_path` are nearly free but kept so a
/// future filter sees them as "referenced". `upstream` is omitted because
/// fixture branches have no configured upstream — adding one would change
/// what the stub variant measures and break label continuity. Both aliases
/// use the same no-output builtin and argument count.
const WITH_VARS_CONFIG: &str = r#"[aliases]
with_vars = ": {{ branch }} {{ default_branch }} {{ commit }} {{ short_commit }} {{ primary_worktree_path }} {{ worktree_path }}"
"#;

/// Build an isolated `wt` invocation pointed at a fixture user config.
fn wt_cmd(binary: &Path, repo: &Path, user_config: &Path, args: &[&str]) -> Command {
    let mut cmd = wt_command(binary, repo, Some(user_config));
    cmd.args(args);
    cmd
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
            isolate_subprocess_env(&mut cmd, None);
            run_and_check(&mut cmd);
        });
    });

    for total_worktrees in [1usize, 100] {
        let fixture = FixtureRecipe::Minimal {
            branchless_branches: 0,
            linked_worktrees: total_worktrees - 1,
        }
        .create();
        let stub_config = fixture.root().join("stub-config.toml");
        let vars_config = fixture.root().join("with-vars-config.toml");
        std::fs::write(&stub_config, STUB_CONFIG).unwrap();
        std::fs::write(&vars_config, WITH_VARS_CONFIG).unwrap();

        // Stub variants keep their label structure (`dispatch/{warm,cold}/{1,100}`)
        // so nightly bench history stays continuous. with_vars sits under a
        // `with_vars/` prefix as new siblings.
        for (id_prefix, alias_name, user_config) in [
            (None, "stub", &stub_config),
            (Some("with_vars"), "with_vars", &vars_config),
        ] {
            for cache in CacheState::WARM_AND_COLD {
                let cache_label = cache.label();
                let id = match id_prefix {
                    Some(prefix) => format!("{prefix}/{cache_label}"),
                    None => cache_label.to_string(),
                };

                group.bench_with_input(
                    BenchmarkId::new(id, total_worktrees),
                    &total_worktrees,
                    |b, _| {
                        // Cold matters here: `build_hook_context` resolves the
                        // default branch, which writes `worktrunk.default-branch`
                        // and would otherwise be a cache hit on iters 2-N.
                        bench_wt(b, fixture.path(), cache, || {
                            wt_cmd(binary, fixture.path(), user_config, &[alias_name])
                        });
                    },
                );
            }
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
