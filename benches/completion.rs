//! Shell-completion latency (`COMPLETE=$SHELL wt -- wt switch <Tab>`).
//!
//! This is the one wt path a user waits on with their finger still on Tab, so
//! the number that matters is whole-process wall time, not any phase of it.
//! Every variant runs the real binary the way a shell does — the completion
//! handler returns before `main` reaches `Repository::prewarm`, so nothing
//! here shares state with an ordinary `wt` invocation.
//!
//! The variants differ only in repo shape, because shape is what decides how
//! many refs the completer's git calls have to read:
//!
//! - `branches_only` / `with_worktrees` — a small repo; wall time is
//!   dominated by fork overhead, so these track the fixed cost of the call
//!   sequence itself.
//! - `many_remote_refs` — a long-lived clone: 80 local branches against 1400
//!   remote-tracking refs. `refs/remotes/` is the most expensive scan on the
//!   path, and at this shape every ref it reads is then discarded, because the
//!   candidate total clears `BranchCompleter`'s 100-entry threshold and
//!   remote-only branches are dropped. Regressions in how the completer
//!   decides what to scan land here and nowhere else.
//! - `remove_many_remote_refs` — the same repo through `wt remove <Tab>`,
//!   whose completer never offers remote-only branches. It exists to hold the
//!   contrast: this one must not pay for `refs/remotes/` at all.
//!
//! ```bash
//! cargo bench --bench completion                    # all variants
//! cargo bench --bench completion many_remote_refs   # the long-lived-clone shape
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use worktrunk::testing::isolate_subprocess_env;
use wt_perf::{RepoConfig, create_repo};

fn run_completion(binary: &Path, repo_path: &Path, words: &[&str]) {
    let index = words.len().saturating_sub(1);
    let mut cmd = Command::new(binary);
    cmd.arg("--").args(words).current_dir(repo_path);
    isolate_subprocess_env(&mut cmd, None);
    cmd.env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("_CLAP_COMPLETE_COMP_TYPE", "9")
        .env("_CLAP_COMPLETE_SPACE", "true")
        .env("_CLAP_IFS", "\n");
    cmd.output().unwrap();
}

fn bench_completion_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    // Without worktrees: all branches are candidates
    group.bench_function("branches_only", |b| {
        let config = RepoConfig::branches(50, 0);
        let temp = create_repo(&config);
        let repo = temp.path().join("repo");
        b.iter(|| run_completion(binary, &repo, &["wt", "switch", ""]));
    });

    // With worktrees: filters out branches that already have worktrees
    group.bench_function("with_worktrees", |b| {
        let config = RepoConfig {
            worktrees: 10,
            ..RepoConfig::branches(50, 0)
        };
        let temp = create_repo(&config);
        let repo = temp.path().join("repo");
        b.iter(|| run_completion(binary, &repo, &["wt", "switch", ""]));
    });

    // A long-lived clone: local branches swamped by remote-tracking refs.
    // Built once and shared by the two completers below, which differ only in
    // whether they can offer remote-only branches at all.
    let clone_shaped = create_repo(&RepoConfig::many_remote_refs(80, 1400));
    let clone_shaped = clone_shaped.path().join("repo");

    group.bench_function("many_remote_refs", |b| {
        b.iter(|| run_completion(binary, &clone_shaped, &["wt", "switch", ""]));
    });

    // `wt remove` never offers remote-only branches, so the same repo must
    // cost less here than through `switch`.
    group.bench_function("remove_many_remote_refs", |b| {
        b.iter(|| run_completion(binary, &clone_shaped, &["wt", "remove", ""]));
    });

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
