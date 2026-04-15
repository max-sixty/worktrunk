//! Progressive-rendering glue between `collect::collect` and the skim picker.
//!
//! Each event funnels into three places: skim's item stream (`tx`, alive
//! while updates may arrive so the 100ms heartbeat keeps firing), each
//! item's shared `rendered` mutex (in-place redraws picked up by the
//! heartbeat), and `shared_items` used by `PickerCollector` for alt-r.
//!
//! Preview + summary pre-compute kicks off at skeleton time so the first
//! preview the user opens is already warm.

use std::sync::{Arc, Mutex, OnceLock};

use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::{StyledLine, strip_osc8_hyperlinks};

use super::items::{HeaderSkimItem, PreviewCache, WorktreeSkimItem};
use super::preview::PreviewMode;
use super::preview_orchestrator::PreviewOrchestrator;
use crate::commands::list::collect::PickerProgressHandler;
use crate::commands::list::model::ListItem;

/// Handler owned by the background collect thread. Implements the
/// `PickerProgressHandler` trait that `collect` drives.
///
/// The `tx` clone lives as long as this handler is referenced — dropping the
/// handler (when the collect thread exits) drops the last sender, which
/// stops skim's heartbeat. That's the explicit contract: once background
/// work is done, the picker can go idle.
pub(super) struct PickerHandler {
    pub(super) tx: SkimItemSender,
    /// Mirror of the skim item vec visible to `PickerCollector`. Populated
    /// atomically in `on_skeleton`.
    pub(super) shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    /// One `Arc<Mutex<String>>` per data row — same Arcs `WorktreeSkimItem`
    /// holds. Set once in `on_skeleton`, read lock-free thereafter.
    pub(super) rendered_slots: OnceLock<Box<[Arc<Mutex<String>>]>>,
    pub(super) preview_cache: PreviewCache,
    pub(super) orchestrator: Arc<PreviewOrchestrator>,
    pub(super) preview_dims: (usize, usize),
    pub(super) llm_command: Option<String>,
    pub(super) repo: Repository,
    /// Filled into the Summary preview cache for every item when summaries
    /// are disabled — gives the Summary tab something useful instead of a
    /// perpetual "Generating…" placeholder.
    pub(super) summary_hint: Option<String>,
}

impl PickerProgressHandler for PickerHandler {
    fn on_skeleton(&self, items: Vec<ListItem>, rendered: Vec<String>, header: StyledLine) {
        debug_assert_eq!(items.len(), rendered.len());

        let mut slots: Vec<Arc<Mutex<String>>> = Vec::with_capacity(items.len());
        let mut skim_items: Vec<Arc<dyn SkimItem>> = Vec::with_capacity(items.len() + 1);
        let mut list_items: Vec<Arc<ListItem>> = Vec::with_capacity(items.len());

        // Header row — non-selectable via `header_lines(1)` on the options.
        skim_items.push(Arc::new(HeaderSkimItem {
            display_text: header.plain_text(),
            display_text_with_ansi: header.render(),
        }) as Arc<dyn SkimItem>);

        for (item, rendered_line) in items.into_iter().zip(rendered) {
            let branch_name = item.branch_name().to_string();
            let path_str = item
                .worktree_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            // `search_text` is what the matcher sees — fuzzy ranks stay
            // stable across progressive updates because this field only
            // depends on fast data (branch + path).
            let search_text = if path_str.is_empty() {
                branch_name.clone()
            } else {
                format!("{branch_name} {path_str}")
            };

            // Strip OSC 8 hyperlinks — skim's pipeline mangles them into
            // garbage like `^[8;;…`. Colors (SGR codes) are preserved.
            let rendered_arc = Arc::new(Mutex::new(strip_osc8_hyperlinks(&rendered_line)));
            slots.push(Arc::clone(&rendered_arc));

            let item_arc = Arc::new(item);
            list_items.push(Arc::clone(&item_arc));

            skim_items.push(Arc::new(WorktreeSkimItem {
                search_text,
                rendered: rendered_arc,
                branch_name,
                item: item_arc,
                preview_cache: Arc::clone(&self.preview_cache),
            }) as Arc<dyn SkimItem>);
        }

        // Publish slots + skim items before sending to skim so alt-r reload
        // (which reads `shared_items`) sees a populated list the moment
        // skim calls `CommandCollector::invoke`.
        let _ = self.rendered_slots.set(slots.into_boxed_slice());
        *self.shared_items.lock().unwrap() = skim_items.clone();

        for skim_item in &skim_items {
            let _ = self.tx.send(Arc::clone(skim_item));
        }

        self.spawn_precompute(&list_items);
    }

    fn on_update(&self, idx: usize, rendered: String) {
        if let Some(slots) = self.rendered_slots.get()
            && let Some(slot) = slots.get(idx)
        {
            *slot.lock().unwrap() = strip_osc8_hyperlinks(&rendered);
        }
    }

    fn on_reveal(&self, rendered: Vec<Option<String>>) {
        let Some(slots) = self.rendered_slots.get() else {
            return;
        };
        for (slot, line) in slots.iter().zip(rendered) {
            if let Some(line) = line {
                *slot.lock().unwrap() = strip_osc8_hyperlinks(&line);
            }
        }
    }
}

