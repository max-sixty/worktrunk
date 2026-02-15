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
use wt_perf::{RepoConfig, create_repo, setup_fake_remote};

fn get_release_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_wt"))
}

/// Run a command and assert it succeeded.
fn run_bench_cmd(cmd: &mut Command) {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Benchmark command failed:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn bench_first_output(c: &mut Criterion) {
    let mut group = c.benchmark_group("first_output");
    let binary = get_release_binary();
    let env = ("WORKTRUNK_FIRST_OUTPUT", "1");

    let config = RepoConfig::typical(4);
    let temp = create_repo(&config);
    let repo_path = temp.path().join("repo");
    setup_fake_remote(&repo_path);

    // remove: exits after validation, before approval/output
    group.bench_function("remove", |b| {
        b.iter(|| {
            run_bench_cmd(
                Command::new(binary)
                    .args(["remove", "--yes", "--no-verify", "--force", "feature-wt-1"])
                    .current_dir(&repo_path)
                    .env(env.0, env.1),
            );
        });
    });

    // switch: exits after execute_switch, before output
    group.bench_function("switch", |b| {
        b.iter(|| {
            run_bench_cmd(
                Command::new(binary)
                    .args(["switch", "--yes", "--no-verify", "feature-wt-1"])
                    .current_dir(&repo_path)
                    .env(env.0, env.1),
            );
        });
    });

    // list: exits after skeleton data collection, before render
    group.bench_function("list", |b| {
        b.iter(|| {
            run_bench_cmd(
                Command::new(binary)
                    .arg("list")
                    .current_dir(&repo_path)
                    .env(env.0, env.1),
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
