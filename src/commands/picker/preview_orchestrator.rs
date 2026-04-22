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
    pub(super) fn spawn_preview(
        &self,
        item: Arc<ListItem>,
        mode: PreviewMode,
        dims: (usize, usize),
    ) {
        let cache = Arc::clone(&self.cache);
        let (w, h) = dims;
        let repo = self.repo.clone();
        self.spawn_task(move || {
            let cache_key = (item.branch_name().to_string(), mode);
            if cache.contains_key(&cache_key) {
                return;
            }
            let value = WorktreeSkimItem::compute_and_page_preview(&repo, &item, mode, w, h);
            cache.insert(cache_key, value);
        });
    }

    /// Spawn an LLM summary task. Returns immediately.
    pub(super) fn spawn_summary(&self, item: Arc<ListItem>, llm_command: String, repo: Repository) {
        let cache = Arc::clone(&self.cache);
        self.spawn_task(move || {
            summary::generate_and_cache_summary(&item, &llm_command, &cache, &repo);
        });
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
