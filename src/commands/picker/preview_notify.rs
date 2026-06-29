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
//! Two panes are fed by live row data rather than an orchestrator cache fill:
//! the `pr` and `comments` panes render from the row's live `pr_status`,
//! mirrored by the collect handler's `on_update` as the `CiStatus` fetch lands.
//! [`notify_pr_status_changed`](PreviewNotifier::notify_pr_status_changed) covers
//! them: when that row is selected, it re-runs the preview so e.g. the `pr` tab
//! flips from "Fetching PR status…" to the resolved PR on its own.
//!
//! It re-runs only when the *visible* tab's body would actually differ, because
//! a re-run resets the preview scroll to the top (skim clears it on every
//! content swap), and yanking the user out of a scrolled pane to repaint
//! identical bytes is the bug this guards against. That makes the gate
//! per-tab, matching what each body reads:
//!
//! - **diff / log / summary tabs** — body comes from the orchestrator cache, not
//!   `pr_status`, so an `on_update` never re-runs them (their fills ride
//!   `notify_filled`). `on_update` also mirrors `local_content`, but that only
//!   re-dims the diff tabs' tab-bar number — chrome the user can wait a keystroke
//!   for, never worth a scroll reset.
//! - **`pr` tab** — body renders every shown `PrStatus` field, so it re-runs on
//!   any real change. The collect handler excludes `is_priming` (a list-cell dim
//!   hint the pane never draws — see `items::pr_status_pane_eq`) so a
//!   cache-prime→live flip that only clears it doesn't reset scroll.
//! - **`comments` tab** — body is the branch-keyed thread (surfaced by
//!   `notify_filled`), invariant to `PrStatus` fields once a PR exists, so it
//!   re-runs only on a [`PrPresence`](super::items::PrPresence) change
//!   (Loading/NoPr/HasPr), not on every field.
//!
//! The collect handler computes those two change signals and passes them as a
//! [`PrStatusDelta`]; this notifier maps the awaited tab to the one it cares
//! about.
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

/// What about a row's live `pr_status` changed, at the granularity each
/// `pr_status`-backed tab's body cares about. The collect handler computes both
/// from the old and new slot values; [`PreviewNotifier::notify_pr_status_changed`]
/// reads the one matching the visible tab.
#[derive(Debug, Clone, Copy)]
pub(super) struct PrStatusDelta {
    /// The rendered `pr` pane would differ — any shown field changed (CI badge,
    /// title, body, review, …), ignoring the `is_priming` dim hint the pane
    /// never draws. Gates the `pr` tab. See `items::pr_status_pane_eq`.
    pub(super) pane_changed: bool,
    /// The [`PrPresence`](super::items::PrPresence) (Loading/NoPr/HasPr) changed.
    /// Gates the `comments` tab, whose body is otherwise the branch-keyed thread
    /// surfaced by [`PreviewNotifier::notify_filled`]. Implies `pane_changed`.
    pub(super) presence_changed: bool,
}

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

    /// Inject an `Event::RunPreview` if the selected row is `row_key` *and* the
    /// part of `pr_status` its visible tab renders actually changed, so the live
    /// `CiStatus` fetch surfaces on its own without resetting the scroll of an
    /// unchanged pane. `delta` carries the two per-tab change signals the collect
    /// handler computed (see [`PrStatusDelta`]); a diff / log / summary tab reads
    /// neither, so it matches nothing and injects nothing.
    pub(super) fn notify_pr_status_changed(&self, row_key: &str, delta: PrStatusDelta) {
        let relevant = {
            let awaiting = self.awaiting.lock().unwrap();
            awaiting.as_ref().is_some_and(|(k, mode)| {
                k == row_key
                    && match mode {
                        PreviewMode::Pr => delta.pane_changed,
                        PreviewMode::Comments => delta.presence_changed,
                        _ => false,
                    }
            })
        };
        if relevant {
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

    /// `notify_pr_status_changed` re-runs the selected row's preview only when
    /// the *visible* tab's own change signal fired: the `pr` tab on a pane
    /// change, the `comments` tab on a presence change, and never a diff tab.
    /// It stays silent for other rows. (`notify_filled`'s exact-key scoping is
    /// covered by `fill_notifies_only_awaited_key`.)
    #[test]
    fn notify_pr_status_changed_pokes_only_the_visible_tabs_signal() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(8);
        let render_tx = Arc::new(OnceLock::new());
        render_tx.set(tx).unwrap();
        let notifier = PreviewNotifier::new(render_tx);

        let pane_only = PrStatusDelta {
            pane_changed: true,
            presence_changed: false,
        };
        let presence_too = PrStatusDelta {
            pane_changed: true,
            presence_changed: true,
        };
        let drain = |rx: &mut tokio::sync::mpsc::Receiver<Event>| {
            let mut n = 0;
            while let Ok(Event::RunPreview) = rx.try_recv() {
                n += 1;
            }
            n
        };

        // `pr` tab: any pane change re-runs it (e.g. "Fetching…" → resolved PR).
        notifier.note_awaiting("feature", PreviewMode::Pr);
        notifier.notify_pr_status_changed("feature", pane_only);
        assert_eq!(drain(&mut rx), 1, "a pr-pane change re-runs the pr tab");

        // `comments` tab: a pane-only change (no presence flip) does NOT re-run —
        // its body is the unchanged branch-keyed thread, so re-running would only
        // reset the scroll. A presence change (e.g. NoPr → HasPr) does re-run.
        notifier.note_awaiting("feature", PreviewMode::Comments);
        notifier.notify_pr_status_changed("feature", pane_only);
        assert_eq!(
            drain(&mut rx),
            0,
            "a pane-only change must not reset a scrolled comments thread"
        );
        notifier.notify_pr_status_changed("feature", presence_too);
        assert_eq!(
            drain(&mut rx),
            1,
            "a presence change re-runs the comments tab"
        );

        // A diff tab reads neither signal → never re-runs (preserves diff scroll).
        notifier.note_awaiting("feature", PreviewMode::WorkingTree);
        notifier.notify_pr_status_changed("feature", presence_too);
        assert_eq!(
            drain(&mut rx),
            0,
            "a CI update while a diff tab is showing must not reset its scroll"
        );

        // A different row's update injects nothing.
        notifier.note_awaiting("feature", PreviewMode::Pr);
        notifier.notify_pr_status_changed("other", presence_too);
        assert_eq!(
            drain(&mut rx),
            0,
            "an off-screen row's update doesn't thrash the visible preview"
        );
    }
}