impl PickerHandler {
    /// Kick off preview + summary pre-compute in the dedicated rayon pool.
    ///
    /// Spawn order mirrors the old synchronous path: the first item's modes
    /// win the first slots (the user lands there and may tab-cycle), then
    /// mode-major across the rest. Summaries queue last because each LLM
    /// call can take seconds.
    fn spawn_precompute(&self, list_items: &[Arc<ListItem>]) {
        let modes = [
            PreviewMode::WorkingTree,
            PreviewMode::Log,
            PreviewMode::BranchDiff,
            PreviewMode::UpstreamDiff,
        ];

        if let Some(first) = list_items.first() {
            for mode in modes {
                self.orchestrator
                    .spawn_preview(Arc::clone(first), mode, self.preview_dims);
            }
        }
        for mode in modes {
            for item in list_items.iter().skip(1) {
                self.orchestrator
                    .spawn_preview(Arc::clone(item), mode, self.preview_dims);
            }
        }

        if let Some(llm) = self.llm_command.as_ref() {
            if let Some(first) = list_items.first() {
                self.orchestrator
                    .spawn_summary(Arc::clone(first), llm.clone(), self.repo.clone());
            }
            for item in list_items.iter().skip(1) {
                self.orchestrator
                    .spawn_summary(Arc::clone(item), llm.clone(), self.repo.clone());
            }
        } else if let Some(hint) = self.summary_hint.as_ref() {
            for item in list_items {
                let branch = item.branch_name().to_string();
                self.preview_cache
                    .insert((branch, PreviewMode::Summary), hint.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::model::ListItem;
    use worktrunk::testing::TestRepo;

    fn make_handler() -> (
        PickerHandler,
        TestRepo,
        crossbeam_channel::Receiver<Arc<dyn SkimItem>>,
    ) {
        let test = TestRepo::with_initial_commit();
        let (tx, rx) = crossbeam_channel::unbounded::<Arc<dyn SkimItem>>();
        let shared_items = Arc::new(Mutex::new(Vec::new()));
        let repo = test.repo.clone();
        let orchestrator = Arc::new(PreviewOrchestrator::new(repo.clone()));
        let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);
        let handler = PickerHandler {
            tx,
            shared_items,
            rendered_slots: OnceLock::new(),
            preview_cache,
            orchestrator,
            preview_dims: (80, 24),
            llm_command: None,
            repo,
            summary_hint: Some("disabled".to_string()),
        };
        (handler, test, rx)
    }

    fn header(text: &str) -> StyledLine {
        let mut line = StyledLine::new();
        line.push_raw(text);
        line
    }

    /// Skeleton → update → reveal: verifies that each event writes through
    /// to the shared `rendered` string the `WorktreeSkimItem` holds. Skim
    /// reads these strings on its heartbeat; the matcher-stable search
    /// text (branch + path) never changes.
    #[test]
    fn handler_updates_render_strings_in_place() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "one".into()),
            ListItem::new_branch("bbb".into(), "two".into()),
        ];
        let rendered = vec!["skel-one".to_string(), "skel-two".to_string()];

        handler.on_skeleton(items, rendered, header("hdr"));

        // Header + 2 items sent to skim.
        let received: Vec<Arc<dyn SkimItem>> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(received.len(), 3, "expected header + 2 items");

        let slots = handler.rendered_slots.get().unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(*slots[0].lock().unwrap(), "skel-one");
        assert_eq!(*slots[1].lock().unwrap(), "skel-two");

        // on_update rewrites a single slot (the second item here).
        handler.on_update(1, "updated-two".into());
        assert_eq!(*slots[0].lock().unwrap(), "skel-one", "row 0 untouched");
        assert_eq!(*slots[1].lock().unwrap(), "updated-two");

        // on_reveal: Some entries rewrite the slot, None entries leave it.
        handler.on_reveal(vec![Some("rev-one".into()), None]);
        assert_eq!(*slots[0].lock().unwrap(), "rev-one");
        assert_eq!(
            *slots[1].lock().unwrap(),
            "updated-two",
            "row 1 had data — reveal must not clobber it"
        );
    }

    /// Header + items get published in order. `output()` of the
    /// WorktreeSkimItem is the branch name so skim returns the correct
    /// identifier when the user hits Enter.
    #[test]
    fn skeleton_publishes_header_then_items() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "feat-a".into()),
            ListItem::new_branch("bbb".into(), "feat-b".into()),
        ];

        handler.on_skeleton(
            items,
            vec!["skel-a".into(), "skel-b".into()],
            header("Branch Status"),
        );

        let received: Vec<Arc<dyn SkimItem>> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(received.len(), 3);
        // Header emits empty output (not selectable).
        assert_eq!(received[0].output().as_ref(), "");
        // Branch items emit the branch name.
        assert_eq!(received[1].output().as_ref(), "feat-a");
        assert_eq!(received[2].output().as_ref(), "feat-b");

        // Shared state matches what was sent.
        let shared = handler.shared_items.lock().unwrap();
        assert_eq!(shared.len(), 3);
        assert_eq!(shared[1].output().as_ref(), "feat-a");
        assert_eq!(shared[2].output().as_ref(), "feat-b");
    }
}
