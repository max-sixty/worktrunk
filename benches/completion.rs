//! Shell-completion latency (`COMPLETE=$SHELL wt -- wt switch <Tab>`).
//!
//! This is the one wt path a user waits on with their finger still on Tab, so
//! the number that matters is whole-process wall time. It runs the real binary
//! the way a shell does — the completion handler answers and returns before
//! `main` builds the rayon pool or parses a command, so nothing here shares
//! state with an ordinary `wt` invocation.
//!
//! **One benchmark, on the `mixed` fixture `full` already uses.** Completion
//! has no phases worth timing separately: it spawns, fills its caches on one
//! set of threads, scans refs and worktrees on another, prints, exits.
//! Splitting that into a variant per dimension re-measures the same startup
//! over a series of deliberately lopsided repos and still can't say which call
//! moved, because the calls within a wave overlap. Localizing a regression is a
//! trace's job here, exactly as it is for `full` — see "Analyzing a trace" in
//! benches/CLAUDE.md, and note that a `-vv` run skips prewarm's rev-parse
//! batch and so is not quite the run users get.
//!
//! `mixed-80-80-1400` is that one repo: 80 worktrees in four states, 80 more
//! branches forked across 200 commits of history, and 1400 remote-tracking
//! refs. It covers all three categories `BranchCompleter` distinguishes, puts
//! both candidate slow calls (`for-each-ref refs/remotes/` and `worktree list
//! --porcelain`) in the same process at a size where each is expensive, and
//! lands past the completer's 100-entry threshold — where the remote refs are
//! scanned and then discarded, as they are on a real long-lived clone.
//!
//! The remote-ref count is what this bench adds to the shared fixture; `full`
//! passes 0 and is unaffected.
//!
//! ```bash
//! cargo bench --bench completion
//! cargo run -p wt-perf -- setup mixed-80-80-1400 --path /tmp/clone   # same repo, by hand
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use wt_perf::{create_mixed_repo, run_and_check, wt_command};

fn run_completion(binary: &Path, repo_path: &Path, words: &[&str]) {
    let index = words.len().saturating_sub(1);
    let mut cmd = wt_command(binary, repo_path, None);
    cmd.arg("--").args(words);
    cmd.env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("_CLAP_COMPLETE_COMP_TYPE", "9")
        .env("_CLAP_COMPLETE_SPACE", "true")
        .env("_CLAP_IFS", "\n");
    run_and_check(&mut cmd);
}

fn bench_completion_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch");
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    group.bench_function("mixed", |b| {
        let fixture = create_mixed_repo(80, 80, 1400);
        b.iter(|| run_completion(binary, fixture.path(), &["wt", "switch", ""]));
    });

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
