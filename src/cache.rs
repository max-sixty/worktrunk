//! Shared primitives for the on-disk caches under `.git/wt/cache/`.
//!
//! Three callers use these primitives: `sha_cache` (content-addressed SHA-pair
//! results), `ci_status::cache` (branch → CI status with TTL, plus the repo-wide
//! max-PR-number ratchet), and `summary`
//! (branch → LLM summary with content-addressed filenames). Each owns its
//! layout, struct shape, and freshness rules — this module only owns the
//! filesystem mechanics so those rules have one implementation instead of
//! three.
//!
//! # Torn-write semantics
//!
//! Writes use a plain [`fs::write`], not temp-file-plus-rename. A crash in the
//! middle of a write produces a truncated file at the expected path, which
//! [`read_json`] rejects as corrupt JSON — indistinguishable from a cache miss
//! from the caller's perspective. Two concurrent writers for the same key
//! produce the same value for content-addressed caches (benign) and the last
//! writer wins for TTL-based ones (benign — the next read re-fetches if
//! stale). Neither case justifies the rename dance.
//!
//! # Error policy
//!
//! - [`read_json`] returns `None` on any failure (missing file, I/O error,
//!   corrupt JSON) — callers treat all three as a cache miss. Corrupt JSON
//!   is logged at debug.
//! - [`write_json`] degrades silently. Callers never observe cache write
//!   failures because a failed write just means the next access re-computes.
//! - [`clear_one`] and [`clear_json_files`] propagate non-`NotFound` I/O
//!   errors so `wt config state clear` can report truthfully when it can't
//!   delete a file (e.g. permission denied). `NotFound` is counted as "already
//!   gone" so concurrent clearers don't fight each other.
//!
//! # Epoch
//!
//! A SHA-keyed entry has no TTL and no invalidation rule, so it answers for
//! whatever code wrote it until its key recurs — which for a finished branch
//! is never. That is correct while a given key means one thing, and wrong the
//! moment a release changes how the value is computed: the stale entry then
//! outranks the new code at the one call that would have corrected it.
//! `CACHE_EPOCH` is the version of that meaning. `ensure_epoch` discards the
//! kinds it governs when the stamp on disk disagrees, so a generator change is
//! one constant bump rather than a rename per affected kind.
//! `UNVERSIONED_KINDS` names the kinds it leaves alone and why each one holds.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Serialize, de::DeserializeOwned};

use crate::git::Repository;

/// The meaning of the cached values, bumped whenever a change alters what any
/// of them says.
///
/// 1 — v0.79.0 moved every diff `wt` parses onto plumbing, so display config
/// can no longer change what it reads. The porcelain reads that preceded it
/// wrote `has-added-changes` and `merge-add-probe` entries that, under
/// `diff.relative` or `submodule.<name>.ignore`, recorded a branch as having
/// no added changes when it had some. Those entries are keyed on the branch
/// and target tips, both of which sit still on a finished branch, so without
/// this stamp the first `wt remove` after the upgrade still reads the old
/// answer and deletes an unmerged branch. `diff-stats` and the diffs behind
/// the LLM prompts moved in the same change.
///
/// The stamp records the last worktrunk to *check* a tree, not the version
/// that wrote each entry, so a v0.78.0 binary run against an already-stamped
/// tree writes porcelain answers this one will trust. Two binaries against one
/// repository is the case that costs; putting the epoch in each entry's path
/// is what would close it.
const CACHE_EPOCH: u32 = 1;

