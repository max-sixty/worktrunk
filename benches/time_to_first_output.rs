// Benchmarks for time-to-first-output across wt commands
//
// Measures how long each command takes before showing any user-visible output.
// Uses WORKTRUNK_FIRST_OUTPUT env var to exit at the point of first output.
//
// Benchmark variants:
//   - first_output/remove
//   - first_output/switch
//   - first_output/list
//
// Run examples:
//   cargo bench --bench time_to_first_output            # All commands
//   cargo bench --bench time_to_first_output -- remove  # Just remove
//   cargo bench --bench time_to_first_output -- switch  # Just switch

use criterion::{Criterion, criterion_group, criterion_main};
use wt_perf::{
    CacheState, FixtureRecipe, bench_wt, run_and_check, standard_benchmark_profile, wt_command,
};

fn bench_first_output(c: &mut Criterion) {
    let mut group = c.benchmark_group("first_output");
    let binary = &worktrunk::testing::wt_bin();

    let fixture = FixtureRecipe::generated(3).create();

    let make_cmd = |args: &[&str]| {
        let mut cmd = wt_command(binary, fixture.path(), None);
        cmd.args(args);
        cmd.env("WORKTRUNK_FIRST_OUTPUT", "1");
        cmd
    };

    // remove: exits after validation, before approval/output.
    //
    // Cold matters here: `prepare_worktree_removal` calls
    // `compute_integration_lazy`, which populates
    // `.git/wt/cache/{is-ancestor,has-added-changes,merge-add-probe}` on the
    // first invocation. Without per-iteration invalidation the reported
    // timing would be warm cache, not the first-invocation TTFO a user sees.
    group.bench_function("remove", |b| {
        bench_wt(b, fixture.path(), CacheState::Cold, || {
            make_cmd(&["remove", "--yes", "--no-hooks", "--force", "wt-0000"])
        });
    });

    // switch: exits after execute_switch, before mismatch computation and output
    group.bench_function("switch", |b| {
        bench_wt(b, fixture.path(), CacheState::Warm, || {
            make_cmd(&["switch", "--yes", "--no-hooks", "wt-0000"])
        });
    });

    // list: stdout is piped here, so first output is the first buffered table
    // line after collection/render preparation, not the progressive skeleton.
    let output = run_and_check(&mut make_cmd(&["list"]));
    assert!(
        !output.stdout.is_empty(),
        "WORKTRUNK_FIRST_OUTPUT should emit the first stdout line"
    );
    group.bench_function("list", |b| {
        b.iter(|| run_and_check(&mut make_cmd(&["list"])));
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = standard_benchmark_profile();
    targets = bench_first_output
}
criterion_main!(benches);
