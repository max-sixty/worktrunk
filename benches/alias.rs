// Benchmarks for `wt <alias>` parent-side dispatch overhead
//
// Isolates the wall-clock cost of running an alias *before* the alias body
// does anything: config load, repo open, template context build, and the
// fork+exec of the child shell. Issue #2322 reports `wt <alias>` being
// dramatically slower than the equivalent subcommand; these benchmarks give
// that cost a regression-free measurement harness.
//
// One group (`dispatch`), variants:
//   - wt_version:  `wt --version` startup floor (no repo discovery)
//   - {noop,commit,everything}/{warm,cold}/{1,100}: alias body variants that
//     reference zero, one, and all expensive template vars. The three
//     variants exercise the gating in `build_hook_context` — `noop` skips
//     both rev-parse and for-each-ref, `commit` keeps rev-parse, `everything`
//     keeps both. Each worktree has its own branch, so 100 worktrees ≈
//     101 branches — this doubles as the regression guard for the O(1)
//     upstream lookup from 4f9bd575a. The cold/100 variant is where a
//     regression to the pre-fix bulk `for-each-ref` would hurt most
//     (packed-refs scan dominates).
//
// Run examples:
//   cargo bench --bench alias                          # All variants
//   cargo bench --bench alias -- noop                  # Just the noop rows
//   cargo bench --bench alias -- --sample-size 10      # Fast iteration

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use wt_perf::{RepoConfig, create_repo, invalidate_caches_auto, isolate_cmd};

/// Three aliases whose templates reference different slices of the hook
/// context, exposing whether `build_hook_context`'s accessors fire.
///
/// - `noop` — zero referenced vars; all gated accessors (rev-parse for
///   `{{ commit }}`, for-each-ref for `{{ upstream }}`) should skip.
/// - `commit` — references `{{ commit }}`; rev-parse fires, for-each-ref
///   still skipped.
/// - `everything` — references every gated var; both accessors fire, matching
///   the pre-gating baseline cost. The `| default` filters keep the template
///   valid even when the bench repo has no remote/upstream — referenced-var
///   detection is AST-level, so the gating decisions are unaffected.
const ALIAS_CONFIG: &str = r#"[aliases]
noop = "echo hello"
commit = "echo {{ commit }}"
everything = "echo {{ commit }} {{ short_commit }} {{ upstream | default('') }} {{ remote | default('') }} {{ remote_url | default('') }} {{ owner | default('') }} {{ default_branch }} {{ primary_worktree_path }}"
"#;

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
        std::fs::write(&user_config, ALIAS_CONFIG).unwrap();

        for alias in ["noop", "commit", "everything"] {
            for cold in [false, true] {
                let cache = if cold { "cold" } else { "warm" };
                let id = BenchmarkId::new(format!("{alias}/{cache}"), worktrees);
                group.bench_with_input(id, &worktrees, |b, _| {
                    let run = || {
                        run_and_check(
                            wt_cmd(binary, &repo_path, &user_config, &[alias]),
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