/// Kinds the epoch leaves alone. `ci-status` and `pr-number` store the forge's
/// own answer rather than anything `wt` derives — the first under a TTL, the
/// second a ratchet under a constant key — so no change to how `wt` computes
/// things can make them wrong.
///
/// `summary` is exempt on weaker grounds: its key is a hash of the diff, but
/// `SUMMARY_TEMPLATE` and `llm::prepare_diff`'s filtering sit downstream of
/// that hash, so rewording the prompt leaves every finished branch holding a
/// summary the old one wrote. It stays exempt because the value is
/// display-only and reaches no destructive decision, while rebuilding it is a
/// model call per branch. Closing that gap means hashing the rendered prompt,
/// not bumping this constant.
///
/// A new kind left off this list is discarded on a bump, which is the safe
/// direction: that costs a recomputation, while wrongly exempting one costs
/// the branch the stale answer is consulted about.
const UNVERSIONED_KINDS: &[&str] = &["summary", "ci-status", "pr-number"];

/// Cache roots whose epoch this process has already checked.
///
/// Keyed by path rather than held on the `Repository`, so the check covers a
/// second repository in the same process and stays inside this module.
static EPOCH_CHECKED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// The root holding every cache kind for a repository.
fn cache_root(repo: &Repository) -> PathBuf {
    repo.wt_dir().join("cache")
}

/// Discard a repository's cache tree when it was written under a different
/// [`CACHE_EPOCH`], then stamp it with the current one.
///
/// Runs at most once per cache root per process, from [`cache_dir`] — the one
/// funnel every read and write passes through. `EPOCH_CHECKED`'s lock is held
/// across the removal, not just the bookkeeping, because `wt list` fans the
/// SHA-keyed reads over a thread pool and a worker that saw the root marked
/// mid-wipe would read the entries the wipe is there to discard. The stamp
/// goes when the cache does ([`clear_epoch`]): a cleared tree then re-stamps
/// on the next command at the cost of wiping nothing, and entries an older
/// worktrunk wrote into it meanwhile are discarded rather than trusted.
///
/// Best-effort, like the rest of this module, and asymmetric in its failures:
/// everything under the root is regenerable, so a failed removal leaves the
/// tree unstamped for the next process to retry, while a removal that succeeds
/// and then fails to stamp costs one more wipe per process until it lands.
fn ensure_epoch(repo: &Repository) {
    let root = cache_root(repo);
    // Held across the removal, not just the set insert: `wt list` fans
    // `has_added_changes_by_sha` out over a Rayon pool, and a worker that found
    // the root already marked while the wipe was still running would read the
    // entries it is there to discard.
    let mut checked = EPOCH_CHECKED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if checked.contains(&root) {
        return;
    }

    if read_json::<u32>(&epoch_stamp(&root)) != Some(CACHE_EPOCH) {
        match discard_governed_kinds(&root) {
            Ok(()) => stamp_epoch(repo),
            // Nothing better is available: the entries are undeletable, so
            // every read for the rest of this process gets the stale tree —
            // including the `has-added-changes` probe the epoch exists to
            // protect. Retrying per call would repeat the failure without
            // changing that. Left unstamped, so the next process tries again.
            Err(e) => {
                tracing::debug!(path = %root.display(), error = %e, "cache: failed to clear {} for epoch {}: {}", root.display(), CACHE_EPOCH, e);
            }
        }
    }
    checked.insert(root);
}

/// Remove every kind directory under `root` that [`CACHE_EPOCH`] governs.
///
/// Walks what is on disk rather than a list of known kinds, so a kind this
/// version no longer writes still goes; [`UNVERSIONED_KINDS`] is the exemption,
/// and anything else found is discarded. A missing root is nothing to do.
fn discard_governed_kinds(root: &Path) -> std::io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // Accumulated with `and` rather than `?` or `try_fold`, both of which
    // stop at the first error: one undeletable directory would strand the
    // others, and which ones would follow `read_dir` order rather than
    // anything meaningful. Every kind is attempted, the first error returned.
    let mut outcome = Ok(());
    for entry in entries {
        let result = entry.and_then(|entry| {
            let name = entry.file_name();
            let exempt = name
                .to_str()
                .is_some_and(|name| UNVERSIONED_KINDS.contains(&name));
            if exempt || !entry.file_type()?.is_dir() {
                return Ok(());
            }
            fs::remove_dir_all(entry.path())
        });
        outcome = outcome.and(result);
    }
    outcome
}

