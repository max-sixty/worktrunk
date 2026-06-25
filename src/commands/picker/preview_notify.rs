//! Surfacing a background preview fill without a keystroke.
//!
//! skim 4.x renders a row's preview only when it *runs* a preview
//! (`Event::RunPreview`), which it produces on a selection change or a
//! preview-tab switch. The picker's preview panes are served from a shared
//! [`PreviewCache`](super::items::PreviewCache) that background workers fill
//! out-of-band (a `git diff HEAD`, a `git log`, a forge `gh pr view`). A fill
//! that lands *after* the `RunPreview` the keystroke produced has no event to
//! surface it: the finished content sits in the cache while the pane keeps
//! showing its "Loading…" placeholder until the next keystroke.
//!
//! `PreviewNotifier` closes that loop. The selected row records what its preview
//! is currently showing — `(row-key, mode)` — on every render via
//! [`note_awaiting`](PreviewNotifier::note_awaiting). When a background fill lands
//! on that exact key, [`notify_filled`](PreviewNotifier::notify_filled) injects an
//! `Event::RunPreview` through skim's own event sender, so skim re-reads the
//! now-warm cache and paints the content. A fill for any other key — an
//! off-screen row, or a tab the user isn't looking at — matches nothing and
//! injects nothing, so background pre-compute never re-runs the visible preview.
//!
//! Some panes aren't fed by an orchestrator cache fill: the `pr` / `comments`
//! panes read the row's live `pr_status`, and the diff tabs' dim state reads its
//! `local_content` — both mirrored by the collect handler's `on_update` as the
//! list pipeline lands. [`notify_row_changed`](PreviewNotifier::notify_row_changed)
//! covers those: it re-runs the selected row's preview (whatever tab is showing)
//! when that row's live data changes, so e.g. the `pr` tab flips from "Fetching
//! PR status…" to the resolved PR on its own.
//!
//! ## Why recording happens before the cache read
//!
//! `*SkimItem::preview` calls `note_awaiting` *before* it reads the cache. That
//! ordering is what makes the hand-off race-free: if the read misses (the
//! placeholder is returned), the fill that satisfies it necessarily completes
//! *after* that read, so it observes the awaited key already set and notifies.
//! The fill can't slip into the gap between "miss" and "record" because the
//! record precedes the read.

use std::sync::{Arc, Mutex, OnceLock};

use skim::prelude::Event;
use tokio::sync::mpsc::Sender;

use super::items::PreviewCacheKey;
use super::preview::PreviewMode;

/// Bridges a background [`PreviewCache`](super::items::PreviewCache) fill to
/// skim's event loop. See the module docstring for the full contract.
///
/// One instance is shared (via `Arc`) for the picker session: the
/// [`PreviewOrchestrator`](super::preview_orchestrator::PreviewOrchestrator) owns
/// it and notifies on every fill; the skim items hold a clone and record their
/// awaited preview on every render.
pub(super) struct PreviewNotifier {
    /// skim's event sender, published once `Skim::init_tui` has run (the same
    /// `OnceLock` the progressive handler pushes `Event::Render` through).
    /// `None` until then — an early fill (the speculative warm-up before skim is
    /// up) simply doesn't notify, which is harmless because skim hasn't rendered
    /// a preview that could strand yet.
    render_tx: Arc<OnceLock<Sender<Event>>>,
    /// The `(row-key, mode)` the selected row's preview is currently showing or
    /// awaiting. Written by `*SkimItem::preview` on every render; read on each
    /// background fill. The row-key is the branch name for worktree rows and the
    /// `pr:{N}` / `mr:{N}` token for `--prs` rows — exactly the
    /// [`PreviewCacheKey`] string, so a fill's key compares directly.
    awaiting: Mutex<Option<PreviewCacheKey>>,
}

impl PreviewNotifier {
    pub(super) fn new(render_tx: Arc<OnceLock<Sender<Event>>>) -> Self {
        Self {
            render_tx,
            awaiting: Mutex::new(None),
        }
    }

    /// Record the `(row-key, mode)` the selected row is rendering, so a matching
    /// background fill knows to surface itself. Called from `*SkimItem::preview`
    /// before it reads the cache (see the module docstring on ordering).
    pub(super) fn note_awaiting(&self, row_key: &str, mode: PreviewMode) {
        *self.awaiting.lock().unwrap() = Some((row_key.to_string(), mode));
    }

    /// Inject an `Event::RunPreview` if `key` is the preview the selected row is
    /// awaiting, so the just-filled content paints without a keystroke. A no-op
    /// when the key isn't the visible one (an off-screen or other-tab fill) or
    /// skim's sender isn't published yet. For the orchestrator's per-mode cache
    /// fills (diff / log / comments / summary).
    pub(super) fn notify_filled(&self, key: &PreviewCacheKey) {
        if self.awaiting.lock().unwrap().as_ref() == Some(key) {
            self.poke();
        }
    }

    /// Inject an `Event::RunPreview` if the selected row is `row_key` on *any*
    /// tab, so a change to that row's live data re-renders its preview without a
    /// keystroke. Unlike [`Self::notify_filled`] this matches the row regardless
    /// of mode, because the data it covers feeds several panes at once: the
    /// collect handler's `on_update` mirrors the live `pr_status` (the `pr` /
    /// `comments` panes) and `local_content` (the diff tabs' dim state), none of
    /// which is an orchestrator cache fill.
    pub(super) fn notify_row_changed(&self, row_key: &str) {
        if self
            .awaiting
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(k, _)| k == row_key)
        {
            self.poke();
        }
    }

    /// Push a `RunPreview` onto skim's event loop so it re-reads the selected
    /// row's preview. A no-op before skim's sender is published (the speculative
    /// warm-up before the TUI is up).
    fn poke(&self) {
        if let Some(tx) = self.render_tx.get() {
            let _ = tx.try_send(Event::RunPreview);
        }
    }

    /// A notifier with no event sender — used by tests that build skim items or
    /// an orchestrator without a live TUI. `notify_filled` is then a no-op.
    #[cfg(test)]
    pub(super) fn detached() -> Arc<Self> {
        Arc::new(Self::new(Arc::new(OnceLock::new())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `notify_row_changed` re-runs the selected row's preview on *any* tab when
    /// that row's live data lands (the `on_update` path), and stays silent for
    /// other rows — so a CI-status / diff-content update surfaces on the visible
    /// row without thrashing off-screen ones. (`notify_filled`'s exact-key
    /// scoping is covered by the orchestrator's `fill_notifies_only_awaited_key`.)
    #[test]
    fn notify_row_changed_pokes_only_the_selected_row() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(8);
        let render_tx = Arc::new(OnceLock::new());
        render_tx.set(tx).unwrap();
        let notifier = PreviewNotifier::new(render_tx);

        // The selected row is on the `pr` tab (still "Fetching PR status…").
        notifier.note_awaiting("feature", PreviewMode::Pr);

        // That row's CI status lands → re-run, even though the awaited tab (Pr)
        // isn't an orchestrator-filled mode.
        notifier.notify_row_changed("feature");
        assert!(
            matches!(rx.try_recv(), Ok(Event::RunPreview)),
            "the selected row's update re-runs its preview"
        );

        // A different row's update injects nothing.
        notifier.notify_row_changed("other");
        assert!(
            rx.try_recv().is_err(),
            "an off-screen row's update doesn't thrash the visible preview"
        );
    }
}
