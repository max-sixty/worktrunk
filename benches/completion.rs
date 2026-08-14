//! Shell-completion latency (`COMPLETE=$SHELL wt -- wt switch <Tab>`).
//!
//! This is the one wt path a user waits on with their finger still on Tab, so
//! the number that matters is whole-process wall time. It runs the real binary
//! the way a shell does — the completion handler answers and returns before
//! `main` builds the rayon pool or parses a command, so nothing here shares
//! state with an ordinary `wt` invocation.
//!
//! **One benchmark, on the generated fixture `full` already uses.** Completion
//! has no phases worth timing separately: it spawns, fills its caches on one
//! set of threads, scans refs and worktrees on another, prints, exits.
//! Splitting that into a variant per dimension re-measures the same startup
//! over a series of deliberately lopsided repos and still can't say which call
//! moved, because the calls within a wave overlap. Localizing a regression is a
//! trace's job here, exactly as it is for `full` — see "Analyzing a trace" in
//! benches/CLAUDE.md, and note that a `-vv` run skips prewarm's rev-parse
//! batch and so is not quite the run users get.
//!
//! The benchmark uses the same 24 linked worktrees and 120 branchless branches
//! as `full`, plus 1400 remote-tracking refs. It covers all three categories
//! `BranchCompleter` distinguishes and runs both candidate slow calls
//! (`for-each-ref refs/remotes/` and `worktree list --porcelain`) in one
//! process at a size where each is expensive. Its 145 local candidates also
//! exceed the completer's 100-entry threshold, where remote refs are scanned
//! and then discarded as they are on a long-lived clone.
//!
//! The remote-ref count is what this bench adds to the shared fixture; `full`
//! passes 0 and is unaffected.
//!
//! ```bash
//! cargo bench --bench completion
//! cargo run -p wt-perf -- setup generated 24 120 1400 --path target/wt-generated-completion
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Output;
use wt_perf::{FixtureRecipe, run_and_check, wt_command};

fn run_completion(binary: &Path, repo_path: &Path, words: &[&str]) -> Output {
    let index = words.len().saturating_sub(1);
    let mut cmd = wt_command(binary, repo_path, None);
    cmd.arg("--").args(words);
    cmd.env("COMPLETE", "bash")
        .env("_CLAP_COMPLETE_INDEX", index.to_string())
        .env("_CLAP_COMPLETE_COMP_TYPE", "9")
        .env("_CLAP_COMPLETE_SPACE", "true")
        .env("_CLAP_IFS", "\n");
    run_and_check(&mut cmd)
}

fn expected_branches(branchless_branches: usize, linked_worktrees: usize) -> BTreeSet<String> {
    std::iter::once("main".to_string())
        .chain((0..branchless_branches).map(|i| format!("br-{i:04}")))
        .chain((0..linked_worktrees).map(|i| format!("wt-{i:04}")))
        .collect()
}

fn assert_completion_candidates(binary: &Path, repo_path: &Path, expected: &BTreeSet<String>) {
    let output = run_completion(binary, repo_path, &["wt", "switch", ""]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let candidates: Vec<String> = stdout.lines().map(str::to_string).collect();
    let actual: BTreeSet<String> = candidates.iter().cloned().collect();

    assert_eq!(
        actual.len(),
        candidates.len(),
        "completion returned duplicate candidates: {candidates:?}"
    );
    assert_eq!(&actual, expected, "unexpected completion candidates");
}

fn bench_completion_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("completion_switch");
    let binary = &worktrunk::testing::wt_bin();
    let (linked_worktrees, branchless_branches) = (24, 120);

    group.bench_function("full_surface", |b| {
        let fixture = FixtureRecipe::Generated {
            linked_worktrees,
            branchless_branches,
            remote_tracking_refs: 1400,
        }
        .create();
        let expected = expected_branches(branchless_branches, linked_worktrees);
        assert_completion_candidates(binary, fixture.path(), &expected);
        b.iter(|| run_completion(binary, fixture.path(), &["wt", "switch", ""]));
    });

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
