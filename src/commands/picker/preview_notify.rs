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
    /// skim's sender isn't published yet.
    pub(super) fn notify_filled(&self, key: &PreviewCacheKey) {
        if self.awaiting.lock().unwrap().as_ref() != Some(key) {
            return;
        }
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
