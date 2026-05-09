//! Background preview pre-compute orchestration.
//!
//! Owns the dedicated rayon pool and preview cache for the picker, so the
//! pre-compute pipeline is testable without standing up skim. The picker
//! entry point (`run_picker`) uses this for its real spawns; the dry-run
//! path (`WORKTRUNK_PICKER_DRY_RUN`) uses it to wait for completion and dump the
//! cache to stdout.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use worktrunk::git::Repository;

use super::items::{PreviewCache, WorktreeSkimItem};
use super::preview::PreviewMode;
use super::summary;
use crate::commands::list::model::ListItem;

struct PendingGuard(Arc<AtomicUsize>);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) struct PreviewOrchestrator {
    pub(super) cache: PreviewCache,
    pool: Arc<rayon::ThreadPool>,
    pending: Arc<AtomicUsize>,
    /// Repository used by preview compute. Captured once at construction
    /// so background tasks see a stable repo binding, and so unit tests
    /// can inject a `TestRepo`-rooted `Repository` instead of relying on
    /// process CWD.
    ///
    /// Cloned into each spawned task so they share the underlying
    /// `Arc<RepoCache>` — including the local-branch inventory that
    /// [`Repository::default_branch_sha`] reads from. That's how the
    /// BranchDiff preview avoids forking `git rev-parse` per item:
    /// the inventory's single `for-each-ref` scan serves every task.
    repo: Repository,
}

