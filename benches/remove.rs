// Benchmarks for `wt remove` end-to-end performance
//
// Measures the full remove command including output rendering and hook
// spawning, to complement `first_output/remove` in `time_to_first_output`,
// which exits before output.
//
// Both variants use the same project and user hook configuration:
//   - remove_e2e/no_hooks       — the hooks are bypassed with --no-hooks
//   - remove_e2e/with_hooks     — the hooks are approved and spawned
//
// Run examples:
//   cargo bench --bench remove              # All variants
//   cargo bench --bench remove -- no_hooks  # Just no-hooks variant

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use wt_perf::{
    FixtureRecipe, FixtureRepo, add_typical_linked_worktrees, linked_worktree_path, run_and_check,
    run_git, run_git_ok, setup_fake_remote, wt_command,
};

const BRANCH: &str = "feature-wt-1";
const TEMPLATE_REF: &str = "refs/wt-perf/remove-template";
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
    /// Build the full hook-bearing repository before the candidate branch
    /// forks, then retain its exact tip under a private ref. Every measured
    /// removal therefore consumes the same 10-commit worktree and three-file
    /// untracked payload while retaining its unmerged branch.
    fn create() -> Self {
        let repo = FixtureRecipe::Typical { total_worktrees: 1 }.create();
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

        add_typical_linked_worktrees(repo.path(), 1);
        setup_fake_remote(repo.path());
        run_git(
            repo.path(),
            &["update-ref", TEMPLATE_REF, &format!("refs/heads/{BRANCH}")],
        );
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

        Self {
            repo,
            user_config,
            project_marker,
            user_marker,
            worktree_admin_dir,
        }
    }

    fn worktree_path(&self) -> PathBuf {
        linked_worktree_path(self.repo.path(), BRANCH)
    }

    /// Prepare one equivalent candidate outside Criterion's timed routine.
    ///
    /// The initial candidate comes from fixture construction. Later calls
    /// first prove that the previous command removed its worktree registration,
    /// then check out the retained branch and restore its untracked payload.
    fn prepare(&self, previous_run: bool, expect_hooks: bool) {
        if previous_run {
            self.assert_consumed(expect_hooks);
            self.clear_markers();
            self.restore_candidate();
        } else {
            self.clear_markers();
            self.assert_candidate_present();
        }
    }

    fn restore_candidate(&self) {
        let worktree = self.worktree_path();
        run_git(
            self.repo.path(),
            &["worktree", "add", worktree.to_str().unwrap(), BRANCH],
        );
        for i in 0..UNTRACKED_FILES {
            std::fs::write(
                worktree.join(format!("uncommitted_{i}.txt")),
                "Uncommitted content\n",
            )
            .unwrap();
        }
        self.assert_candidate_present();
    }

    fn assert_candidate_present(&self) {
        assert!(
            self.worktree_path().is_dir(),
            "remove fixture candidate worktree is missing"
        );
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
            "remove fixture candidate branch is missing"
        );
        assert_same_commit(self.repo.path(), BRANCH, TEMPLATE_REF);
        assert!(
            self.worktree_admin_dir.is_dir(),
            "remove fixture worktree registration is missing"
        );
        for i in 0..UNTRACKED_FILES {
            assert!(
                self.worktree_path()
                    .join(format!("uncommitted_{i}.txt"))
                    .is_file(),
                "remove fixture untracked payload {i} is missing"
            );
        }
    }

    /// Wait for current-worktree cleanup and detached hooks to settle, then
    /// check both the destructive result and the hook-control contract.
    fn assert_consumed(&self, expect_hooks: bool) {
        let worktree = self.worktree_path();
        wait_for("remove background cleanup and hooks", || {
            let hooks_finished = !expect_hooks
                || (marker_has_contents(&self.project_marker, "project")
                    && marker_has_contents(&self.user_marker, "user"));
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
        assert_same_commit(self.repo.path(), BRANCH, TEMPLATE_REF);
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

    fn clear_markers(&self) {
        remove_if_exists(&self.project_marker);
        remove_if_exists(&self.user_marker);
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

/// Mutual ancestry is an object-ID equality check for commits.
fn assert_same_commit(repo: &Path, left: &str, right: &str) {
    assert!(
        run_git_ok(repo, &["merge-base", "--is-ancestor", left, right])
            && run_git_ok(repo, &["merge-base", "--is-ancestor", right, left]),
        "{left} and {right} no longer name the same commit"
    );
}

fn remove_if_exists(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("failed to clear marker {}: {error}", path.display()),
    }
}

fn marker_has_contents(path: &Path, expected: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents == expected,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("failed to read hook marker {}: {error}", path.display()),
    }
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
    fixture: &RemoveFixture,
    expect_hooks: bool,
) {
    let ran = Cell::new(false);
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    group.bench_function(name, |b| {
        b.iter_batched(
            || fixture.prepare(ran.get(), expect_hooks),
            |()| {
                let worktree = fixture.worktree_path();
                let mut cmd = wt_command(binary, &worktree, Some(&fixture.user_config));
                cmd.args(["remove", "--yes", "--force"]);
                if !expect_hooks {
                    cmd.arg("--no-hooks");
                }
                run_and_check(&mut cmd);
                ran.set(true);
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // Criterion does not call a filtered-out benchmark closure.
    if ran.get() {
        fixture.assert_consumed(expect_hooks);
    }
}

fn bench_remove_e2e(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_e2e");
    let no_hooks = RemoveFixture::create();
    let with_hooks = RemoveFixture::create();

    bench_variant(&mut group, "no_hooks", &no_hooks, false);
    bench_variant(&mut group, "with_hooks", &with_hooks, true);

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
