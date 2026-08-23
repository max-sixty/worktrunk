//! `wt list`'s working-tree conflict probe must not leave objects in the real
//! object database.
//!
//! `WorkingTreeConflictsTask` snapshots each dirty worktree with `git add -A`
//! into a temporary index and then `git write-tree`. The temporary index keeps
//! the user's staging state intact, but `write-tree` still materialises a tree —
//! and a blob per non-gitignored untracked file — and nothing ever references
//! them. Written to the real database, every invocation whose working tree
//! changed since the last one left unreachable objects behind, so a repo with
//! large untracked artifacts grew by their full size per probe until a
//! `git gc --prune`.
//!
//! `Repository::redirect_objects_for_observation` routes those writes into a
//! side store under `.git/wt/cache/probe-objects`, which keeps them reusable across
//! invocations without entering the database `git gc` and `git fsck` account
//! for. These tests pin that the observational path stays object-neutral.

use crate::common::{TestRepo, list_snapshots, repo};
use rstest::rstest;
use std::path::Path;

/// Every loose object file in an object database, ignoring `info/` and `pack/`.
///
/// Takes the object database directory itself, so it serves both `<gitdir>/objects`
/// and the probe store. Unwraps rather than defaulting to empty: a `read_dir`
/// failure would make both the count and byte assertions pass vacuously.
fn loose_object_files(objects: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(objects)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Loose object fanout directories are two hex characters.
            name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit())
        })
        .flat_map(|fanout| {
            std::fs::read_dir(fanout.path())
                .unwrap()
                .filter_map(Result::ok)
        })
        .collect()
}

/// Count loose objects in an object database.
fn loose_object_count(objects: &Path) -> usize {
    loose_object_files(objects).len()
}

