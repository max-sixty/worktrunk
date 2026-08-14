// Benchmarks for `wt remove` end-to-end performance
//
// Measures the full remove command including output rendering and hook
// spawning, to complement `first_output/remove` in `time_to_first_output`,
// which exits before output.
//
// Three single-factor variants use the same project and user hook configuration:
//   - remove_e2e/warm/no_hooks — steady-state removal with hooks bypassed
//   - remove_e2e/cold/no_hooks — fresh-state neighbor for the cache cost
//   - remove_e2e/warm/with_hooks — warm neighbor for the hook cost
//
// Run examples:
//   cargo bench --bench remove              # All variants
//   cargo bench --bench remove -- no_hooks  # Just no-hooks variant

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use wt_perf::{
    CacheState, FixtureRecipe, FixtureRepo, add_heterogeneous_linked_worktrees,
    invalidate_caches_auto, invalidate_probe_caches, linked_worktree_path, run_and_check, run_git,
    run_git_ok, setup_fake_remote, wt_command,
};

const BRANCH: &str = "wt-0000";
const UNTRACKED_FILES: usize = 3;
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

struct RemoveFixture {
    repo: FixtureRepo,
    user_config: PathBuf,
    project_marker: PathBuf,
    user_marker: PathBuf,
    worktree_admin_dir: PathBuf,
}

impl RemoveFixture {
    /// Build one complete candidate outside the measured duration.
    fn create(binary: &Path, cache: CacheState) -> Self {
        let repo = FixtureRecipe::generated(0).create();
        let project_marker = repo.root().join("project-post-remove.marker");
        let user_marker = repo.root().join("user-post-switch.marker");

        let config_dir = repo.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("wt.toml"),
            format!(
                "[post-remove]\nbenchmark = \"{}\"\n",
                marker_command(&project_marker, "project")
            ),
        )
        .unwrap();
        run_git(repo.path(), &["add", ".config/wt.toml"]);
        run_git(repo.path(), &["commit", "-m", "Add benchmark hook"]);

        add_heterogeneous_linked_worktrees(repo.path(), 1);
        setup_fake_remote(repo.path());
        let worktree = repo.worktree_path(BRANCH);
        for i in 0..UNTRACKED_FILES {
            std::fs::write(
                worktree.join(format!("uncommitted_{i}.txt")),
                "Uncommitted content\n",
            )
            .unwrap();
        }
        let worktree_admin_dir = linked_worktree_admin_dir(&repo.worktree_path(BRANCH));

        let user_config = repo.root().join("config.toml");
        std::fs::write(
            &user_config,
            format!(
                "[post-switch]\nbenchmark = \"{}\"\n",
                marker_command(&user_marker, "user")
            ),
        )
        .unwrap();

        let fixture = Self {
            repo,
            user_config,
            project_marker,
            user_marker,
            worktree_admin_dir,
        };
        for i in 0..UNTRACKED_FILES {
            assert!(
                fixture
                    .worktree_path()
                    .join(format!("uncommitted_{i}.txt"))
                    .is_file(),
                "remove fixture untracked payload {i} is missing"
            );
        }
        fixture.prewarm(binary);
        match cache {
            CacheState::Warm => {}
            CacheState::Cold => invalidate_caches_auto(fixture.repo.path()),
            CacheState::ProbeCold => invalidate_probe_caches(fixture.repo.path()),
        }
        fixture
    }

    fn worktree_path(&self) -> PathBuf {
        linked_worktree_path(self.repo.path(), BRANCH)
    }

    /// Prewarm the integration probes without consuming the unmerged candidate.
    fn prewarm(&self, binary: &Path) {
        let mut cmd = wt_command(binary, self.repo.path(), Some(&self.user_config));
        cmd.args([
            "step",
            "prune",
            "--dry-run",
            "--min-age",
            "0s",
            "--format",
            "json",
        ]);
        run_and_check(&mut cmd);
    }

    /// Wait for current-worktree cleanup and detached hooks to settle, then
    /// check both the destructive result and the hook-control contract.
    fn assert_consumed(&self, expect_hooks: bool) {
        let worktree = self.worktree_path();
        wait_for("remove background cleanup and hooks", || {
            let hooks_finished =
                !expect_hooks || (self.project_marker.is_file() && self.user_marker.is_file());
            !worktree.exists() && hooks_finished
        });

        assert!(
            run_git_ok(
                self.repo.path(),
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{BRANCH}")
                ]
            ),
            "measured remove deleted the retained candidate branch"
        );
        assert!(
            !self.worktree_admin_dir.exists(),
            "measured remove left the worktree registration behind"
        );

        if expect_hooks {
            assert_eq!(
                std::fs::read_to_string(&self.project_marker).unwrap(),
                "project"
            );
            assert_eq!(std::fs::read_to_string(&self.user_marker).unwrap(), "user");
        } else {
            assert!(
                !self.project_marker.exists() && !self.user_marker.exists(),
                "--no-hooks unexpectedly produced a hook marker"
            );
        }
    }
}

fn marker_command(path: &Path, value: &str) -> String {
    format!(
        "printf {value} > {}",
        shell_escape::unix::escape(path.to_string_lossy())
    )
}

fn linked_worktree_admin_dir(worktree: &Path) -> PathBuf {
    let dot_git = std::fs::read_to_string(worktree.join(".git")).unwrap();
    let git_dir = dot_git
        .trim()
        .strip_prefix("gitdir: ")
        .expect("linked worktree .git file must contain a gitdir");
    PathBuf::from(git_dir)
}

fn wait_for(label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + BACKGROUND_TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn bench_variant(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    expect_hooks: bool,
    cache: CacheState,
) {
    let binary = &worktrunk::testing::wt_bin();

    group.bench_function(name, |b| {
        b.iter_custom(|iterations| {
            let mut measured = Duration::ZERO;
            for _ in 0..iterations {
                let fixture = RemoveFixture::create(binary, cache);
                let worktree = fixture.worktree_path();
                let mut cmd = wt_command(binary, &worktree, Some(&fixture.user_config));
                cmd.args(["remove", "--yes", "--force"]);
                if !expect_hooks {
                    cmd.arg("--no-hooks");
                }

                let started = Instant::now();
                run_and_check(&mut cmd);
                measured += started.elapsed();
                fixture.assert_consumed(expect_hooks);
            }
            measured
        });
    });
}

fn bench_remove_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_e2e");

    bench_variant(&mut group, "warm/no_hooks", false, CacheState::Warm);
    bench_variant(&mut group, "cold/no_hooks", false, CacheState::Cold);
    bench_variant(&mut group, "warm/with_hooks", true, CacheState::Warm);

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(std::time::Duration::from_secs(3))
        .warm_up_time(std::time::Duration::from_secs(1));
    targets = bench_remove_e2e
}
criterion_main!(benches);