/// The stamp naming the [`CACHE_EPOCH`] a cache tree was written under.
fn epoch_stamp(root: &Path) -> PathBuf {
    root.join(".epoch")
}

/// Record a repository's cache tree as written under the current
/// [`CACHE_EPOCH`], leaving its entries alone.
///
/// `ensure_epoch` calls this after discarding a tree from an older epoch.
/// A caller that puts entries in the tree itself — a test seeding what this
/// version would have produced — calls it beforehand, so the first read
/// doesn't sweep them as an older version's.
pub(crate) fn stamp_epoch(repo: &Repository) {
    write_json(&epoch_stamp(&cache_root(repo)), &CACHE_EPOCH);
}

/// Drop a repository's epoch stamp, as part of clearing its cache.
///
/// The stamp describes the tree's contents, so it belongs to them: left behind
/// over an emptied tree it would vouch for whatever lands there next,
/// including entries an older worktrunk writes before this one runs again.
///
/// Propagates like the other clear functions, so a stamp that can't be removed
/// fails the clear rather than leaving `wt config state cache clear` reporting
/// a tree it didn't finish clearing. The removal isn't counted as a cleared
/// entry — it is metadata, and its absence costs one wipe of an empty
/// directory.
pub fn clear_epoch(repo: &Repository) -> anyhow::Result<()> {
    clear_one(&epoch_stamp(&cache_root(repo))).map(|_| ())
}

/// The root directory for a named cache kind.
///
/// Returns `<git-common-dir>/wt/cache/<kind>/`. All worktrunk caches live
/// here; the `kind` is the subdirectory name (e.g. `"ci-status"`,
/// `"summary"`, `"is-ancestor"`).
///
/// The first call per cache root in a process settles the tree's
/// `CACHE_EPOCH` (see `ensure_epoch`).
pub fn cache_dir(repo: &Repository, kind: &str) -> PathBuf {
    ensure_epoch(repo);
    cache_root(repo).join(kind)
}

/// Read and deserialize a JSON cache entry.
///
/// Returns `None` on any failure. Corrupt JSON is logged at debug — a torn
/// write is indistinguishable from a cache miss at this layer.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let json = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<T>(&json) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "cache: corrupt entry at {}: {}", path.display(), e);
            None
        }
    }
}

/// Serialize and write a JSON cache entry, creating parent directories as
/// needed.
///
/// Degrades silently on any failure — parent dir creation, serialization,
/// or the write itself. A failed write just means the next access
/// re-computes; callers must never observe the error.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        tracing::debug!(path = %parent.display(), error = %e, "cache: failed to create dir {}: {}", parent.display(), e);
        return;
    }

    let Ok(json) = serde_json::to_string(value) else {
        tracing::debug!(path = %path.display(), "cache: failed to serialize entry for {}", path.display());
        return;
    };

    if let Err(e) = fs::write(path, &json) {
        tracing::debug!(path = %path.display(), error = %e, "cache: failed to write {}: {}", path.display(), e);
    }
}

/// Read a JSON entry at `<wt-cache>/<kind>/<key>`.
///
/// Paired with [`write_with_lru`] for the flat-dir "kind + key filename"
/// layout. Returns `None` on any failure (missing file, I/O error, corrupt
/// JSON).
pub fn read<T: DeserializeOwned>(repo: &Repository, kind: &str, key: &str) -> Option<T> {
    read_json(&cache_dir(repo, kind).join(key))
}

/// Write a JSON entry at `<wt-cache>/<kind>/<key>`, then sweep the kind
/// directory so it holds at most `max_entries` top-level `.json` files.
///
/// Combines [`write_json`] with [`sweep_lru`] — the "write + bound" pattern
/// every `sha_cache` `put_*` function repeats. Degrades silently on write
/// failure; the sweep runs regardless so a torn write still triggers the
/// size bound check.
pub fn write_with_lru<T: Serialize>(
    repo: &Repository,
    kind: &str,
    key: &str,
    value: &T,
    max_entries: usize,
) {
    let dir = cache_dir(repo, kind);
    write_json(&dir.join(key), value);
    sweep_lru(&dir, max_entries);
}

