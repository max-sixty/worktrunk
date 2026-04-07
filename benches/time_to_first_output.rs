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
use std::path::Path;
use std::process::Command;
use wt_perf::{RepoConfig, create_repo, isolate_cmd, setup_fake_remote};

fn bench_first_output(c: &mut Criterion) {
    let mut group = c.benchmark_group("first_output");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    let config = RepoConfig::typical(4);
    let temp = create_repo(&config);
    let repo_path = temp.path().join("repo");
    setup_fake_remote(&repo_path);

    let make_cmd = |args: &[&str]| {
        let mut cmd = Command::new(binary);
        cmd.args(args).current_dir(&repo_path);
        isolate_cmd(&mut cmd, None);
        cmd.env("WORKTRUNK_FIRST_OUTPUT", "1");
        cmd
    };

    // remove: exits after validation, before approval/output
    group.bench_function("remove", |b| {
        b.iter(|| {
            let output = make_cmd(&["remove", "--yes", "--no-hooks", "--force", "feature-wt-1"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Benchmark command failed:\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        });
    });

    // switch: exits after execute_switch, before mismatch computation and output
    group.bench_function("switch", |b| {
        b.iter(|| {
            let output = make_cmd(&["switch", "--yes", "--no-hooks", "feature-wt-1"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Benchmark command failed:\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        });
    });

    // list: exits after skeleton data collection, before render
    group.bench_function("list", |b| {
        b.iter(|| {
            let output = make_cmd(&["list"]).output().unwrap();
            assert!(
                output.status.success(),
                "Benchmark command failed:\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(15))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_first_output
}
criterion_main!(benches);