/// Total bytes of loose objects in an object database.
fn loose_object_bytes(objects: &Path) -> u64 {
    loose_object_files(objects)
        .iter()
        .map(|object| object.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

fn run_list(repo: &TestRepo) {
    let status = list_snapshots::command(repo, repo.root_path())
        .status()
        .unwrap();
    assert!(status.success(), "wt list should succeed");
}

/// The temporary index leaves the user's staging state alone.
///
/// This is the half of the mechanism that behaves: `git add -A` runs against a
/// copy of the index under `GIT_INDEX_FILE`, so nothing is staged as a
/// side effect of listing.
#[rstest]
fn test_list_does_not_stage_working_tree_changes(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    std::fs::write(worktree.join("untracked.txt"), "new file\n").unwrap();

    // Assert against the worktree the probe actually snapshots. The primary is
    // clean, so `WorkingTreeConflictsTask` returns before the temp-index
    // `git add -A` ever runs there.
    let probed = worktree.to_str().unwrap();
    let before = repo.git_output(&["-C", probed, "status", "--porcelain"]);
    assert!(
        before.contains("?? untracked.txt"),
        "precondition: the probed worktree must be dirty, got: {before}"
    );
    run_list(&repo);
    let after = repo.git_output(&["-C", probed, "status", "--porcelain"]);

    assert_eq!(
        before, after,
        "wt list must not change the probed worktree's staging state"
    );
}

/// A `wt list` over an unchanged working tree adds no objects.
///
/// `write-tree` is content-addressed, so re-snapshotting identical content
/// re-derives the same SHA and writes nothing. This is why the leak is easy to
/// miss: it only shows up when the working tree moves between invocations.
#[rstest]
fn test_list_over_unchanged_worktree_adds_no_objects(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    std::fs::write(worktree.join("untracked.txt"), "new file\n").unwrap();

    let objects = repo.root_path().join(".git/objects");

    // Prime the object database with this working-tree state.
    run_list(&repo);
    let baseline = loose_object_count(&objects);

    for _ in 0..3 {
        run_list(&repo);
    }

    assert_eq!(
        loose_object_count(&objects),
        baseline,
        "repeated wt list over an unchanged worktree should be object-neutral"
    );
}

/// Repeated `wt list` over a *changing* working tree stays object-neutral.
///
/// This is the regression: the probe's tree (plus the blobs it needs) is never
/// committed or referenced, so it must land in the probe store rather than
/// climbing the real database on every invocation.
#[rstest]
fn test_list_over_changing_worktree_adds_no_objects(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    let churning = worktree.join("churn.txt");
    std::fs::write(&churning, "initial\n").unwrap();

    let objects = repo.root_path().join(".git/objects");

    run_list(&repo);
    let baseline = loose_object_count(&objects);

    const ITERATIONS: usize = 4;
    for iteration in 0..ITERATIONS {
        // A distinct working-tree state per invocation, as an edit-and-look
        // loop produces.
        std::fs::write(&churning, format!("revision {iteration}\n")).unwrap();
        run_list(&repo);
    }

    assert_eq!(
        loose_object_count(&objects),
        baseline,
        "the conflict probe must not add objects to the real database, \
         even when the working tree changes between invocations"
    );

    // And nothing unreachable is left for a later `git gc` to reclaim.
    let unreachable = repo.git_output(&["fsck", "--unreachable"]);
    let unreachable_trees = unreachable
        .lines()
        .filter(|line| line.contains("unreachable tree"))
        .count();
    assert_eq!(
        unreachable_trees, 0,
        "the probe must leave no unreachable trees behind:\n{unreachable}"
    );
}

/// Untracked file *content* never reaches the real object database.
///
/// `git add -A` stages every non-gitignored untracked file, so before the
/// redirect each invocation copied that content in as an unreachable blob —
/// the byte volume, not the object count, is what made this a disk-space
/// problem. This pins the bytes, since a count-based assertion would pass
/// while gigabytes accumulated.
#[rstest]
fn test_list_keeps_untracked_content_out_of_the_object_database(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    let artifact = worktree.join("artifact.bin");

    let objects = repo.root_path().join(".git/objects");

    // Distinct, poorly-compressible 1 MiB payloads, so no two invocations
    // dedupe and zlib can't hide the cost. A small xorshift keeps the entropy
    // high without pulling in an RNG dependency.
    const PAYLOAD_BYTES: usize = 1024 * 1024;
    const ITERATIONS: usize = 3;
    for iteration in 0..ITERATIONS {
        let mut state = 0x2545_F491_4F6C_DD1Du64 ^ (iteration as u64 + 1);
        let payload: Vec<u8> = (0..PAYLOAD_BYTES)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        std::fs::write(&artifact, &payload).unwrap();
        run_list(&repo);
    }

    // Sum loose object sizes rather than counting them: the count barely moves
    // while the bytes climb by the artifact's size each time.
    let loose_bytes = loose_object_bytes(&objects);

    // The payloads barely compress, so even one leaked copy would blow past a
    // half-payload ceiling; the repo's own committed objects are far smaller.
    let ceiling = (PAYLOAD_BYTES / 2) as u64;
    assert!(
        loose_bytes < ceiling,
        "untracked content must not reach the real object database: \
         {loose_bytes} bytes loose after {ITERATIONS} invocations of \
         {PAYLOAD_BYTES} bytes each"
    );
}

/// The probe store persists across invocations, so repeat probes stay cheap.
///
/// Git skips an object write entirely when the id already resolves, so a store
/// that started empty each run would re-deflate an unchanged untracked artifact
/// every time. Pin that the store survives and holds the probe output, which is
/// what makes that skip possible on the second run.
#[rstest]
fn test_probe_store_persists_between_invocations(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    std::fs::write(worktree.join("untracked.txt"), "new file\n").unwrap();

    let probe_store = repo.root_path().join(".git/wt/cache/probe-objects");

    run_list(&repo);
    assert!(
        probe_store.is_dir(),
        "the probe store must outlive the invocation that created it"
    );
    let after_first = loose_object_count(&probe_store);
    assert!(
        after_first > 0,
        "the probe store must hold the probe's objects, found none"
    );

    // A second run over identical content re-resolves rather than re-writing.
    run_list(&repo);
    assert_eq!(
        loose_object_count(&probe_store),
        after_first,
        "an unchanged worktree must not add objects to the probe store either"
    );
}

/// The probe store does not show up as a working-tree change.
///
/// It lives under `.git`, so git ignores it without a `.gitignore` entry. A
/// store placed anywhere else would make every repo permanently dirty.
#[rstest]
fn test_probe_store_does_not_dirty_the_worktree(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    std::fs::write(worktree.join("untracked.txt"), "new file\n").unwrap();

    run_list(&repo);

    let status = repo.git_output(&["status", "--porcelain"]);
    assert!(
        status.trim().is_empty(),
        "the probe store must not appear in git status, got: {status}"
    );
}