/// Enforce a size bound on `dir`. If it holds more than `max` top-level
/// `.json` entries, delete the oldest-mtime files until the count is back
/// at `max`.
///
/// The fast path is a single directory listing and `count_json_files` — no
/// per-file `stat` when the cache is under the bound. Only falls through
/// to stat+sort when trimming is actually needed.
///
/// Best-effort: I/O errors during the sweep are logged at debug and ignored
/// because the cache is always an optimization over re-computation.
pub fn sweep_lru(dir: &Path, max: usize) {
    if count_json_files(dir) <= max {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let json_entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
        .collect();
    if json_entries.len() <= max {
        return;
    }

    let mut with_mtime: Vec<(PathBuf, SystemTime)> = json_entries
        .into_iter()
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), mtime))
        })
        .collect();
    with_mtime.sort_by_key(|(_, mtime)| *mtime);

    let excess = with_mtime.len().saturating_sub(max);
    for (path, _) in with_mtime.iter().take(excess) {
        let _ = fs::remove_file(path);
    }
    tracing::debug!(count = excess, dir = %dir.display(), "cache: swept {} entries from {}", excess, dir.display());
}

/// Remove a single cache entry.
///
/// Returns `Ok(true)` if a file was removed, `Ok(false)` if it was already
/// gone (a concurrent clearer, or the caller being paranoid). Propagates
/// other I/O errors with the path attached, so `wt config state clear`
/// reports "Cleared"/"No cache" truthfully instead of swallowing a
/// permission-denied failure.
pub fn clear_one(path: &Path) -> anyhow::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("failed to remove {}", path.display())))
        }
    }
}

