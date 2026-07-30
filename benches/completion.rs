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

fn expected_branches(branchless_branches: usize) -> BTreeSet<String> {
    std::iter::once("main".to_string())
        .chain((0..branchless_branches).map(|i| format!("feature-{i:03}")))
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
    let binary = Path::new(env!("CARGO_BIN_EXE_wt"));

    for (id, linked_worktrees) in [("branches_only", 0), ("with_worktrees", 9)] {
        group.bench_function(id, |b| {
            let fixture = FixtureRecipe::Minimal {
                branchless_branches: 50,
                linked_worktrees,
            }
            .create();
            let mut expected = expected_branches(50);
            expected.extend((1..=linked_worktrees).map(|i| format!("feature-wt-{i}")));
            assert_completion_candidates(binary, fixture.path(), &expected);
            b.iter(|| run_completion(binary, fixture.path(), &["wt", "switch", ""]));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_completion_switch);
criterion_main!(benches);