impl PreviewOrchestrator {
    pub(super) fn new(repo: Repository) -> Self {
        let cache = Arc::new(DashMap::new());
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(crate::rayon_thread_count())
                .thread_name(|i| format!("picker-preview-{i}"))
                .build()
                .expect("failed to build picker preview rayon pool"),
        );
        Self {
            cache,
            pool,
            pending: Arc::new(AtomicUsize::new(0)),
            repo,
        }
    }

    /// Spawn a preview compute task. Returns immediately.
    ///
    /// Idempotent on the cache key: if another task already populated it,
    /// this one short-circuits after the `contains_key` check. Compute
    /// happens outside any DashMap lock so skim's UI thread (which calls
    /// `preview()` synchronously and reads via `DashMap::get`) is never
    /// blocked on a shard write held across git/pager subprocesses.
    ///
    /// Log mode that hits the disk cache also enqueues a refresh task
    /// (via `pool.spawn_fifo`, so it lands behind in-flight foreground
    /// precompute) to recompute the embedded ref decorations before the
    /// next visit. See the `LogCacheEntry` docstring for why the disk
    /// cache itself is SHA-keyed but decoration text drifts.
    pub(super) fn spawn_preview(
        &self,
        item: Arc<ListItem>,
        mode: PreviewMode,
        dims: (usize, usize),
    ) {
        let cache = Arc::clone(&self.cache);
        let (w, h) = dims;
        let repo = self.repo.clone();
        let pool = Arc::clone(&self.pool);
        let pending = Arc::clone(&self.pending);
        self.spawn_task(move || {
            let cache_key = (item.branch_name().to_string(), mode);
            if cache.contains_key(&cache_key) {
                return;
            }
            let (value, log_disk_hit) =
                WorktreeSkimItem::compute_and_page_preview(&repo, &item, mode, w, h);
            cache.insert(cache_key, value);
            if log_disk_hit {
                pending.fetch_add(1, Ordering::SeqCst);
                let guard = PendingGuard(Arc::clone(&pending));
                let item = Arc::clone(&item);
                let cache = Arc::clone(&cache);
                let repo = repo.clone();
                pool.spawn_fifo(move || {
                    let _g = guard;
                    let rendered = WorktreeSkimItem::refresh_log_preview(&repo, &item, w, h);
                    // Skip empty results so a transient `git log` failure
                    // doesn't poison the in-memory cache with "" and wipe
                    // out the value the producer just inserted.
                    if !rendered.is_empty() {
                        cache.insert((item.branch_name().to_string(), PreviewMode::Log), rendered);
                    }
                });
            }
        });
    }

    /// Spawn an LLM summary task. Returns immediately.
    pub(super) fn spawn_summary(&self, item: Arc<ListItem>, llm_command: String, repo: Repository) {
        let cache = Arc::clone(&self.cache);
        self.spawn_task(move || {
            summary::generate_and_cache_summary(&item, &llm_command, &cache, &repo);
        });
    }

    /// Spawn the full pre-compute fan-out for a freshly published skeleton.
    ///
    /// Spawn order: the first item's modes win the first slots (the user
    /// lands there and may tab-cycle), then mode-major across the rest.
    /// Summaries queue last because each LLM call can take seconds.
    ///
    /// When `llm_command` is `None` and `summary_hint` is `Some`, the hint
    /// is written directly into the Summary cache for every item — gives
    /// the Summary tab something useful instead of a perpetual
    /// "Generating…" placeholder.
    pub(super) fn spawn_all_precompute(
        &self,
        list_items: &[Arc<ListItem>],
        preview_dims: (usize, usize),
        llm_command: Option<&str>,
        summary_hint: Option<&str>,
    ) {
        let modes = [
            PreviewMode::WorkingTree,
            PreviewMode::Log,
            PreviewMode::BranchDiff,
            PreviewMode::UpstreamDiff,
        ];

        if let Some(first) = list_items.first() {
            for mode in modes {
                self.spawn_preview(Arc::clone(first), mode, preview_dims);
            }
        }
        for mode in modes {
            for item in list_items.iter().skip(1) {
                self.spawn_preview(Arc::clone(item), mode, preview_dims);
            }
        }

        if let Some(llm) = llm_command {
            if let Some(first) = list_items.first() {
                self.spawn_summary(Arc::clone(first), llm.to_string(), self.repo.clone());
            }
            for item in list_items.iter().skip(1) {
                self.spawn_summary(Arc::clone(item), llm.to_string(), self.repo.clone());
            }
        } else if let Some(hint) = summary_hint {
            for item in list_items {
                let branch = item.branch_name().to_string();
                self.cache
                    .insert((branch, PreviewMode::Summary), hint.to_string());
            }
        }
    }

    fn spawn_task<F: FnOnce() + Send + 'static>(&self, task: F) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let guard = PendingGuard(Arc::clone(&self.pending));
        let wrapped = move || {
            // Guard decrements on drop, so a panic inside `task` still
            // releases the counter — otherwise `wait_for_idle` hangs
            // forever on any panicking preview task.
            let _g = guard;
            task();
        };
        self.pool.spawn(wrapped);
    }

    /// Block until all spawned tasks complete.
    ///
    /// Used by the dry-run path and tests; production never waits — tasks
    /// are fire-and-forget while skim runs. Polls at 10ms resolution; tasks
    /// typically take tens to hundreds of ms, so a condvar isn't worth the
    /// complexity.
    pub(super) fn wait_for_idle(&self) {
        while self.pending.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Dump cache state as JSON for dry-run diagnostics. Byte-length only
    /// (not content) keeps output small and deterministic across terminals.
    pub(super) fn dump_cache_json(&self) -> String {
        let mut entries: Vec<_> = self
            .cache
            .iter()
            .map(|e| {
                let (branch, mode) = e.key();
                (branch.clone(), *mode as u8, e.value().len())
            })
            .collect();
        entries.sort();

        let items: Vec<String> = entries
            .iter()
            .map(|(branch, mode, bytes)| {
                format!("    {{ \"branch\": {branch:?}, \"mode\": {mode}, \"bytes\": {bytes} }}")
            })
            .collect();
        format!("{{\n  \"entries\": [\n{}\n  ]\n}}", items.join(",\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::model::{ItemKind, WorktreeData};
    use std::fs;
    use worktrunk::testing::TestRepo;

    fn orch_for(t: &TestRepo) -> PreviewOrchestrator {
        PreviewOrchestrator::new(Repository::at(t.path()).unwrap())
    }

    fn dirty_worktree_item() -> (TestRepo, Arc<ListItem>) {
        let t = TestRepo::new();
        fs::write(t.path().join("README.md"), "# Project\n").unwrap();
        t.repo.run_command(&["add", "README.md"]).unwrap();
        t.repo.run_command(&["commit", "-m", "initial"]).unwrap();
        // Dirty the working tree so WorkingTree diff has content.
        fs::write(t.path().join("README.md"), "# Project\nmore\n").unwrap();

        let head = t
            .repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let mut item = ListItem::new_branch(head, "main".to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: t.path().to_path_buf(),
            ..Default::default()
        }));
        (t, Arc::new(item))
    }

    /// End-to-end: orchestrator spawns real previews, populates the cache.
    /// Regression test for the "previews never load" class of bugs — if the
    /// spawn pipeline silently fails, this catches it without needing skim.
    #[test]
    fn orchestrator_populates_cache_for_real_worktree() {
        let (t, item) = dirty_worktree_item();

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.spawn_preview(Arc::clone(&item), PreviewMode::Log, (80, 24));
        orch.wait_for_idle();

        let wt_key = ("main".to_string(), PreviewMode::WorkingTree);
        let log_key = ("main".to_string(), PreviewMode::Log);
        assert!(
            orch.cache.contains_key(&wt_key),
            "WorkingTree preview not cached"
        );
        assert!(orch.cache.contains_key(&log_key), "Log preview not cached");
        assert!(
            !orch.cache.get(&wt_key).unwrap().is_empty(),
            "WorkingTree preview was empty"
        );
    }

    #[test]
    fn duplicate_spawn_short_circuits() {
        let (t, item) = dirty_worktree_item();

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.wait_for_idle();
        let first = orch
            .cache
            .get(&("main".to_string(), PreviewMode::WorkingTree))
            .unwrap()
            .value()
            .clone();

        // Second spawn should hit `contains_key` and skip.
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.wait_for_idle();
        let second = orch
            .cache
            .get(&("main".to_string(), PreviewMode::WorkingTree))
            .unwrap()
            .value()
            .clone();
        assert_eq!(first, second);
    }

    /// `spawn_summary` delegates to the same spawn-task machinery as
    /// `spawn_preview`, but via the LLM summary path. The test uses `/bin/cat`
    /// as a fake LLM command (it echoes the prompt back), so the test stays
    /// hermetic — no real LLM is invoked, but the cache receives a Summary
    /// entry proving the task ran to completion.
    #[test]
    fn spawn_summary_populates_cache() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let orch = orch_for(&t);
        orch.spawn_summary(Arc::clone(&item), "/bin/cat".to_string(), repo);
        orch.wait_for_idle();

        assert!(
            orch.cache
                .contains_key(&("main".to_string(), PreviewMode::Summary)),
            "Summary entry not cached"
        );
    }

    /// Disk-cache hit on a Log preview enqueues a background refresh that
    /// overwrites both the disk file and the in-memory DashMap. Seed the
    /// disk cache with a stale `LogCacheEntry` containing a marker —
    /// after `spawn_preview` + `wait_for_idle`, neither cache should
    /// hold the marker, because the refresh thread re-ran
    /// `compute_log_raw_and_stats` and wrote real git-log output.
    ///
    /// `wait_for_idle` covers the refresh thread's task because the
    /// producer increments `pending` before sending and the refresh
    /// thread decrements via `PendingGuard` after running.
    #[test]
    fn log_disk_hit_triggers_background_refresh() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let stale = super::super::preview_cache::LogCacheEntry {
            raw_log: "STALE_MARKER\n".to_string(),
            stats: std::collections::HashMap::new(),
        };
        super::super::preview_cache::write_log(&repo, item.head(), 80, 24, &stale);

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::Log, (80, 24));
        orch.wait_for_idle();

        let disk = super::super::preview_cache::read_log(&repo, item.head(), 80, 24)
            .expect("disk cache present after refresh");
        assert!(
            !disk.raw_log.contains("STALE_MARKER"),
            "refresh should overwrite stale disk entry, got raw_log: {:?}",
            disk.raw_log
        );

        let in_memory = orch
            .cache
            .get(&("main".to_string(), PreviewMode::Log))
            .expect("in-memory entry present")
            .clone();
        assert!(
            !in_memory.contains("STALE_MARKER"),
            "refresh should overwrite stale in-memory entry, got: {in_memory:?}"
        );
    }

    /// Non-Log modes have content-addressed cache keys (BranchDiff is
    /// `(base_sha, branch_sha, w)`, UpstreamDiff similar) and no
    /// decoration drift, so a disk-cache hit on those modes must NOT
    /// enqueue a Log refresh. Seed the disk Log cache with stale content
    /// and spawn a BranchDiff preview — the disk Log cache must remain
    /// stale because the refresh thread never received a task.
    #[test]
    fn non_log_modes_do_not_trigger_log_refresh() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let stale = super::super::preview_cache::LogCacheEntry {
            raw_log: "STALE_MARKER\n".to_string(),
            stats: std::collections::HashMap::new(),
        };
        super::super::preview_cache::write_log(&repo, item.head(), 80, 24, &stale);

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::BranchDiff, (80, 24));
        orch.wait_for_idle();

        let disk = super::super::preview_cache::read_log(&repo, item.head(), 80, 24)
            .expect("disk Log cache untouched");
        assert_eq!(
            disk.raw_log, "STALE_MARKER\n",
            "non-Log spawn must not trigger Log refresh"
        );
    }

    #[test]
    fn dump_cache_json_format() {
        let t = TestRepo::new();
        let orch = orch_for(&t);
        orch.cache.insert(
            ("branch-a".to_string(), PreviewMode::WorkingTree),
            "x".to_string(),
        );
        orch.cache
            .insert(("branch-b".to_string(), PreviewMode::Log), "xy".to_string());
        let json = orch.dump_cache_json();
        // Structural assertion — future field additions shouldn't flake the test.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let entries = parsed["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 2);
    }
}