/// Remove every top-level `.json` file in `dir`, returning the count
/// removed.
///
/// Missing directory is `Ok(0)` — the caller's cache is already empty.
/// Concurrent removal of individual entries is counted as "already gone".
/// Non-`.json` siblings (e.g. leftover `.json.tmp` from old code, or a
/// stray `README`) are left in place.
pub fn clear_json_files(dir: &Path) -> anyhow::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("failed to read {}", dir.display())));
        }
    };

    let mut cleared = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if clear_one(&path)? {
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Count top-level `.json` files in `dir`, returning `0` when the directory
/// is missing. Used by `wt config state get` for the `get ↔ clear` parity
/// view.
pub fn count_json_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;
    use tempfile::TempDir;

    /// The epoch's whole job is to answer "was this written by code that meant
    /// something else". A missing stamp is only the v0.78.0 case; every bump
    /// after this one turns on a stamp that disagrees, and on a matching stamp
    /// leaving entries where they are.
    #[test]
    fn test_epoch_discards_a_tree_stamped_by_another_version() {
        for (stamped, survives) in [
            (None, false),
            (Some(CACHE_EPOCH + 1), false),
            (Some(CACHE_EPOCH), true),
        ] {
            let test = TestRepo::new();
            let root = cache_root(&test.repo);
            // Seeded off `cache_root`, not `cache_dir`: that call is itself the
            // funnel, so seeding through it would sweep the tree first and
            // land both entries in one already discarded.
            let entry = root.join("has-added-changes").join("a-b.json");
            let exempt = root.join(UNVERSIONED_KINDS[0]).join("x.json");
            write_json(&entry, &true);
            write_json(&exempt, &true);

            let stamp = epoch_stamp(&root);
            match stamped {
                Some(epoch) => write_json(&stamp, &epoch),
                None => fs::remove_file(&stamp).unwrap(),
            }
            // `cache_dir` checks once per root per process, and `TestRepo`
            // stamped this root as it built it.
            EPOCH_CHECKED
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .unwrap()
                .remove(&root);

            let _ = cache_dir(&test.repo, "has-added-changes");
            assert_eq!(
                read_json::<bool>(&entry).is_some(),
                survives,
                "stamp {stamped:?} against epoch {CACHE_EPOCH}"
            );
            assert!(
                read_json::<bool>(&exempt).is_some(),
                "an unversioned kind survives any stamp, here {stamped:?}"
            );
            assert_eq!(
                read_json::<u32>(&epoch_stamp(&root)),
                Some(CACHE_EPOCH),
                "the tree is stamped current either way"
            );
        }
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct V {
        x: u32,
    }

    #[test]
    fn test_read_write_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sub/entry.json");

        // Missing file is a miss.
        assert!(read_json::<V>(&path).is_none());

        // Write creates parent dirs and round-trips.
        write_json(&path, &V { x: 42 });
        assert_eq!(read_json::<V>(&path), Some(V { x: 42 }));
    }

    #[test]
    fn test_read_corrupt_json_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "not json {{").unwrap();
        assert!(read_json::<V>(&path).is_none());
    }

    #[test]
    fn test_clear_one_missing_returns_false() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nope.json");
        assert!(!clear_one(&path).unwrap());
    }

    #[test]
    fn test_clear_one_propagates_non_not_found() {
        let tmp = TempDir::new().unwrap();
        // Put a directory where a file is expected so remove_file returns
        // EISDIR (or similar), not NotFound.
        let path = tmp.path().join("dir.json");
        fs::create_dir(&path).unwrap();
        let err = clear_one(&path).unwrap_err();
        assert!(err.to_string().contains("failed to remove"), "got: {err}");
    }

    #[test]
    fn test_clear_json_files_counts_and_skips() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), "{}").unwrap();
        fs::write(dir.join("b.json"), "{}").unwrap();
        // Non-.json siblings must be skipped and left in place.
        fs::write(dir.join("README"), "stray").unwrap();
        fs::write(dir.join("a.json.tmp"), "leftover").unwrap();

        assert_eq!(clear_json_files(&dir).unwrap(), 2);
        assert!(!dir.join("a.json").exists());
        assert!(!dir.join("b.json").exists());
        assert!(dir.join("README").exists());
        assert!(dir.join("a.json.tmp").exists());
    }

    #[test]
    fn test_clear_json_files_missing_dir_is_zero() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(clear_json_files(&tmp.path().join("nope")).unwrap(), 0);
    }

    #[test]
    fn test_clear_json_files_propagates_read_dir_error() {
        let tmp = TempDir::new().unwrap();
        // Put a file where a directory is expected — read_dir returns
        // NotADirectory (not NotFound).
        let path = tmp.path().join("not-a-dir");
        fs::write(&path, "file").unwrap();
        let err = clear_json_files(&path).unwrap_err();
        assert!(err.to_string().contains("failed to read"), "got: {err}");
    }

    #[test]
    fn test_count_json_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), "{}").unwrap();
        fs::write(dir.join("README"), "stray").unwrap();

        assert_eq!(count_json_files(&dir), 1);
        assert_eq!(count_json_files(&tmp.path().join("nope")), 0);
    }

    #[test]
    fn test_sweep_lru_trims_oldest_entries() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();

        for i in 0..5 {
            fs::write(dir.join(format!("entry{i}.json")), "true").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        sweep_lru(&dir, 3);

        let mut remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        assert_eq!(remaining, ["entry2.json", "entry3.json", "entry4.json"]);
    }

    #[test]
    fn test_sweep_lru_no_op_under_bound() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("c");
        fs::create_dir_all(&dir).unwrap();

        for i in 0..3 {
            fs::write(dir.join(format!("entry{i}.json")), "true").unwrap();
        }

        sweep_lru(&dir, 5);

        let count = fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 3, "should not delete anything when under bound");
    }
}
