// Benchmarks for `wt remove` end-to-end performance
//
// Measures the full remove command including output rendering and hook
// spawning, to complement `first_output/remove` in `time_to_first_output`,
// which exits before output.
//
// Benchmark variants:
//   - remove_e2e/no_hooks       — remove with --no-hooks (no hook loading)
//   - remove_e2e/with_hooks     — remove with hooks configured (user + project)
//
// Run examples:
//   cargo bench --bench remove              # All variants
//   cargo bench --bench remove -- no_hooks  # Just no-hooks variant

use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use wt_perf::{
    FixtureRepo, RepoConfig, create_repo, linked_worktree_path, run_and_check, run_git, run_git_ok,
    wt_command,
};

/// Create an owned benchmark repo with optional hooks.
fn create_bench_repo(with_hooks: bool) -> FixtureRepo {
    let config = RepoConfig::typical(2); // main + 1 feature worktree
    let fixture = create_repo(&config);

    if with_hooks {
        // Project config with post-remove hook
        let config_dir = fixture.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("wt.toml"),
            "[post-remove]\ndocs = \"echo post-remove-done\"\n",
        )
        .unwrap();
        run_git(fixture.path(), &["add", "."]);
        run_git(fixture.path(), &["commit", "-m", "Add project config"]);
    }

    fixture
}

/// Recreate the feature worktree after it was removed.
fn recreate_worktree(repo_path: &Path) {
    let wt_path = linked_worktree_path(repo_path, "feature-wt-1");

    // Wait briefly for background removal to finish (sleep 1 + rm -rf in detached process).
    // Without this, the background rmdir/rm-rf races with worktree recreation.
    std::thread::sleep(std::time::Duration::from_millis(1200));

    // Clean up any leftover directory (placeholder or staged trash)
    let _ = std::fs::remove_dir_all(&wt_path);

    // Clean up trash directory from staged removals
    let trash_dir = repo_path.join(".git/wt/trash");
    if trash_dir.exists() {
        let _ = std::fs::remove_dir_all(&trash_dir);
    }

    // Prune stale worktree metadata (best-effort)
    let _ = run_git_ok(repo_path, &["worktree", "prune"]);

    // Delete branch if it exists (may already be deleted by removal)
    let _ = run_git_ok(repo_path, &["branch", "-D", "feature-wt-1"]);

    // Recreate branch + worktree
    run_git(
        repo_path,
        &[
            "worktree",
            "add",
            "-b",
            "feature-wt-1",
            wt_path.to_str().unwrap(),
            "HEAD",
        ],
    );
}

fn bench_remove_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_e2e");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    let repo_no_hooks = create_bench_repo(false);
    let repo_with_hooks = create_bench_repo(true);

    // User config with post-switch hook (written beside repo)
    let user_config_no_hooks = repo_no_hooks.root().join("config.toml");
    std::fs::write(&user_config_no_hooks, "").unwrap();

    let user_config_with_hooks = repo_with_hooks.root().join("config.toml");
    std::fs::write(
        &user_config_with_hooks,
        "[hooks.post-switch]\nzellij-tab = \"echo post-switch-done\"\n",
    )
    .unwrap();

    // No hooks: --no-hooks (skip hook loading), run from feature worktree
    group.bench_function("no_hooks", |b| {
        b.iter_batched(
            || recreate_worktree(repo_no_hooks.path()),
            |()| {
                let wt_path = repo_no_hooks.worktree_path("feature-wt-1");
                let mut cmd = wt_command(binary, &wt_path, Some(&user_config_no_hooks));
                cmd.args(["remove", "--yes", "--no-hooks", "--force"]);
                run_and_check(&mut cmd);
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // With hooks: user post-switch + project post-remove
    group.bench_function("with_hooks", |b| {
        b.iter_batched(
            || recreate_worktree(repo_with_hooks.path()),
            |()| {
                let wt_path = repo_with_hooks.worktree_path("feature-wt-1");
                let mut cmd = wt_command(binary, &wt_path, Some(&user_config_with_hooks));
                cmd.args(["remove", "--yes", "--force"]);
                run_and_check(&mut cmd);
            },
            criterion::BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(std::time::Duration::from_secs(20))
        .warm_up_time(std::time::Duration::from_secs(3));
    targets = bench_remove_e2e
}
criterion_main!(benches);
