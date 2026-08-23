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
//! temporary object store that is discarded at process exit. These tests pin
//! that the observational path stays object-neutral.

use crate::common::{TestRepo, list_snapshots, repo};
use rstest::rstest;
use std::path::Path;

/// Count loose objects in `<gitdir>/objects`, ignoring `info/` and `pack/`.
fn loose_object_count(git_dir: &Path) -> usize {
    let objects = git_dir.join("objects");
    let entries = std::fs::read_dir(&objects).unwrap();
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Loose object fanout directories are two hex characters.
            name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit())
        })
        .filter_map(|fanout| std::fs::read_dir(fanout.path()).ok())
        .map(|files| files.filter_map(Result::ok).count())
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

    let before = repo.git_output(&["status", "--porcelain"]);
    run_list(&repo);
    let after = repo.git_output(&["status", "--porcelain"]);

    assert_eq!(
        before, after,
        "wt list must not change the primary worktree's staging state"
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

    let git_dir = repo.root_path().join(".git");

    // Prime the object database with this working-tree state.
    run_list(&repo);
    let baseline = loose_object_count(&git_dir);

    for _ in 0..3 {
        run_list(&repo);
    }

    assert_eq!(
        loose_object_count(&git_dir),
        baseline,
        "repeated wt list over an unchanged worktree should be object-neutral"
    );
}

/// Repeated `wt list` over a *changing* working tree stays object-neutral.
///
/// This is the regression: the probe's tree (plus the blobs it needs) is never
/// committed or referenced, so it must land in the throwaway store rather than
/// climbing the real database on every invocation.
#[rstest]
fn test_list_over_changing_worktree_adds_no_objects(mut repo: TestRepo) {
    repo.commit("Initial commit");
    let worktree = repo.add_worktree("feature");
    let churning = worktree.join("churn.txt");
    std::fs::write(&churning, "initial\n").unwrap();

    let git_dir = repo.root_path().join(".git");

    run_list(&repo);
    let baseline = loose_object_count(&git_dir);

    const ITERATIONS: usize = 4;
    for iteration in 0..ITERATIONS {
        // A distinct working-tree state per invocation, as an edit-and-look
        // loop produces.
        std::fs::write(&churning, format!("revision {iteration}\n")).unwrap();
        run_list(&repo);
    }

    assert_eq!(
        loose_object_count(&git_dir),
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

    let git_dir = repo.root_path().join(".git");

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
    let objects = git_dir.join("objects");
    let mut loose_bytes = 0u64;
    for fanout in std::fs::read_dir(&objects).unwrap().filter_map(Result::ok) {
        let name = fanout.file_name();
        let name = name.to_string_lossy();
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        for object in std::fs::read_dir(fanout.path())
            .unwrap()
            .filter_map(Result::ok)
        {
            loose_bytes += object.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }

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
