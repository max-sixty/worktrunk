// Benchmarks for `wt remove` end-to-end performance
//
// Measures the full remove command including output rendering and hook spawning,
// to complement `time_to_first_output` which exits before output.
//
// Benchmark variants:
//   - remove_e2e/no_hooks       — remove with --no-verify (no hook loading)
//   - remove_e2e/with_hooks     — remove with hooks configured (user + project)
//   - remove_e2e/first_output   — baseline: exits before output (same as time_to_first_output)
//
// Run examples:
//   cargo bench --bench remove              # All variants
//   cargo bench --bench remove -- no_hooks  # Just no-hooks variant

use criterion::{Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::process::Command;
use worktrunk::shell_exec::Cmd;
use wt_perf::{RepoConfig, run_git, setup_fake_remote};

fn release_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_wt"))
}

/// Isolate a command from host environment (mirrors test configure_cli_command).
fn isolate_cmd(cmd: &mut Command, user_config_path: &Path) {
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") || key.starts_with("WORKTRUNK_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("SHELL");
    cmd.env("WORKTRUNK_CONFIG_PATH", user_config_path);
    cmd.env(
        "WORKTRUNK_SYSTEM_CONFIG_PATH",
        "/nonexistent/bench/system-config.toml",
    );
    cmd.env(
        "WORKTRUNK_APPROVALS_PATH",
        "/nonexistent/bench/approvals.toml",
    );
}

/// Create a benchmark repo at a specific path with optional hooks.
fn create_bench_repo(base_path: &Path, with_hooks: bool) -> PathBuf {
    let config = RepoConfig::typical(2); // main + 1 feature worktree
    wt_perf::create_repo_at(&config, base_path);
    setup_fake_remote(base_path);

    if with_hooks {
        // Project config with post-remove hook
        let config_dir = base_path.join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("wt.toml"),
            "[post-remove]\ndocs = \"echo post-remove-done\"\n",
        )
        .unwrap();
        run_git(base_path, &["add", "."]);
        run_git(base_path, &["commit", "-m", "Add project config"]);
    }

    base_path.to_path_buf()
}

/// Recreate the feature worktree after it was removed.
fn recreate_worktree(repo_path: &Path) {
    let wt_path = repo_path.parent().unwrap().join(format!(
        "{}.feature-wt-1",
        repo_path.file_name().unwrap().to_str().unwrap()
    ));

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
    let _ = Cmd::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .run();

    // Delete branch if it exists (may already be deleted by removal)
    let _ = Cmd::new("git")
        .args(["branch", "-D", "feature-wt-1"])
        .current_dir(repo_path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .run();

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
    let binary = release_binary();

    // Persistent temp dirs (kept alive for the benchmark group)
    let temp_no_hooks = tempfile::tempdir().unwrap();
    let temp_with_hooks = tempfile::tempdir().unwrap();

    let repo_no_hooks = create_bench_repo(&temp_no_hooks.path().join("repo"), false);
    let repo_with_hooks = create_bench_repo(&temp_with_hooks.path().join("repo"), true);

    // User config with post-switch hook (written beside repo)
    let user_config_no_hooks = temp_no_hooks.path().join("config.toml");
    std::fs::write(&user_config_no_hooks, "").unwrap();

    let user_config_with_hooks = temp_with_hooks.path().join("config.toml");
    std::fs::write(
        &user_config_with_hooks,
        "[hooks.post-switch]\nzellij-tab = \"echo post-switch-done\"\n",
    )
    .unwrap();

    let wt_name = |repo: &Path| -> PathBuf {
        repo.parent().unwrap().join(format!(
            "{}.feature-wt-1",
            repo.file_name().unwrap().to_str().unwrap()
        ))
    };

    // Baseline: first_output (exits before output rendering)
    group.bench_function("first_output", |b| {
        b.iter(|| {
            let mut cmd = Command::new(binary);
            cmd.args(["remove", "--yes", "--no-verify", "--force", "feature-wt-1"]);
            cmd.current_dir(&repo_no_hooks);
            isolate_cmd(&mut cmd, &user_config_no_hooks);
            cmd.env("WORKTRUNK_FIRST_OUTPUT", "1");
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "first_output failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        });
    });

    // No hooks: --no-verify (skip hook loading), run from feature worktree
    group.bench_function("no_hooks", |b| {
        b.iter_batched(
            || recreate_worktree(&repo_no_hooks),
            |()| {
                let wt_path = wt_name(&repo_no_hooks);
                let mut cmd = Command::new(binary);
                cmd.args(["remove", "--yes", "--no-verify", "--force"]);
                cmd.current_dir(&wt_path);
                isolate_cmd(&mut cmd, &user_config_no_hooks);
                let output = cmd.output().unwrap();
                assert!(
                    output.status.success(),
                    "no_hooks failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // With hooks: user post-switch + project post-remove
    group.bench_function("with_hooks", |b| {
        b.iter_batched(
            || recreate_worktree(&repo_with_hooks),
            |()| {
                let wt_path = wt_name(&repo_with_hooks);
                let mut cmd = Command::new(binary);
                cmd.args(["remove", "--yes", "--force"]);
                cmd.current_dir(&wt_path);
                isolate_cmd(&mut cmd, &user_config_with_hooks);
                let output = cmd.output().unwrap();
                assert!(
                    output.status.success(),
                    "with_hooks failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
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
