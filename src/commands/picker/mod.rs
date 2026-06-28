//! Interactive branch/worktree selector.
//!
//! A skim-based TUI for selecting and switching between worktrees. The picker
//! shares `super::list::collect::collect` with `wt list` — see
//! `commands/list/collect/mod.rs` for the rendering-pipeline spec — but inverts
//! the ordering because skim's `preview_window` height is baked into
//! `SkimOptions` before skim takes over the terminal, so we have
//! to estimate the visible row count up front rather than learn it from
//! collect's skeleton pass.
//!
//! # "Skeleton"
//!
//! Same meaning as in `wt list`: the column/row frame with placeholder cells
//! the user sees first. In the picker, `collect::collect` builds those rows
//! and streams them via `on_skeleton` → `PickerHandler` → `SkimItemSender` →
//! skim. (Not to be confused with the rendered skeleton-row *strings* that
//! flow through that channel.)
//!
//! # Startup flow
//!
//! On the main thread, `handle_picker`:
//!
//! 1. `current_or_recover` + config resolution.
//! 2. Reads the terminal size once; `PreviewState::new` records the
//!    Right-vs-Down layout detected from it. Every later sizing step (the
//!    estimate cap, preview dimensions, half-page scroll) reuses that snapshot.
//! 3. Allocates the `PreviewOrchestrator` and kicks off a *speculative*
//!    `git diff HEAD` for the current worktree on `COLLECT_POOL`.
//!    That bg work overlaps with everything below.
//! 4. Computes `num_items_estimate` — `list_worktrees` plus (conditionally)
//!    `local_branches` / `remote_branches`, capped at the Down layout's
//!    `max_visible_items(available)`. Only used to size skim's `preview_window`.
//! 5. Builds `SkimOptions` (immutable after this — which is why steps 1-4 have
//!    to run first).
//! 6. Spawns the `picker-collect` bg thread, which calls `collect::collect`.
//! 7. Calls `run_skim(rx)` (a thin wrapper over skim's `init`/`run` that also
//!    hands the collect handler skim's event sender for progressive repaints);
//!    skim paints the empty frame and then ingests skeleton rows from the
//!    channel as the bg thread streams them via `on_skeleton`.
//!
//! Time-to-skeleton = steps 1-6 on the main thread *plus* collect's
//! pre-skeleton phase on the bg thread. See `commands/list/collect/mod.rs`
//! § "Forks on the Critical Path" for the subprocess inventory (five
//! forks, plus one more in `extensions.worktreeConfig` repos).
//!
//! ## Phase timings
//!
//! Representative medians on the worktrunk dev repo (7 worktrees, 6 branches,
//! warm caches, release build).
//!
//! | Phase (instant-to-instant) | median | cmds |
//! |-----------------------------|-------:|-----:|
//! | `Picker started → Picker config resolved` | ~16ms | 3 |
//! | `Picker config resolved → Picker layout detected` | <1ms | 0 |
//! | `Picker layout detected → Picker estimate computed` | ~39ms | 11 (includes bg preview `git diff`s) |
//! | `Picker estimate computed → Picker skim options built` | <1ms | 0 |
//! | `Picker skim options built → Picker collect spawned` | <100µs | 0 |
//! | `Picker collect spawned → List collect started` | <100µs | 0 |
//! | `List collect started → Skeleton rendered` (bg, pre-skeleton) | ~41ms | 25 |
//! | **Time-to-skeleton** (≈ main-thread prelude + bg pre-skeleton) | **~96ms** | |
//! | `Skeleton rendered → Spawning worker thread` (post-skeleton, pre-work) | ~156ms | 86 |
//! | `Parallel execution started → All results drained` (post-skeleton work) | ~1.1s | 254 |
//! | Wall clock under `WORKTRUNK_PICKER_DRY_RUN=1` (median / p95) | ~1.4s / ~4.4s | |
//!
//! Skim's own paint cost isn't observable from the dry-run path — skim is
//! bypassed there.
//!
//! ### Reproducing
//!
//! End-to-end time-to-first-output (criterion, synthetic repo):
//!
//! ```bash
//! cargo bench --bench time_to_first_output -- switch
//! ```
//!
//! Preview pre-compute workload — spawn → all preview tasks drained,
//! skim bypassed (criterion, synthetic repo):
//!
//! ```bash
//! cargo bench --bench picker_preview
//! ```
//!
//! Per-phase breakdown on a specific repo (a single trace is usually enough
//! to spot where time goes; re-run a few times if you want variance):
//!
//! ```bash
//! RUST_LOG=debug ./target/release/wt -C <repo> switch \
//!   2> >(cargo run -p wt-perf --release -q -- trace > trace.json)
//! # Open trace.json in Perfetto, or run the phase-duration SQL query
//! # documented in benches/CLAUDE.md §"What's on the critical path?".
//! ```

mod items;
mod log_formatter;
mod os;
mod pager;
mod pr_pane;
mod preview;
pub(crate) mod preview_cache;
mod preview_notify;
mod preview_orchestrator;
mod progressive_handler;
mod prs;
mod summary;

use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use ansi_str::AnsiStr;
use anyhow::Context;
use color_print::cformat;
// bounded/unbounded/Sender are re-exported by skim::prelude
use skim::prelude::*;
use skim::reader::CommandCollector;
use skim::tui::event::ActionCallback;
use worktrunk::HookType;
use worktrunk::config::Approvals;
use worktrunk::git::{ErrorExt, Repository, current_or_recover};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, hint_message, info_message, strip_osc8_hyperlinks, warning_message,
};

use super::hook_plan::{ApprovedHookPlan, HookPlanBuilder};
use super::hooks::HookAnnouncer;
use super::list::collect;
use super::list::model::{BranchScope, ItemKind, ListItem};
use super::list::progressive::RenderTarget;
use super::list::render::PLACEHOLDER;
use super::repository_ext::{RemoveTarget, RepositoryCliExt};
use super::worktree::{RemoveResult, SwitchPipeline};
use crate::cli::SwitchFormat;
use crate::output::{BackgroundFallbackMode, handle_remove_output};
use worktrunk::git::{BranchDeletionMode, delete_branch_if_safe};

use items::{LocalContent, LocalContentSlot, PreviewCache, ShortcutTable, WORKTREE_OUTPUT_PREFIX};
use preview::{PreviewLayout, PreviewMode, PreviewState, PreviewStateData};
use preview_orchestrator::PreviewOrchestrator;

/// Drain stashed warnings to stderr. Called after skim has released the
/// terminal (or in the dry-run path after the bg thread joins) — eprintln
/// during the picker would corrupt skim's frame, so collect routes warnings
/// through `PickerProgressHandler::stash_warning` and we emit them here.
///
/// TODO(picker-feedback): the declined-removal diagnostics (the main-worktree /
/// dirty / unmerged "can't remove this row" messages from the `alt-x` keep paths)
/// only surface here, on exit — the user presses `alt-x`, the row visibly stays,
/// and the reason scrolls past after they quit. Consider a short in-picker message
/// at `alt-x` time so the *why* lands immediately. skim has no footer slot (see the
/// dropped Stall-indicator work), so the realistic slot is the header line — swap
/// it to a transient "main worktree can't be removed" for a beat, then restore. The
/// stash stays the fallback for background failures that surface after exit.
fn drain_stashed_warnings(stash: &Mutex<Vec<String>>) {
    for line in stash.lock().unwrap().drain(..) {
        eprintln!("{line}");
    }
}

/// Action selected by the user in the picker.
enum PickerAction {
    /// Switch to the selected worktree (Enter key).
    Switch,
    /// Create a new worktree from the search query (alt-c).
    Create,
}

/// The alt-x removal target parsed back out of a row's `output()` token.
///
/// A worktree-backed row's token is `worktree-path:<path>` (paths are
/// unique — detached worktrees would otherwise collide on the shared
/// `(detached)` label); a branch-only row's token is the bare branch name.
enum PickerRemovalTarget {
    WorktreePath(PathBuf),
    Branch(String),
}

impl PickerRemovalTarget {
    fn from_signal(signal: &str) -> Option<Self> {
        let signal = signal.trim();
        if signal.is_empty() {
            return None;
        }
        if let Some(path) = signal.strip_prefix(WORKTREE_OUTPUT_PREFIX) {
            if path.is_empty() {
                return None;
            }
            return Some(Self::WorktreePath(PathBuf::from(path)));
        }
        Some(Self::Branch(signal.to_string()))
    }
}

/// Resolve the switch identifier for a selected picker row, decoded from its
/// `output()` token: the worktree path for any worktree-backed row, the branch
/// name for a branch-only row.
///
/// `wt switch` accepts a worktree path for any existing worktree (`plan_switch`
/// phase 2b), so a worktree-backed row always switches by its unique path —
/// detached *and* branched alike. A branch-only row has no worktree, so its
/// branch name is the only handle.
///
/// Decoding `output()` rather than `downcast_ref::<PickerRow>()` also
/// sidesteps skim's cross-thread `TypeId` mismatch, which can make the
/// downcast fail when the item originates on the reader thread.
fn picker_item_identifier(item: &dyn SkimItem) -> String {
    let output = item.output().to_string();
    match PickerRemovalTarget::from_signal(&output) {
        Some(PickerRemovalTarget::WorktreePath(path)) => path.to_string_lossy().into_owned(),
        _ => output,
    }
}

/// skim's [`CommandCollector`] for the picker's `reload` actions. Only `alt-r`
/// (`reload(refresh)`) reaches it now — `alt-x` removal runs synchronously through
/// [`AltXRemover`] instead of a `reload` (see its docs). `invoke` re-runs the
/// collect pipeline for a refresh and otherwise re-streams the current rows.
struct PickerCollector {
    /// The picker's row list (shared with the handler's `shared_items` and the
    /// [`AltXRemover`]). `invoke` re-streams it when a `reload` isn't a refresh.
    items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    /// Re-runs the collect pipeline for the `alt-r` refresh: `reload(refresh)`
    /// routes here, and [`PipelineFactory::spawn`] streams a fresh item list
    /// back. Shared (`Rc`) with `handle_picker`, which used it for the initial
    /// spawn.
    factory: Rc<PipelineFactory>,
}

/// What an `alt-x` press did to the selected row, so the keybinding callback knows
/// how to refresh skim's view (see [`AltXRemover::apply`] and
/// [`install_remove_keybinding`]).
enum RemovalEffect {
    /// The row left the list (`items` shrank): the callback resyncs skim's pool
    /// from the shrunk list ([`resync_pool`]).
    Dropped,
    /// The row stayed but its content changed (morphed to `/ branch` in place):
    /// the callback repaints it and refreshes its preview.
    Morphed,
    /// The row stayed unchanged (the removal was declined or kept): the callback
    /// just re-anchors and repaints.
    Kept,
}

/// Runs `alt-x` removal for the selected picker row, **synchronously on skim's
/// event loop** rather than through skim's `reload`.
///
/// # Why not `reload`
///
/// `alt-x` used to be `reload(remove {})`. skim's `handle_reload` clears the item
/// pool and restarts the matcher *before* the new rows stream in, so the matcher
/// runs once against the empty pool, `Replace`s `item_list` with nothing, and
/// skim's render clamp resets the cursor to the top (`current = 0`). A
/// `reposition` action then snapped it back — but for the frames in between, the
/// `>` pointer flashed to the top row. The fix removes the `reload`: the keybinding
/// callback mutates the row list and rebuilds the pool itself ([`resync_pool`]) so
/// the matcher only ever sees the post-removal list (never empty) and the cursor
/// holds its slot. The row that slides into the removed row's place lands under the
/// cursor for free, with no flash.
///
/// # Send
///
/// The callback skim runs for a keybinding must be `Send`, so this holds only
/// `Send` state (every field is an `Arc`, or a `Repository`, which is `Send`) — it
/// can't carry the collector's `Rc<PipelineFactory>`. It owns the morph/keep
/// shared slots directly instead of reaching them through the factory.
///
/// Git operations (worktree removal, branch deletion) still run on a background
/// thread — `apply` is on skim's event loop and blocking it would freeze the TUI.
/// The row is mutated optimistically; if the background removal finds the target
/// survived ([`removal_target_still_present`]) it restores the row
/// ([`restore_failed_removal`] / [`revert_morph`]) and stashes why.
struct AltXRemover {
    /// The picker's row list (shared with [`PickerCollector`] and the handler).
    /// `apply` drops a row from it for the drop path; the callback then rebuilds
    /// skim's pool from it.
    items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    repo: Repository,
    /// Approvals snapshot, loaded once at picker startup. A queued removal runs
    /// its `pre-remove` / `post-remove` / `post-switch` hooks only when every
    /// one is in here — the picker can't show an approval prompt mid-render, so
    /// unapproved project commands are skipped, never run. See
    /// [`approved_removal_plan`].
    approvals: Arc<Approvals>,
    /// skim's event sender, published once the TUI is initialized (same
    /// `OnceLock` the progressive handler pushes `Event::Render` through). A
    /// background removal that fails injects a [`resync_pool_action`] through it to
    /// re-show the restored row. `None` until the TUI is up — but `alt-x` can only
    /// fire after skim is showing rows, so it's always set by then.
    render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    /// Same warning stash the progressive handler fills (drained to stderr once
    /// skim releases the terminal). A failed background removal pushes a
    /// `worktree kept` warning here so the user learns the row that flickered
    /// back (or un-morphed) didn't actually go away. See [`restore_failed_removal`]
    /// and [`revert_morph`].
    stashed_warnings: Arc<Mutex<Vec<String>>>,
    /// `alt-y` / `alt-o` lookup table (token → branch + URL). A morph re-keys the
    /// row's entry from the worktree token to the branch token. Shared with the
    /// handler (which fills it) and the shortcut keybindings (which read it).
    shortcut_table: ShortcutTable,
    /// The picker's full-width layout, handed over once the rows land. A morph
    /// renders the `/ branch` row on this grid so it lines up with the worktree
    /// rows. Shared with the handler (which fills it).
    layout_slot: Arc<Mutex<Option<crate::commands::list::layout::LayoutConfig>>>,
}

impl AltXRemover {
    /// Build removal state from a fresh `Repository` so picker reloads after a
    /// background removal do not reuse the startup worktree inventory cache.
    ///
    /// `target` carries the exact worktree path or branch name decoded from
    /// the row's `output()` token — no `git worktree list` lookup, so a
    /// detached row can't be confused with another detached row.
    fn prepare_removal(
        &self,
        target: &PickerRemovalTarget,
    ) -> anyhow::Result<(Repository, RemoveResult)> {
        let repo = Repository::at(self.repo.discovery_path())?;

        // Validate removal before touching the list. prepare_worktree_removal
        // runs a few git commands (~15-20ms) — acceptable on skim's event loop.
        // Only remove the item and spawn background deletion if this succeeds.
        let caller_path = repo.current_worktree().root().ok();

        let result = {
            let config = repo.user_config();
            let remove_target = match target {
                PickerRemovalTarget::WorktreePath(path) => RemoveTarget::Path(path),
                PickerRemovalTarget::Branch(branch) => RemoveTarget::Branch(branch),
            };
            repo.prepare_worktree_removal(
                remove_target,
                BranchDeletionMode::SafeDelete,
                false,
                config,
                caller_path,
                None,
                None,
            )?
        };

        Ok((repo, result))
    }

    /// Execute a queued removal in the background.
    ///
    /// A `RemovedWorktree` result goes through [`handle_remove_output`] in its
    /// silent (TUI) mode — the git worktree removal with no `wt`-generated
    /// messages, spinner, or `cd` directive (skim owns the terminal). Its
    /// `pre-remove` / `post-remove` / `post-switch` hooks run only when they're
    /// already approved ([`approved_removal_plan`] — a read-only `Approvals`
    /// filter, no prompt): the picker can't prompt mid-render, so unapproved
    /// project commands are dropped from the plan, never run. (A hook that
    /// *does* run still
    /// streams its own output to stderr, like any hook — a rough edge of
    /// removing inside the picker.) A `BranchOnly` result just deletes the
    /// branch if it's safe to.
    ///
    /// Called from a background thread after the picker optimistically removes
    /// the item from the list, so the whole operation runs off skim's event loop
    /// and the TUI stays responsive. Only reached for a removal
    /// [`removal_will_remove_target`] predicts will remove the target — a
    /// predictably-kept unmerged branch never gets here. The caller does not infer
    /// the outcome from this `Result` — a removal can fail before *or* after the
    /// worktree is physically gone (rendering or spawning a
    /// `post-remove`/`post-switch` hook can error during the announcer flush, which
    /// runs after the dir is renamed into `.git/wt/trash/`), and a `BranchOnly`
    /// delete that raced from integrated to unmerged returns `Ok` with the branch
    /// surviving. Instead it
    /// observes whether the target still exists ([`removal_target_still_present`])
    /// and restores the row via [`restore_failed_removal`] only when it does, so
    /// the list never shows a removal that didn't happen. The `Result` is for
    /// logging.
    ///
    /// `repo` is the worktree the picker is operating from — the config source
    /// for the removal hooks (see [`approved_removal_plan`]) and the target of
    /// a `BranchOnly` deletion. `RemovedWorktree` removal itself is rooted at
    /// `main_path` (which may differ from the picker's startup repo in bare-repo
    /// setups).
    fn do_removal(
        repo: &Repository,
        result: &RemoveResult,
        approvals: &Approvals,
    ) -> anyhow::Result<()> {
        match result {
            RemoveResult::RemovedWorktree {
                main_path,
                worktree_path,
                ..
            } => {
                let main_repo = Repository::at(main_path)?;
                let plan = approved_removal_plan(repo, main_path, worktree_path, approvals)?;
                let mut announcer = HookAnnouncer::new(&main_repo, false);
                handle_remove_output(
                    result,
                    /* foreground */ true,
                    &plan,
                    /* quiet */ true,
                    /* silent */ true,
                    &mut announcer,
                    BackgroundFallbackMode::Detached,
                )?;
                announcer.flush()?;
            }
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
                ..
            } => {
                if !deletion_mode.should_keep() {
                    let default_branch = repo.default_branch();
                    let target = default_branch.as_deref().unwrap_or("HEAD");
                    if let Ok(snapshot) = repo.capture_refs()
                        && let Err(e) = delete_branch_if_safe(
                            repo,
                            &snapshot,
                            branch_name,
                            target,
                            deletion_mode.is_force(),
                        )
                    {
                        // A safe-delete refusal is `Ok(NotDeleted)`, not an error;
                        // this is a genuine `git branch -D` failure. The row is
                        // restored anyway because the branch still exists (see
                        // `removal_target_still_present`) — surface the cause.
                        tracing::warn!(branch = %branch_name, error = %e, "picker: failed to delete branch '{branch_name}': {e:#}");
                    }
                }
            }
        }
        Ok(())
    }

    /// Drop the selected row and remove its target on a background thread.
    ///
    /// For a removal that will remove the row entirely — a worktree whose branch
    /// is *also* deleted (integrated, or force), or a force-deleted branch-only
    /// row. A worktree removal that *keeps* its branch never reaches here; that's
    /// the in-place morph ([`morph_and_remove_in_background`](Self::morph_and_remove_in_background)).
    /// The `output()` token is unique per row (a `worktree-path:` path for
    /// worktrees), so this drops exactly the selected row even when several
    /// detached rows share the `(detached)` branch label.
    ///
    /// The row drops optimistically so the list stays snappy; the git work runs on
    /// a background thread off skim's event loop. The dropped row is restored only
    /// when the target survives — observed directly ([`removal_target_still_present`]),
    /// not inferred from `do_removal`'s `Result`, which is `Err` after a successful
    /// removal whose `post-remove` hook fails to render/spawn. This keeps the list
    /// from showing a removal that didn't happen without ever resurrecting a row
    /// for a target that's actually gone.
    fn drop_and_remove_in_background(
        &self,
        selected_output: String,
        planning_repo: Repository,
        result: RemoveResult,
    ) {
        // Capture the removed row (and its position) before dropping it: the
        // position is handed to the background thread so it can put the row back
        // at its slot if the removal fails (see `restore_failed_removal`). The
        // cursor needs no separate repositioning — the caller rebuilds skim's pool
        // from this shrunk list ([`resync_pool`]) without a `reload`, so `current`
        // holds its index and the row that slides up into the removed slot lands
        // under the cursor for free, query or no query.
        let removed = {
            let mut items = self.items.lock().unwrap();
            let removed = items
                .iter()
                .position(|item| item.output().as_ref() == selected_output)
                .map(|pos| (Arc::clone(&items[pos]), pos));
            items.retain(|item| item.output().as_ref() != selected_output);
            removed
        };

        // A user-facing (label, noun) for the `kept` message, taken from the result
        // before it moves into the background thread.
        let (removal_label, removal_noun) = removal_failure_subject(&result);

        let repo = planning_repo.clone();
        let approvals = Arc::clone(&self.approvals);
        let items = Arc::clone(&self.items);
        let render_tx = Arc::clone(&self.render_tx);
        let stashed_warnings = Arc::clone(&self.stashed_warnings);
        let _ = std::thread::Builder::new()
            .name(format!("picker-remove-{selected_output}"))
            .spawn(move || {
                if let Err(e) = Self::do_removal(&repo, &result, &approvals) {
                    tracing::warn!(selected_output = %selected_output, error = %e, "picker: removal of '{selected_output}' errored: {e:#}");
                }
                // A removal that keeps its branch never reaches here — that's the
                // morph path (`morph_and_remove_in_background`). So a surviving
                // target means the removal itself failed: put the row back.
                if removal_target_still_present(&repo, &result)
                    && let Some((item, pos)) = removed
                {
                    restore_failed_removal(
                        &items,
                        &render_tx,
                        &stashed_warnings,
                        item,
                        pos,
                        &removal_label,
                        removal_noun,
                    );
                }
            });
    }

    /// Keep the selected row in place and explain why its target wasn't removed.
    ///
    /// Called from [`apply`](Self::apply) when [`removal_will_remove_target`]
    /// predicts the removal would keep the target — a branch-only row whose branch
    /// is unmerged, which `SafeDelete` declines to delete (data safety). Deciding
    /// this up front from `prepare_removal`'s already-computed integration check
    /// means the row never drops (no flicker) and no background `do_removal` runs
    /// for a no-op. The row stays in its slot under the (un-reset) cursor; this just
    /// stashes the canonical "retained; unmerged" info + hint pair `wt remove`
    /// itself prints (see `print_retained_unmerged_branch`), deduped and drained to
    /// stderr when the picker exits. (This is a by-design retain, not a failure —
    /// distinct from [`restore_failed_removal`]'s `kept … could not remove it`
    /// warning.)
    fn keep_unremovable_row(&self, branch_name: &str) {
        // The canonical "retained; unmerged" info + hint `wt remove` prints,
        // shared so the picker copy can't drift (see
        // `stash_retained_unmerged_branch`). Taking the branch name (not the whole
        // `RemoveResult`) makes it unrepresentable for this keep path to be handed
        // a `RemovedWorktree`, which always removes — see the dispatch in
        // [`apply`](Self::apply) and [`removal_will_remove_target`].
        stash_retained_unmerged_branch(&self.stashed_warnings, branch_name);
    }

    /// Keep the current worktree's row in place and explain why the picker won't
    /// remove it.
    ///
    /// Called from [`apply`](Self::apply) when [`removal_targets_current_worktree`]
    /// is true — alt-x on the worktree the picker was launched from. Removing it
    /// would have to switch the shell elsewhere first (see
    /// `removal_targets_current_worktree` for why that's disruptive mid-picker), so
    /// the row stays put and a hint to switch away first is stashed, drained to
    /// stderr when the picker exits. The row never drops and no `do_removal` runs,
    /// so this is the only removal path that never reaches a background thread.
    fn keep_current_worktree_row(&self) {
        stash_current_worktree_hint(&self.stashed_warnings);
    }

    /// Morph the selected worktree row into a `/ branch` row in place, then remove
    /// the worktree on a background thread.
    ///
    /// For a `RemovedWorktree` removal that [`worktree_removal_keeps_branch`]
    /// predicts will keep its (unmerged) branch. The row never leaves its slot:
    /// the morph rewrites the row's shared `rendered` line to the branch line
    /// (rendered on the live layout — gutter `+` → `/`, path blank), flips the
    /// row's [`morphed`](items::LocalCheckout::morphed) flag (so `output()`
    /// becomes the branch token), dims the `working_tree` preview tab (no worktree
    /// left to diff), and re-keys the row's `alt-y`/`alt-o` shortcut entry to the
    /// branch token. skim repaints just that row, and the (un-reset) cursor holds
    /// the same slot — no teleport, no reset.
    ///
    /// The morph is optimistic, like the drop path. The background thread runs the
    /// git removal and, only if the worktree unexpectedly survives
    /// ([`removal_target_still_present`] — a clean-check race, a locked dir, a
    /// failing `pre-remove` hook), reverts the morph back to the worktree row via
    /// [`revert_morph`] and surfaces why. (The branch can't flip integrated in the
    /// millisecond between the prediction and the delete, so the only realistic
    /// failure is the worktree removal itself.)
    ///
    /// Returns [`RemovalEffect::Morphed`] on the in-place morph, or
    /// [`RemovalEffect::Dropped`] when it falls back to
    /// [`drop_and_remove_in_background`](Self::drop_and_remove_in_background) —
    /// the row carries no [`MorphHandle`](items::MorphHandle) or the layout hasn't
    /// landed, so the worktree still removes but the row drops instead of morphing.
    fn morph_and_remove_in_background(
        &self,
        selected_output: String,
        branch: String,
        planning_repo: Repository,
        result: RemoveResult,
    ) -> RemovalEffect {
        // Gather the row's shared morph handles and render the branch line on the
        // live layout. Any gap (row not morphable, layout not yet handed over)
        // means no clean in-place morph — drop the row instead, same end state.
        let default_branch = self.repo.default_branch();
        let prepared = {
            let table = self.shortcut_table.lock().unwrap();
            let layout = self.layout_slot.lock().unwrap();
            match (
                table.get(&selected_output).and_then(|d| d.morph.as_ref()),
                layout.as_ref(),
            ) {
                (Some(handle), Some(layout)) => {
                    let (branch_line, branch_local) =
                        build_morph_branch_row(layout, &handle.item, default_branch.as_deref());
                    Some(MorphSlots {
                        rendered: Arc::clone(&handle.rendered),
                        morphed: Arc::clone(&handle.morphed),
                        local_content: Arc::clone(&handle.local_content),
                        branch_line,
                        branch_local,
                    })
                }
                _ => None,
            }
        };
        let Some(slots) = prepared else {
            self.drop_and_remove_in_background(selected_output, planning_repo, result);
            return RemovalEffect::Dropped;
        };

        // Snapshot the pre-morph display for the revert, then apply the morph.
        let original_rendered = slots.rendered.lock().unwrap().clone();
        let original_local = *slots.local_content.lock().unwrap();
        *slots.rendered.lock().unwrap() = slots.branch_line;
        slots.morphed.store(true, Ordering::Relaxed);
        *slots.local_content.lock().unwrap() = slots.branch_local;

        // Re-key the `alt-y`/`alt-o` lookup to the branch token (the row's new
        // `output()`); the revert moves it back.
        {
            let mut table = self.shortcut_table.lock().unwrap();
            if let Some(data) = table.remove(&selected_output) {
                table.insert(branch.clone(), data);
            }
        }

        let repo = planning_repo.clone();
        let approvals = Arc::clone(&self.approvals);
        let render_tx = Arc::clone(&self.render_tx);
        let stashed_warnings = Arc::clone(&self.stashed_warnings);
        let shortcut_table = Arc::clone(&self.shortcut_table);
        let revert = MorphRevert {
            rendered: slots.rendered,
            original_rendered,
            morphed: slots.morphed,
            local_content: slots.local_content,
            original_local,
            shortcut_table,
            branch_token: branch.clone(),
            worktree_token: selected_output.clone(),
        };
        let _ = std::thread::Builder::new()
            .name(format!("picker-morph-{branch}"))
            .spawn(move || {
                if let Err(e) = Self::do_removal(&repo, &result, &approvals) {
                    tracing::warn!(branch = %branch, error = %e, "picker: removal of '{branch}' worktree errored: {e:#}");
                }
                // Only the worktree removal can realistically fail here; if it did,
                // the worktree dir survives — undo the morph and say so.
                if removal_target_still_present(&repo, &result) {
                    revert_morph(revert, &stashed_warnings, &render_tx);
                }
            });

        RemovalEffect::Morphed
    }

    /// Run the `alt-x` removal dispatch for the selected row.
    ///
    /// Decides up front, from `prepare_removal`'s already-computed result, what the
    /// removal does to the row, mutates the picker's row list / shared row state
    /// accordingly, and kicks off the background git work. Returns the
    /// [`RemovalEffect`] so the keybinding callback can refresh skim's view:
    ///   - targets the current worktree → keep it (removing the worktree you're
    ///     standing in has to switch you away first, which the picker declines);
    ///   - keeps its (unmerged) branch → morph to `/ branch` in place;
    ///   - removes the target → drop the row;
    ///   - branch-only row whose branch is unmerged → stays put, explained.
    ///
    /// Runs on skim's event loop (the `alt-x` keybinding callback), so the row
    /// mutation and the caller's pool rebuild ([`resync_pool`]) are atomic from
    /// skim's view — no `reload`, so the cursor never resets. The `~15-20ms`
    /// `prepare_removal` git work is the same cost the old `reload`-time dispatch
    /// paid; the actual worktree/branch deletion is deferred to a background thread.
    fn apply(&self, selected_output: String) -> RemovalEffect {
        let Some(removal_target) = PickerRemovalTarget::from_signal(&selected_output) else {
            return RemovalEffect::Kept;
        };
        match self.prepare_removal(&removal_target) {
            Ok((planning_repo, result)) => {
                if removal_targets_current_worktree(&result) {
                    self.keep_current_worktree_row();
                    RemovalEffect::Kept
                } else if let Some(branch) = worktree_removal_keeps_branch(&planning_repo, &result)
                {
                    self.morph_and_remove_in_background(
                        selected_output,
                        branch,
                        planning_repo,
                        result,
                    )
                } else if removal_will_remove_target(&result) {
                    self.drop_and_remove_in_background(selected_output, planning_repo, result);
                    RemovalEffect::Dropped
                } else {
                    // The only non-removing outcome: `removal_will_remove_target`
                    // returns false solely for an unmerged `BranchOnly` row (a
                    // `RemovedWorktree` always removes, so it never reaches here), so
                    // this arm is always that row — keep it, explained.
                    // `keep_unremovable_row` taking the branch name — not the whole
                    // result — keeps that narrowing at the type level.
                    if let RemoveResult::BranchOnly { branch_name, .. } = &result {
                        self.keep_unremovable_row(branch_name);
                    }
                    RemovalEffect::Kept
                }
            }
            Err(e) => {
                tracing::info!(selected_output = %selected_output, error = %e, "picker: cannot remove '{selected_output}': {e:#}");
                // The target can't be removed — the main worktree, a dirty
                // worktree, a lock. Surface the *same* diagnostic `wt remove` prints
                // (drained to stderr on exit) instead of swallowing it, so alt-x
                // isn't a silent dead keypress. Nothing was removed, so the row
                // stays under the (un-reset) cursor.
                if let Some(diagnostic) = e.render_diagnostic() {
                    let mut stashed = self.stashed_warnings.lock().unwrap();
                    if !stashed.contains(&diagnostic) {
                        stashed.push(diagnostic);
                    }
                }
                RemovalEffect::Kept
            }
        }
    }
}

/// The row's shared display slots plus the pre-rendered branch line a morph
/// swaps in (see [`AltXRemover::morph_and_remove_in_background`]).
struct MorphSlots {
    rendered: Arc<Mutex<String>>,
    morphed: Arc<AtomicBool>,
    local_content: LocalContentSlot,
    branch_line: String,
    branch_local: LocalContent,
}

/// Everything the background thread needs to undo a morph when the worktree
/// removal failed (see [`revert_morph`]).
struct MorphRevert {
    rendered: Arc<Mutex<String>>,
    original_rendered: String,
    morphed: Arc<AtomicBool>,
    local_content: LocalContentSlot,
    original_local: LocalContent,
    shortcut_table: ShortcutTable,
    /// The branch token the morph re-keyed the shortcut entry to.
    branch_token: String,
    /// The worktree-path token the entry is keyed under before (and after) morph.
    worktree_token: String,
}

/// Build the `/ branch` row a kept-branch `alt-x` morph swaps in — the rendered
/// line (on the picker's live `layout`, the same grid the worktree rows use) and
/// the diff-content signals for its preview tabs.
///
/// Clones the worktree row's model and demotes it to a local branch: `kind` →
/// `Branch` blanks the path and worktree-status columns and switches the gutter
/// to `/`, while counts / age / message carry over unchanged (the branch keeps
/// the worktree's HEAD). Status symbols are reset and recomputed for the branch
/// kind — `refresh_status_symbols` only fills empty slots, so the worktree's must
/// be cleared first. The [`LocalContent`] is read off the demoted item, so its
/// `working_tree` signal resolves empty (no worktree to diff) and the
/// `working_tree` preview tab dims. OSC 8 hyperlinks are stripped to match the
/// rows the handler builds (skim's pipeline mangles them).
fn build_morph_branch_row(
    layout: &crate::commands::list::layout::LayoutConfig,
    worktree_item: &ListItem,
    default_branch: Option<&str>,
) -> (String, LocalContent) {
    let mut branch_item = worktree_item.clone();
    branch_item.kind = ItemKind::Branch(BranchScope::Local);
    branch_item.status_symbols = Default::default();
    branch_item.refresh_status_symbols(default_branch);
    let line = strip_osc8_hyperlinks(
        &layout
            .render_list_item_line(&branch_item, PLACEHOLDER)
            .render(),
    );
    (line, LocalContent::from_item(&branch_item))
}

/// Undo a morph after the worktree removal failed, restoring the worktree row in
/// place and explaining why it didn't go away.
///
/// The mirror of [`AltXRemover::morph_and_remove_in_background`]'s apply
/// step: restore the row's pre-morph display, clear the
/// [`morphed`](items::LocalCheckout::morphed) flag (so `output()` is the
/// worktree token again), restore the diff-content slot, and move the
/// `alt-y`/`alt-o` shortcut entry back to the worktree token. The row never left
/// its slot, so a plain `Event::Render` repaints it — no reload, no cursor move
/// (unlike [`restore_failed_removal`], which re-inserts a dropped row). The
/// `kept … could not remove it` warning drains to stderr when the picker exits.
fn revert_morph(
    revert: MorphRevert,
    stashed_warnings: &Mutex<Vec<String>>,
    render_tx: &OnceLock<tokio::sync::mpsc::Sender<Event>>,
) {
    let MorphRevert {
        rendered,
        original_rendered,
        morphed,
        local_content,
        original_local,
        shortcut_table,
        branch_token,
        worktree_token,
    } = revert;

    *rendered.lock().unwrap() = original_rendered;
    morphed.store(false, Ordering::Relaxed);
    *local_content.lock().unwrap() = original_local;
    {
        let mut table = shortcut_table.lock().unwrap();
        if let Some(data) = table.remove(&branch_token) {
            table.insert(worktree_token, data);
        }
    }

    stashed_warnings.lock().unwrap().push(
        warning_message(cformat!(
            "Kept <bold>{branch_token}</> worktree — could not remove it"
        ))
        .to_string(),
    );

    if let Some(tx) = render_tx.get() {
        let _ = tx.try_send(Event::Render);
    }
}

/// Number of leading non-selectable header rows the picker streams (the single
/// `HeaderSkimItem`). The skim options pass this to `.header_lines(...)`: skim
/// reserves these from the item pool into its own Header widget, so `item_list`
/// — what the cursor moves over — holds data rows only, indexed from 0.
const PICKER_HEADER_ROWS: usize = 1;

/// Rebuild skim's item pool from the picker's row list and restart the matcher,
/// **synchronously**, so the cursor holds its slot across an `alt-x` removal.
///
/// This is the picker's replacement for skim's `reload`. `reload` clears the pool
/// and restarts the matcher *before* the reader streams the new rows in, so the
/// matcher runs once against the empty pool, `Replace`s `item_list` with nothing,
/// and skim's render clamp (`items.is_empty() → current = 0`) snaps the cursor to
/// the top — the flash. Filling the pool here, before `restart_matcher` runs the
/// matcher (which is async, on the matcher thread pool), means the matcher only
/// ever sees the post-removal list — never empty — so `current` is preserved
/// (clamped to the shrunk list) and the row that slid into the removed slot lands
/// under the cursor. No reposition, no flash. The active query still applies
/// (`restart_matcher` re-filters with the current input), so this is correct under
/// a fuzzy filter too: the cursor's filtered-list index holds.
///
/// `items` carries the leading `HeaderSkimItem`, which `append` re-reserves as
/// the non-selectable header (`header_lines(1)`), matching the initial stream.
fn resync_pool(app: &mut skim::tui::App, items: &Arc<Mutex<Vec<Arc<dyn SkimItem>>>>) {
    let batch: Vec<Arc<dyn SkimItem>> = items.lock().unwrap().iter().map(Arc::clone).collect();
    app.item_pool.clear();
    app.item_pool.append(batch);
    app.restart_matcher(true);
}

/// A skim `Custom` action that runs [`resync_pool`] on the event loop.
///
/// Both alt-x sites queue it. The keybinding callback returns it for the drop path
/// — skim processes a callback's returned events (then a Render) in order, so the
/// queued resync rebuilds the pool before any repaint, equivalent to an inline
/// rebuild. A background removal that fails ([`restore_failed_removal`]) re-inserts
/// the row from off the event loop and has no `App`, so it queues this through
/// skim's event sender to re-show the restored row. Sharing one action keeps the
/// pool-rebuild logic in a single place. The re-inserted row lands at the removed
/// row's old slot — which is exactly where `current` sits after the drop slid the
/// successor up — so the cursor lands back on it for free.
fn resync_pool_action(items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>) -> Action {
    Action::Custom(ActionCallback::new_sync(
        move |app| -> Result<Vec<Event>, Box<dyn std::error::Error + Send + Sync>> {
            resync_pool(app, &items);
            Ok(Vec::new())
        },
    ))
}

/// Consecutive "matcher settled on the resynced pool" observations
/// [`run_preview_when_settled`] waits out before firing the preview. The matcher
/// writes its result, then a later render applies the `Replace` into `item_list`
/// and clamps the cursor — so `item_list` lags the matcher by a render. Each
/// re-arm queues a Render (skim appends one after every action), so three settled
/// observations guarantee the reloaded rows (and the cursor clamp) are in before
/// the preview fires.
const PREVIEW_SETTLED_RENDERS: usize = 3;

/// Hard backstop on [`run_preview_when_settled`] re-arms, far above the handful a
/// normal resync needs. The settled check is the real stop condition; this only
/// guards an unforeseen never-settles state (e.g. a resync that empties the pool).
const PREVIEW_MAX_ATTEMPTS: usize = 1000;

/// A skim `Custom` action that fires [`Event::RunPreview`] once the resynced pool's
/// matcher has settled, refreshing the preview for the row the cursor landed on
/// after an `alt-x` drop.
///
/// skim auto-refreshes the preview across a matcher `Replace` only when the
/// selected row's `text()` changes (`ItemList`'s `on_selection_changed`). That
/// covers a middle-row drop — a successor slides under the cursor — but not the
/// *last* row: `current` is briefly out of range at the `Replace` render, so the
/// selection reads empty and the clamp then lands it on the new last row with no
/// text change to detect, leaving the pane showing the removed row's preview. This
/// fires the missing `RunPreview` (no cursor move — the resync already landed it).
///
/// It re-arms until the matcher has settled on the resynced pool — stopped, the
/// pool non-empty, every item taken — for [`PREVIEW_SETTLED_RENDERS`] consecutive
/// checks; firing earlier would preview the pre-`Replace` (removed) row. A drop
/// that empties the filtered list settles the same way and previews nothing.
fn run_preview_when_settled(
    attempts: Arc<AtomicUsize>,
    settled_streak: Arc<AtomicUsize>,
) -> Action {
    Action::Custom(ActionCallback::new_sync(
        move |app| -> Result<Vec<Event>, Box<dyn std::error::Error + Send + Sync>> {
            let matcher_settled = app.matcher_control.stopped()
                && !app.item_pool.is_empty()
                && app.item_pool.num_not_taken() == 0;
            let streak = if matcher_settled {
                settled_streak.fetch_add(1, Ordering::Relaxed) + 1
            } else {
                settled_streak.store(0, Ordering::Relaxed);
                0
            };
            if streak < PREVIEW_SETTLED_RENDERS
                && attempts.fetch_add(1, Ordering::Relaxed) < PREVIEW_MAX_ATTEMPTS
            {
                return Ok(vec![Event::Action(run_preview_when_settled(
                    Arc::clone(&attempts),
                    Arc::clone(&settled_streak),
                ))]);
            }
            Ok(vec![Event::RunPreview])
        },
    ))
}

/// A removal's user-facing subject for the `kept` warning: a `(label, noun)`
/// pair where `noun` is `worktree` or `branch` and `label` is the branch name
/// (or the worktree's display path for a detached worktree). Computed before the
/// `RemoveResult` moves into the background thread; surfaced by
/// [`restore_failed_removal`].
fn removal_failure_subject(result: &RemoveResult) -> (String, &'static str) {
    match result {
        RemoveResult::RemovedWorktree {
            branch_name: Some(branch),
            ..
        } => (branch.clone(), "worktree"),
        RemoveResult::RemovedWorktree { worktree_path, .. } => {
            (format_path_for_display(worktree_path), "worktree")
        }
        RemoveResult::BranchOnly { branch_name, .. } => (branch_name.clone(), "branch"),
    }
}

/// Whether `do_removal` will actually remove the target — predicted up front from
/// `prepare_removal`'s already-computed [`RemoveResult`], before the row is
/// dropped. The dual of [`removal_target_still_present`]: this decides whether to
/// drop the row, that confirms the drop afterward.
///
/// A `RemovedWorktree` result has passed `ensure_clean` (Phase 5 of
/// `prepare_worktree_removal`), so the worktree removes — the only failures left
/// are async and rare (a clean-check race, a failing approved `pre-remove` hook),
/// which the background restore still catches. A `BranchOnly` result deletes only
/// when `delete_branch_if_safe` would: not `Keep` mode, and either force or an
/// integrated branch (the `integration_reason` here is computed from the *same*
/// `Repository::integration_reason` the later delete consults, so they can't
/// drift). An unmerged branch-only row is thus kept, and predicting it here means
/// it never drops (no flicker) — see [`AltXRemover::keep_unremovable_row`].
fn removal_will_remove_target(result: &RemoveResult) -> bool {
    match result {
        RemoveResult::RemovedWorktree { .. } => true,
        RemoveResult::BranchOnly {
            deletion_mode,
            integration_reason,
            ..
        } => {
            !deletion_mode.should_keep()
                && (deletion_mode.is_force() || integration_reason.is_some())
        }
    }
}

/// Whether the row's target is the worktree the picker was launched from — the
/// `changed_directory` flag `prepare_worktree_removal` sets when the removed
/// worktree is the caller's own.
///
/// The picker declines this case (see [`AltXRemover::keep_current_worktree_row`]):
/// removing the current worktree would have to cd the shell elsewhere first, and
/// that switch drags in `post-switch` hooks streaming into the picker, an empty
/// placeholder directory swapped under the cursor mid-render, and a directory
/// change the picker can't cleanly reflect. Switching away (Enter) and then
/// removing the now-non-current row is the clean path, so alt-x on the current
/// worktree keeps the row and explains. `BranchOnly` rows have no worktree to be
/// standing in, so this is always `false` for them.
fn removal_targets_current_worktree(result: &RemoveResult) -> bool {
    matches!(
        result,
        RemoveResult::RemovedWorktree {
            changed_directory: true,
            ..
        }
    )
}

/// The branch a `RemovedWorktree` removal will **keep** — worktree gone, branch
/// retained — or `None` if the removal will delete the branch (or there's no
/// branch). Drives the `alt-x` in-place morph: a kept branch turns the row into
/// a `/ branch` row rather than dropping it.
///
/// Mirrors [`delete_branch_if_safe`] exactly so the prediction can't drift from
/// the deletion the background `do_removal` performs: force always deletes; a
/// `Keep` flag always retains; otherwise the branch is kept precisely when it is
/// **not** integrated into the same `target_branch.unwrap_or("HEAD")` the actual
/// delete checks (`Repository::integration_reason` → `None`). A `capture_refs`
/// or integration error yields `None` (fall back to the drop path) — never a
/// morph the removal won't back up. Runs a couple of git commands on skim's
/// event loop, like `prepare_removal`'s own validation.
fn worktree_removal_keeps_branch(repo: &Repository, result: &RemoveResult) -> Option<String> {
    let RemoveResult::RemovedWorktree {
        branch_name: Some(branch),
        deletion_mode,
        target_branch,
        ..
    } = result
    else {
        return None;
    };
    if deletion_mode.is_force() {
        return None; // `-D` deletes regardless of integration.
    }
    if deletion_mode.should_keep() {
        return Some(branch.clone()); // `Keep` retains regardless of integration.
    }
    // SafeDelete: kept iff unmerged — the exact check `delete_branch_if_safe` runs.
    let snapshot = repo.capture_refs().ok()?;
    let target = target_branch.as_deref().unwrap_or("HEAD");
    let (_, reason) = repo.integration_reason(&snapshot, branch, target).ok()?;
    reason.is_none().then(|| branch.clone())
}

/// Whether the row's underlying target still exists after `do_removal` ran — the
/// primary evidence for "was this actually removed," used in place of inferring
/// from `do_removal`'s `Result`.
///
/// A `Result` is the wrong signal in two directions: a `RemovedWorktree` removal
/// can return `Err` *after* the worktree is already trashed (rendering or
/// spawning a `post-remove`/`post-switch` hook fails during the announcer flush),
/// and a `BranchOnly` safe-delete that raced from integrated to unmerged returns
/// `Ok` while leaving the branch in place. (The *predictable* unmerged case never
/// reaches here — [`removal_will_remove_target`] keeps that row without dropping
/// it.) Observing the target directly handles both: the worktree dir is gone once
/// removed (renamed into `.git/wt/trash/`), and the branch ref is gone once
/// deleted. The check runs on the background thread, off skim's event loop.
///
/// `worktree_path.exists()` is the right signal here because the picker only ever
/// removes *non-current* worktrees — [`removal_targets_current_worktree`] keeps the
/// current one in place rather than removing it. So no empty placeholder directory
/// is ever left at `worktree_path` (that placeholder, which keeps `$PWD` valid, is
/// created only when removing the worktree the shell is sitting in — see
/// [`crate::output::handlers`]). A successful removal renames the whole tree away;
/// a failed one leaves it intact.
fn removal_target_still_present(repo: &Repository, result: &RemoveResult) -> bool {
    match result {
        RemoveResult::RemovedWorktree { worktree_path, .. } => worktree_path.exists(),
        RemoveResult::BranchOnly { branch_name, .. } => {
            repo.branch(branch_name).exists_locally().unwrap_or(false)
        }
    }
}

/// Stash the canonical "retained; unmerged" info + hint pair (deduped), drained
/// to stderr once the picker releases the terminal. Used by
/// [`AltXRemover::keep_unremovable_row`] — a branch-only row whose unmerged
/// branch `SafeDelete` declines to delete stays put, and this explains the
/// no-op. (A worktree removal that keeps its branch instead transforms the row
/// to `/ branch` live — see [`AltXRemover::morph_and_remove_in_background`] —
/// so it needs no stashed message.) The pair is the one `wt remove` itself
/// prints — see [`crate::output::retained_unmerged_branch_messages`].
fn stash_retained_unmerged_branch(stashed: &Mutex<Vec<String>>, branch_name: &str) {
    let (info, hint) = crate::output::retained_unmerged_branch_messages(branch_name);
    let mut stashed = stashed.lock().unwrap();
    if !stashed.contains(&info) {
        stashed.push(info);
        stashed.push(hint);
    }
}

/// Stash the "can't remove the current worktree here" info + hint pair (deduped),
/// drained to stderr once the picker releases the terminal. Used by
/// [`AltXRemover::keep_current_worktree_row`] — alt-x on the worktree the
/// picker was launched from keeps the row and explains, since removing it would
/// have to switch the shell elsewhere first.
fn stash_current_worktree_hint(stashed: &Mutex<Vec<String>>) {
    let info = info_message("Can't remove the current worktree from the picker").to_string();
    let hint = hint_message("Switch to another worktree first").to_string();
    let mut stashed = stashed.lock().unwrap();
    if !stashed.contains(&info) {
        stashed.push(info);
        stashed.push(hint);
    }
}

/// Put a row back after its background removal didn't happen, closing the alt-x
/// loop so the list never shows a removal that didn't occur.
///
/// `invoke` drops a row optimistically once alt-x's validation passes, then
/// removes the target on a background thread. When the target unexpectedly
/// survives (data safety: a clean-check race against `ensure_clean`, a locked
/// directory, a failing `pre-remove` hook, or a `BranchOnly` delete that raced
/// from integrated to unmerged — see [`removal_target_still_present`]; the
/// predictably-kept unmerged branch is filtered earlier by
/// [`removal_will_remove_target`]), the row must reappear. This re-inserts it into
/// `shared_items` at its original slot, stashes a `kept` warning (drained to
/// stderr once skim releases the terminal; the full error, if any, is in the
/// `tracing::warn!` the caller emits), then queues a [`resync_pool_action`] to
/// re-show it.
///
/// Re-inserting at the removed row's old slot lands the cursor back on the row for
/// free: the drop slid the successor up into that slot under the cursor, so the
/// re-insert pushes the successor back down and the restored row takes the cursor's
/// position. Runs off the event loop (the background removal thread), so it can't
/// touch `App` directly — the queued action does the [`resync_pool`] on the loop.
fn restore_failed_removal(
    items: &Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    render_tx: &Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    stashed_warnings: &Arc<Mutex<Vec<String>>>,
    removed_item: Arc<dyn SkimItem>,
    removed_pos: usize,
    label: &str,
    noun: &str,
) {
    {
        let mut items = items.lock().unwrap();
        let token = removed_item.output().into_owned();
        // A concurrent restore (rapid alt-x on the same row) may have already
        // put it back; don't duplicate it.
        if items.iter().any(|item| item.output().as_ref() == token) {
            return;
        }
        // Another removal may have shrunk the list since the drop; clamp.
        let insert_at = removed_pos.min(items.len());
        items.insert(insert_at, removed_item);
    }

    stashed_warnings.lock().unwrap().push(
        warning_message(cformat!(
            "Kept <bold>{label}</> {noun} — could not remove it"
        ))
        .to_string(),
    );

    let Some(event_tx) = render_tx.get() else {
        return;
    };
    // Re-show the restored row by rebuilding skim's pool from the list it's back in.
    let _ = event_tx.try_send(Event::Action(resync_pool_action(Arc::clone(items))));
}

impl CommandCollector for PickerCollector {
    fn invoke(
        &mut self,
        cmd: &str,
        components_to_stop: Arc<AtomicUsize>,
    ) -> (SkimItemReceiver, Sender<i32>) {
        let _ = components_to_stop;

        // alt-r refresh: `reload(refresh)` re-runs collect and streams a fresh
        // list. The new pipeline's threads feed `rx`; on completion their senders
        // drop and skim's reload sees EOF. The returned handler and join handles
        // are kept alive by those threads, so let them drop here. On a spawn
        // failure we fall through and re-stream the current items unchanged.
        //
        // `alt-x` removal does NOT route here — it runs synchronously through
        // [`AltXRemover`] / [`resync_pool`] instead of a `reload`, so `refresh` is
        // the only command this collector now sees. The re-stream below stays as the
        // fall-through for a failed `refresh` spawn.
        if cmd.trim() == "refresh" {
            match self.factory.spawn(true) {
                Ok(SpawnedPipeline { rx, .. }) => {
                    let (tx_interrupt, _rx_interrupt) = bounded(1);
                    return (rx, tx_interrupt);
                }
                Err(e) => log::warn!("picker: refresh failed: {e:#}"),
            }
        }

        // Stream the current items through a channel for skim to consume. skim
        // 4.x's item channel carries Vec batches, so send the whole list as a
        // single batch; unbounded means the send never blocks.
        let items = self.items.lock().unwrap();
        let (tx, rx) = unbounded();
        let batch: Vec<Arc<dyn SkimItem>> = items.iter().map(Arc::clone).collect();
        let _ = tx.send(batch);
        drop(tx);

        // Dummy interrupt channel — no subprocess to kill.
        // The reader's collect_item thread handles its own components_to_stop
        // accounting; we just need a valid Sender to satisfy the trait signature.
        let (tx_interrupt, _rx_interrupt) = bounded(1);
        (rx, tx_interrupt)
    }
}

/// Whether every `pre-remove` / `post-remove` / `post-switch` command this
/// removal would run is already approved — a read-only check, no prompt.
///
/// `repo` is the worktree the picker is operating from; its `.config/wt.toml`
/// is what every removal hook resolves against, matching `wt remove` /
/// `wt merge`. `main_path` is the post-removal destination (the `post-switch`
/// anchor); `worktree_path` is the worktree being removed (the `pre-remove` /
/// `post-remove` anchor). The picker can't prompt mid-render, so it runs the
/// removal's hooks only when they're already approved (e.g. from a prior
/// `wt remove` / `wt merge`) and skips them otherwise — unapproved project
/// commands must never run. See CLAUDE.md → "Project Commands Run Only After
/// Approval".
fn approved_removal_plan(
    repo: &Repository,
    main_path: &Path,
    worktree_path: &Path,
    approvals: &Approvals,
) -> anyhow::Result<ApprovedHookPlan> {
    // Non-fatal: an unresolvable project identifier just means no project
    // pipeline can be matched against approvals — `approve_readonly` then
    // drops them (fail-closed), rather than aborting the picker removal.
    let project_id = repo.project_identifier().ok();
    let pid = project_id.as_deref();
    let user = repo.user_config();
    let project_config = repo.load_project_config()?;

    let mut builder = HookPlanBuilder::new(project_config.as_ref(), user, pid);
    builder.add(worktree_path, &[HookType::PreRemove, HookType::PostRemove]);
    builder.add(main_path, &[HookType::PostSwitch]);
    Ok(builder.finish().approve_readonly(approvals, pid))
}

/// Everything needed to (re)spawn the picker's collect pipeline. Used once at
/// startup and again on every `alt-r` refresh — which re-runs `collect` so
/// worktrees and branches created outside the session (a teammate's push, a
/// parallel agent) appear without reopening the picker.
///
/// Each [`spawn`](Self::spawn) builds a *fresh* progressive handler (its
/// `OnceLock` slots can't be reset) and item channel, but shares the
/// session-long state — the orchestrator / preview cache (so previews stay
/// warm), `shared_items` and `shortcut_table` (which `on_skeleton` seeds and the
/// `--prs` thread extends), and skim's `render_tx`. Held by [`PickerCollector`]
/// so a refresh can re-enter the pipeline.
struct PipelineFactory {
    repo: Repository,
    render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    /// Monotonic spawn counter. Each [`spawn`](Self::spawn) bumps it and hands
    /// the value to that spawn's `--prs` thread, which appends its PR/MR rows to
    /// `shared_items` only while the counter still matches — so a stale forge call
    /// from a pre-refresh spawn can't pollute the list a later spawn rebuilt. See
    /// [`prs::PrsShared`].
    prs_epoch: Arc<AtomicUsize>,
    shortcut_table: ShortcutTable,
    preview_cache: PreviewCache,
    orchestrator: Arc<PreviewOrchestrator>,
    stashed_warnings: Arc<Mutex<Vec<String>>>,
    /// Handoff of the collect layout to the collector, for rendering a
    /// `/ branch` row on the same grid at `alt-x` time. Filled by the handler's
    /// `provide_layout`, read by [`PickerCollector`]. See [`items::LayoutSlot`].
    layout_slot: items::LayoutSlot,
    preview_dims: (usize, usize),
    skim_list_width: usize,
    command_timeout: Option<std::time::Duration>,
    skip_tasks: std::collections::HashSet<collect::TaskKind>,
    llm_command: Option<String>,
    summary_hint: Option<String>,
    show_branches: bool,
    show_remotes: bool,
    show_prs: bool,
    is_preview_bench: bool,
}

/// The product of one [`PipelineFactory::spawn`]: skim's item receiver plus the
/// handler and thread handles the caller manages (joined in the dry-run path,
/// dropped — detaching the threads — in the interactive and refresh paths).
struct SpawnedPipeline {
    rx: SkimItemReceiver,
    handler: Arc<progressive_handler::PickerHandler>,
    collect_handle: std::thread::JoinHandle<()>,
    prs_handle: Option<std::thread::JoinHandle<()>>,
}

impl PipelineFactory {
    /// Build a fresh handler + item channel, start the `picker-collect` thread
    /// (and the `picker-prs` thread when `--prs` is active), and hand back the
    /// receiver skim reads. The handler holds the only non-thread `tx` clone, so
    /// once the spawned threads finish the channel closes and skim's reader sees
    /// EOF — the "background work done → picker idle" contract, which a refresh
    /// relies on to end its `reload`.
    /// `rebuild_repo` controls the worktree/branch inventory source. A refresh
    /// (`alt-r`) passes `true` to rebuild a fresh `Repository`, re-enumerating
    /// after an in-picker removal. The initial spawn passes `false` to reuse the
    /// startup repo, whose cache the prelude already primed — nothing has mutated
    /// yet, so reusing it is correct and avoids re-paying `git worktree list` /
    /// `local_branches` on the first-paint hot path (doubling them there slows
    /// the picker, worst on Windows).
    ///
    /// The rebuild is also what lets `alt-r` drop a worktree an in-picker `alt-x`
    /// removed: re-enumerating from a fresh handle skips the gone worktree, where
    /// the startup cache would still list it.
    fn spawn(&self, rebuild_repo: bool) -> anyhow::Result<SpawnedPipeline> {
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();

        // Fresh per spawn: the header shows a "loading…" marker keyed to this
        // flag while the forge call is in flight.
        let prs_loading: Option<Arc<AtomicBool>> =
            (self.show_prs && !self.is_preview_bench).then(|| Arc::new(AtomicBool::new(true)));

        // Worktree/branch inventory source for this spawn. The factory's `repo`
        // was primed with `git worktree list` / `local_branches` at picker
        // startup, and those `RepoCache` cells are `OnceCell`s that are never
        // invalidated. A refresh re-probing that shared cache would re-serve the
        // startup list, so after an in-picker removal the removed worktrees would
        // still appear and collect's per-worktree git ops would fail against the
        // gone branches ("fatal: Needed a single revision"). So a refresh
        // (`rebuild_repo`) builds a fresh `Repository::at` — the post-mutation
        // discipline the `RepoCache` docs and `prepare_removal` already require.
        // The initial spawn skips the rebuild: the primed cache is still valid,
        // and rebuilding would re-pay both git calls on the first-paint path.
        // The collect thread (`bg_repo`), the `--prs` thread (`prs_repo`), and
        // the skeleton handler's inventory reads all share this one snapshot.
        let spawn_repo = if rebuild_repo {
            Repository::at(self.repo.discovery_path())?
        } else {
            self.repo.clone()
        };

        // The skeleton→`--prs` handoff (column geometry + the branches already
        // shown for dedup). Fresh per spawn so an alt-r reload's `--prs` thread
        // reads *this* reload's branch set — a session-shared first-write-wins
        // slot would feed it the original skeleton's stale set, double-listing or
        // dropping a PR whose worktree was created/removed since (see
        // `prs::Skeleton`). The grid is width-stable, so per-spawn grids are
        // identical anyway.
        let grid_slot = Arc::new(prs::GridSlot::new());

        let handler: Arc<progressive_handler::PickerHandler> =
            Arc::new(progressive_handler::PickerHandler {
                tx: tx.clone(),
                render_tx: Arc::clone(&self.render_tx),
                last_render_poke: Mutex::new(Instant::now()),
                shared_items: Arc::clone(&self.shared_items),
                shortcut_table: Arc::clone(&self.shortcut_table),
                rendered_slots: OnceLock::new(),
                pr_status_slots: OnceLock::new(),
                comments_fetched: OnceLock::new(),
                local_content_slots: OnceLock::new(),
                preview_cache: Arc::clone(&self.preview_cache),
                repo: spawn_repo.clone(),
                orchestrator: Arc::clone(&self.orchestrator),
                preview_dims: self.preview_dims,
                llm_command: self.llm_command.clone(),
                summary_hint: self.summary_hint.clone(),
                stashed_warnings: Arc::clone(&self.stashed_warnings),
                deferred_items: OnceLock::new(),
                grid_slot: Arc::clone(&grid_slot),
                layout_slot: Arc::clone(&self.layout_slot),
                prs_loading: prs_loading.clone(),
            });

        let bg_handler: Arc<dyn collect::PickerProgressHandler> = handler.clone();
        let bg_repo = spawn_repo.clone();
        let bg_skip_tasks = self.skip_tasks.clone();
        let show_branches = self.show_branches;
        let show_remotes = self.show_remotes;
        let command_timeout = self.command_timeout;
        let skim_list_width = self.skim_list_width;
        let collect_handle = std::thread::Builder::new()
            .name("picker-collect".into())
            .spawn(move || {
                let _ = collect::collect(
                    &bg_repo,
                    collect::ShowConfig::Resolved {
                        show_branches,
                        show_remotes,
                        skip_tasks: bg_skip_tasks,
                        command_timeout,
                        collect_deadline: None,
                        list_width: Some(skim_list_width),
                        progressive_handler: Some(bg_handler),
                    },
                    // Picker renders its own UI through `progressive_handler`;
                    // collect must not write to stdout.
                    RenderTarget::Json,
                );
            })
            .context("Failed to spawn picker-collect thread")?;

        // PR/MR streaming (`--prs`). One forge call on its own thread holding
        // another `tx` clone, so the frame paints from local data immediately and
        // PR rows stream in (~1s) when the call returns.
        let prs_handle = if let Some(prs_loading) = prs_loading {
            let prs_tx = tx.clone();
            let prs_repo = spawn_repo.clone();
            let prs_warnings = Arc::clone(&self.stashed_warnings);
            let prs_orchestrator = Arc::clone(&self.orchestrator);
            let prs_render_tx = Arc::clone(&self.render_tx);
            // Bump the spawn counter and capture this spawn's value: the `--prs`
            // thread appends its rows to `shared_items` only while the counter
            // still matches, so an earlier spawn's still-in-flight forge call
            // can't add rows to this (or a later) spawn's list. See
            // `PipelineFactory::prs_epoch` and `prs::PrsShared`.
            let current_epoch = self.prs_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let prs_shared = prs::PrsShared {
                grid_slot: Arc::clone(&grid_slot),
                shortcut_table: Arc::clone(&self.shortcut_table),
                shared_items: Arc::clone(&self.shared_items),
                epoch: Arc::clone(&self.prs_epoch),
                current_epoch,
            };
            let prs_layout = prs::PrsLayout {
                list_width: self.skim_list_width,
                preview_dims: self.preview_dims,
            };
            Some(
                std::thread::Builder::new()
                    .name("picker-prs".into())
                    .spawn(move || {
                        prs::stream_open_prs(
                            &prs_repo,
                            &prs_layout,
                            &prs_tx,
                            &prs_warnings,
                            &prs_orchestrator,
                            &prs_shared,
                            &prs::PrsStreamSignal {
                                pending: &prs_loading,
                                render_tx: &prs_render_tx,
                            },
                        );
                    })
                    .context("Failed to spawn picker-prs thread")?,
            )
        } else {
            None
        };

        // Drop the local `tx` so the handler's clone (and the threads' clones)
        // are the only senders left — their drop is what signals EOF to skim.
        drop(tx);

        Ok(SpawnedPipeline {
            rx,
            handler,
            collect_handle,
            prs_handle,
        })
    }
}

pub fn handle_picker(
    cli_branches: bool,
    cli_remotes: bool,
    cli_prs: bool,
    change_dir_flag: Option<bool>,
    format: SwitchFormat,
) -> anyhow::Result<()> {
    // Interactive picker requires a terminal for the TUI. The dry-run and
    // preview-bench paths bypass skim entirely, so no TTY is required —
    // useful for tests, for diagnosing the pre-compute pipeline from scripts,
    // and for benchmarking the preview workload headlessly.
    let is_dry_run = std::env::var_os("WORKTRUNK_PICKER_DRY_RUN").is_some();
    let is_preview_bench = std::env::var_os("WORKTRUNK_PREVIEW_BENCH").is_some();
    let skip_tui = is_dry_run || is_preview_bench;
    if !skip_tui && !std::io::stdin().is_terminal() {
        anyhow::bail!("Interactive picker requires an interactive terminal");
    }
    worktrunk::trace::instant("Picker started");

    let (repo, is_recovered) = current_or_recover()?;

    // Merge CLI flags with resolved config (project-specific config is now available)
    let config = repo.config();
    let change_dir = change_dir_flag.unwrap_or_else(|| config.switch.cd());
    let show_branches = cli_branches || config.list.branches();
    let show_remotes = cli_remotes || config.list.remotes();
    // Flag-only: listing PRs always reaches the forge, so it stays opt-in
    // per invocation rather than defaulting on via config.
    let show_prs = cli_prs;
    worktrunk::trace::instant("Picker config resolved");

    // Read the terminal size once, from the canonical reader that
    // `crate::display::terminal_width` also projects (stderr first, then stdout,
    // then `COLUMNS`). The skim list-column width (`skim_list_width` below)
    // derives from the same snapshot, so the two can never observe different
    // widths — whether across a resize or because stdout and stderr point to
    // different terminals.
    //
    // The layout needs both dimensions, so it trusts the snapshot only when a
    // real terminal was detected (width and height both present). A width-only
    // `COLUMNS` reading — or no reading at all — falls back to 80x24, exactly as
    // before. The picker requires a TTY, so that fallback only bites the
    // headless dry-run / preview-bench paths; `skim_list_width` still uses the
    // `COLUMNS` width there.
    let term_dims = crate::display::terminal_dimensions();
    let (term_width, term_height) = match term_dims {
        Some((w, Some(h))) => (w, h),
        _ => (80, 24),
    };

    // Reset the preview tab to working-tree and select the layout from the
    // terminal size.
    let state = PreviewState::new(PreviewLayout::for_dimensions(
        term_width as f64,
        term_height as f64,
    ));
    worktrunk::trace::instant("Picker layout detected");

    // Prime the current worktree's root / git-dir / branch caches with one
    // batched `git rev-parse`. Subsumes the two standalone forks that the
    // speculative preview block below would otherwise make via `branch()`
    // and `root()`, and is also short-circuited when `collect::collect` calls
    // `repo.url_template()` → `load_project_config()` → `project_config_path()`
    // (which runs `prewarm_info` again — now a cache hit).
    let _ = repo.current_worktree().prewarm_info();

    // Preview cache is created up-front so the speculative first-item
    // preview can run in parallel with `collect::collect` below. Tasks
    // route to `COLLECT_POOL` (shared with the row pipeline).
    // Wrapped in `Arc` because the progressive handler (running on the
    // collect background thread) also calls `spawn_preview`.
    //
    // BranchDiff previews resolve the default branch's SHA via
    // `Repository::default_branch_sha`, which sources it from the
    // local-branch inventory cached on the shared `RepoCache`. N parallel
    // preview tasks share one inventory scan instead of each forking
    // `git rev-parse`. Read-only for the picker session — see the
    // accessor's docstring for the staleness contract.
    //
    // skim 4.x repaints on demand, so the orchestrator needs a handle to skim's
    // event loop to surface a preview compute that lands after the keystroke that
    // requested it. The picker fills this `OnceLock` once `Skim::init_tui` has run
    // (inside `run_skim`); until then a fill simply doesn't poke (harmless — skim
    // hasn't rendered a preview to strand yet). The progressive handler and a
    // failed-removal restore share the same sender for their own `Event::Render` /
    // resync pokes. See `preview_notify` and the `progressive_handler` module
    // docstring.
    let render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>> = Arc::new(OnceLock::new());
    let orchestrator = Arc::new(PreviewOrchestrator::new(
        repo.clone(),
        Arc::clone(&render_tx),
    ));
    let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);

    // Speculative warm-up: the picker sorts the current worktree first, and
    // the default tab (WorkingTree = `git diff HEAD` in that worktree) is
    // what skim will render first. Kicking this off before `collect::collect`
    // overlaps preview compute with list collection.
    // The real spawn later skips this key via `contains_key`.
    if let (Ok(Some(branch)), Ok(path)) = (
        repo.current_worktree().branch(),
        repo.current_worktree().root(),
    ) {
        use super::list::model::{ItemKind, ListItem, WorktreeData};
        let mut item = ListItem::new_branch(String::new(), branch);
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path,
            ..Default::default()
        }));
        // num_items doesn't matter for Right (dims independent of it); for
        // Down it only affects height, which doesn't alter pager wrapping.
        let dims = state
            .initial_layout
            .dimensions_for(term_width, term_height, 0);
        orchestrator.spawn_preview(Arc::new(item), PreviewMode::WorkingTree, dims);
    }

    // Run every task — the picker is `wt list --full`. `main…±` (BranchDiff) is
    // now a default `wt list` column, so the picker surfaces it too; it's local
    // git keyed by a persistent content-addressed cache, so warm rows are instant
    // and a cold row computes once in the background (its merge-base walk streams
    // in behind the frame, never blocking the picker). CiStatus is primed from
    // the local cache so the first frame shows cached status (see
    // `populate_from_cache`), then fetched live and streamed in — the same
    // 30–60s-TTL cache plus live fetch as `wt list --full`. The picker's lifetime
    // is bounded by the user, so a slow forge call never blocks anything (see the
    // "Network Access" notes in CLAUDE.md). The `pr` preview tab reads the same
    // live status. `--prs` rows carry their own number from the explicit `--prs`
    // forge call.
    let skip_tasks: std::collections::HashSet<collect::TaskKind> = std::collections::HashSet::new();

    // Per-task command timeout (bounds any single git invocation) from
    // shared `[list]` config. Still applies in progressive mode.
    let command_timeout = config.list.task_timeout();

    // Progressive rendering means the picker never blocks waiting for
    // collect — so there's no UI-freeze budget to bound. The drain runs
    // until its results channel closes or the fallback DRAIN_TIMEOUT
    // (120s) fires.

    // Lay the table out at full terminal width regardless of the preview
    // layout. With the preview shown (Right), skim splits the screen and renders
    // this full-width row into the left pane, clipping the overflow at the
    // boundary; `no_hscroll` plus an empty ellipsis (set on the builder below)
    // make that a clean left-anchored cut. Toggling the preview off with alt-p
    // widens skim's list pane to full width and the SAME rows reveal their
    // right-hand columns — no reload, no re-layout, so no column ever moves.
    // (The Down layout already used full width, so this is a no-op there.)
    //
    // The width comes from the same `term_dims` snapshot as the layout above;
    // its fully-headless fallback is `usize::MAX` (vs. the layout's 80 width) to
    // keep the math total when no width is known at all — the picker requires a
    // TTY, so that only applies to the headless paths. Skim prefixes every line
    // with a 2-column cursor gutter ("> "), so the full width loses 2.
    let list_width_source = term_dims.map(|(w, _)| w).unwrap_or(usize::MAX);
    let skim_list_width = list_width_source.saturating_sub(2);

    // Estimate item count for the preview window spec (only the Down
    // layout depends on it). The Down layout caps visible rows at
    // `max_visible_items(available)`; every row past that cap is a no-op
    // for the height computation, so we short-circuit once the estimate
    // reaches it.
    let num_items_estimate = {
        let cap = preview::max_visible_items(preview::available_height(term_height));
        let mut estimate = repo.list_worktrees().map(|w| w.len()).unwrap_or(cap);
        if estimate < cap && show_branches {
            // Local branches are a superset of worktree branches (each
            // linked worktree normally has one), so take the max rather
            // than summing.
            let local = repo.local_branches().map(|b| b.len()).unwrap_or(cap);
            estimate = estimate.max(local);
        }
        if estimate < cap && show_remotes {
            let remotes = repo.remote_branches().map(|b| b.len()).unwrap_or(0);
            estimate = estimate.saturating_add(remotes);
        }
        estimate
    };
    worktrunk::trace::instant("Picker estimate computed");
    // Compute the dimensions once; the skim preview-window spec is formatted
    // from them rather than recomputed.
    let preview_dims =
        state
            .initial_layout
            .dimensions_for(term_width, term_height, num_items_estimate);
    let preview_window_spec = state.initial_layout.spec_for(preview_dims);

    // Summary hint: when summaries are disabled, prime the Summary cache
    // with config guidance instead of showing a perpetual "Generating…"
    // placeholder.
    let (llm_command, summary_hint) =
        if config.list.summary() && config.commit_generation.is_configured() {
            (config.commit_generation.command.clone(), None)
        } else {
            // Point at the config file wt actually loads from — respecting
            // --config, WORKTRUNK_CONFIG_PATH, and $XDG_CONFIG_HOME — rather
            // than a hardcoded default. Fall back to the canonical path on the
            // rare occasion no location can be determined.
            let config_path = worktrunk::config::config_path()
                .map(|p| format_path_for_display(&p))
                .unwrap_or_else(|| "~/.config/worktrunk/config.toml".to_string());
            let hint = if !config.commit_generation.is_configured() {
                format!(
                    "Configure [commit.generation] command to enable LLM summaries.\n\n\
                     Example in {config_path}:\n\n\
                     [commit.generation]\n\
                     command = \"llm -m haiku\"\n\n\
                     [list]\n\
                     summary = true\n"
                )
            } else {
                format!(
                    "Enable summaries in {config_path}:\n\n\
                     [list]\n\
                     summary = true\n"
                )
            };
            (None, Some(hint))
        };

    // The picker's full row list — header, worktree/branch rows, and (in `--prs`
    // mode) PR/MR rows. `on_skeleton` fills it with the header + worktree/branch
    // rows and the `--prs` thread appends its PR/MR rows; an `alt-x` removal
    // mutates it (`AltXRemover`) and rebuilds skim's pool from it (`resync_pool`).
    // Starts empty — those writers run only after skim is displaying rows.
    let shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>> = Arc::new(Mutex::new(Vec::new()));

    // `alt-y` / `alt-o` lookup table (token → branch + URL). The collect handler
    // fills it with worktree/branch rows and the `--prs` thread extends it; the
    // shortcut keybinding callbacks read it. See `ShortcutTable`.
    let shortcut_table: ShortcutTable = Arc::new(Mutex::new(std::collections::HashMap::new()));

    // Approvals snapshot for the session: alt-x removals consult it read-only
    // to filter the hook plan; see `approved_removal_plan`.
    let approvals = Arc::new(Approvals::load().context("Failed to load approvals")?);

    // Shared between the bg-thread collect handler and a failed alt-x removal
    // (both push warnings while skim owns the terminal) and the main thread
    // (which drains them after `Skim::run_with` returns and stderr is safe
    // again).
    let stashed_warnings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // The collect pipeline, captured so the initial spawn below and every alt-r
    // refresh build it the same way. See `PipelineFactory`.
    let factory = Rc::new(PipelineFactory {
        repo: repo.clone(),
        render_tx: Arc::clone(&render_tx),
        shared_items: Arc::clone(&shared_items),
        prs_epoch: Arc::new(AtomicUsize::new(0)),
        shortcut_table: Arc::clone(&shortcut_table),
        preview_cache: Arc::clone(&preview_cache),
        orchestrator: Arc::clone(&orchestrator),
        stashed_warnings: Arc::clone(&stashed_warnings),
        // Full-layout handoff: the handler fills it in `provide_layout`, the
        // collector reads it to render the `alt-x` `/ branch` row on this grid.
        layout_slot: Arc::new(Mutex::new(None)),
        preview_dims,
        skim_list_width,
        command_timeout,
        skip_tasks,
        llm_command,
        summary_hint,
        show_branches,
        show_remotes,
        show_prs,
        is_preview_bench,
    });

    // skim's pull-based reader side: only `alt-r` (`reload(refresh)`) reaches this
    // now — `alt-x` removal runs synchronously through the `AltXRemover` below.
    let collector = PickerCollector {
        items: Arc::clone(&shared_items),
        factory: Rc::clone(&factory),
    };

    // The `alt-x` removal handler. Holds only `Send` state (every field an `Arc`,
    // or the `Send` `Repository`) so it can move into the keybinding's `Send`
    // callback — it can't carry the collector's `Rc<PipelineFactory>`, so it owns
    // the morph/keep shared slots directly. See `AltXRemover` and
    // `install_remove_keybinding`.
    let alt_x_remover = AltXRemover {
        items: Arc::clone(&shared_items),
        repo: repo.clone(),
        approvals,
        render_tx: Arc::clone(&render_tx),
        stashed_warnings: Arc::clone(&stashed_warnings),
        shortcut_table: Arc::clone(&shortcut_table),
        layout_slot: Arc::clone(&factory.layout_slot),
    };

    // Half-page preview scroll: half of skim's usable height.
    let half_page = (preview::available_height(term_height) / 2).max(5);

    // Configure skim options with Rust-based preview and mode switching keybindings
    let mut options = SkimOptionsBuilder::default()
        .height("90%".to_string())
        .reverse(true)
        // Rank matches by a row's *distinguishing* tail, not the shared
        // `~/workspace/` prefix every worktree path carries. `last_match` makes
        // the matcher prefer the query's rightmost occurrence, and front-loading
        // `PathName` in the tiebreak ranks leaf-segment matches (at/after the
        // last `/`) above parent-directory ones — so `feature/auth` ranks on
        // `auth`, and the worktree folder name ranks on its tail. This is skim's
        // `Path` scheme spelled out as its two underlying knobs: a
        // `.scheme(MatchScheme::Path)` call would also expand here (the builder's
        // `build()` runs `SkimOptions::build`, which expands the scheme — unlike
        // the clap-only `scrollbar` default), but it injects a duplicate `Score`
        // criterion, so setting the knobs directly is the same effect without the
        // artifact. (Default tiebreak is `[Score, Begin, End]`.) Paired with the
        // distinct-path `search_text` built in `progressive_handler::on_skeleton`.
        //
        // `PathName` reads the whole `search_text`, including the trailing gutter
        // glyph. Local-branch rows fold in `/` as that glyph (the gutter sigil),
        // which `PathName` then reads as a path separator, so on a *score tie* a
        // local-branch row sorts just under a worktree/remote row whose glyph
        // (`+`/`@`/`^`/`|`) isn't a separator. The effect is confined to exact
        // ties (`PathName` is the 2nd criterion) and only reorders rows, so it
        // rides along rather than warranting a change to the gutter sigils.
        .last_match(true)
        .tiebreak(vec![
            RankCriteria::Score,
            RankCriteria::PathName,
            RankCriteria::Begin,
            RankCriteria::End,
        ])
        // Fill the whole selected row with the `current` background (set via
        // `current_bg` in `.color(...)` below). skim 4.x applies the current-row
        // style at the line level only when this is on; without it the selection
        // shows just the `>` pointer (the row's own `display()` ANSI spans carry
        // no background). skim 0.20's tuikit backend highlighted the row for free.
        .highlight_line(true)
        // Each row's `display()` owns its layout: a leading gutter sigil
        // (`+`/`@`/`^`/`/`/`|`), then columns, right-truncated to the list width
        // with a trailing `…`. skim's horizontal scroll is a second, conflicting
        // layout authority over the same row — on a query match it scrolls the row
        // left to bring the matched char into view, deriving the offset from that
        // char's *position* in the match text (`search_text` = branch + full path +
        // glyph), which is far longer than the visible row, while clamping against
        // the rendered line's own width. Any row whose rendered width exceeds skim's
        // container (e.g. a long branch name, or a width-count disagreement on wide
        // glyphs) then gets shifted left far enough to clip its leading gutter sigil
        // — typing a few chars made the sigil vanish from every overflowing row.
        // Disabling hscroll leaves worktrunk as the sole row-layout
        // authority: overflow truncates on the right (gutter kept) instead of
        // scrolling left. The picker doesn't reveal matches by scrolling anyway —
        // `display()` ignores the match context and renders its own ANSI.
        .no_hscroll(true)
        // Draw a scrollbar thumb on the item list when it overflows the view.
        // skim's `▐` default is the clap `default_value`, gated on skim's `cli`
        // feature; with `default-features = false` the library `Default` for
        // this `String` field is empty, which skim reads as "no scrollbar".
        // Setting it explicitly restores the thumb — without it a long worktree
        // (or `--prs`) list scrolls with no position cue, made worse by
        // `no_info(true)` below hiding the matched/total counter.
        .scrollbar("▐".to_string())
        // First line (header) non-selectable; `PICKER_HEADER_ROWS` names the count.
        .header_lines(PICKER_HEADER_ROWS)
        .multi(false)
        // The table is laid out at full terminal width (see `skim_list_width`
        // above), so while the preview is shown the rows overflow skim's
        // half-width list pane. Disable horizontal scroll so a fuzzy match deep
        // in the search key can never shift the leading columns out of view — the
        // row always clips left-anchored at the pane boundary. An empty ellipsis
        // makes that a clean cut with no "..": it is the library default under
        // `default-features = false` (the `..` default is gated on skim's `cli`
        // feature, off here), pinned explicitly because the clean clip is
        // load-bearing for the overflow.
        .no_hscroll(true)
        .ellipsis(String::new())
        .no_info(true) // Hide info line (matched/total counter)
        .preview("") // Enable preview (empty string means use SkimItem::preview())
        .preview_window(preview_window_spec.as_str())
        // Color scheme using fzf's --color=light values: dark text (237) on light gray bg (251)
        //
        // Terminal color compatibility is tricky:
        // - current_bg:254 (original): too bright on dark terminals, washes out text
        // - current_bg:236 (fzf dark): too dark on light terminals, jarring contrast
        // - current_bg:251 + current:-1: light bg works on both, but unstyled text
        //   becomes unreadable on dark terminals (light-on-light)
        // - current_bg:251 + current:237: fzf's light theme, best compromise
        //
        // The light theme works universally because:
        // - On dark terminals: light gray highlight stands out clearly
        // - On light terminals: light gray is subtle but visible
        // - Dark text (237) ensures readability regardless of terminal theme
        .color("fg:-1,bg:-1,header:-1,matched:108,current:237,current_bg:251,current_match:108")
        .cmd_collector(Rc::new(RefCell::new(collector)) as Rc<RefCell<dyn CommandCollector>>)
        .bind(vec![
            // Preview-tab switching (alt-1..alt-7 jump to a tab; tab / shift-tab
            // cycle) is installed natively below via `install_preview_tab_keybindings`
            // rather than here — those keys run Rust callbacks, not shell commands.
            // Bare digits 1-7 stay unbound so they flow to the query input (a PR
            // number, or digits within a branch name).
            //
            // Create new worktree with query as branch name (alt-c for "create")
            "alt-c:accept(create)".to_string(),
            // alt-x (remove) is installed natively below via
            // `install_remove_keybinding` — a Custom callback that runs the removal
            // synchronously and rebuilds skim's pool in place (no `reload`, so no
            // cursor flash), which a string bind can't express.
            // Refresh the list (alt-r for "refresh"): `reload(refresh)` re-runs
            // collect through PickerCollector, picking up worktrees/branches
            // created outside the session (a teammate's push, a parallel agent)
            // without reopening the picker.
            "alt-r:reload(refresh)".to_string(),
            // Preview toggle (alt-p shows/hides preview)
            // Note: skim doesn't support change-preview-window like fzf, only toggle
            "alt-p:toggle-preview".to_string(),
            // Suppress skim's default manual horizontal scroll (alt-h / alt-l map to
            // ScrollLeft / ScrollRight in its built-in keymap). `no_hscroll(true)`
            // above only zeros the *automatic* match-following shift; it doesn't gate
            // the manual `manual_hscroll` offset these keys push, so they still slide
            // each row's `display()` left under the fixed gutter — clipping the leading
            // worktree-status sigil (`+`/`@`/`^`/`/`/`|`) and the branch name with no
            // ellipsis. The row table is laid out to fit the pane, so there is nothing
            // to scroll to; ignore both.
            "alt-h:ignore".to_string(),
            "alt-l:ignore".to_string(),
            // Preview scrolling (half-page based on terminal height)
            format!("ctrl-u:preview-up({half_page})"),
            format!("ctrl-d:preview-down({half_page})"),
        ])
        // Legend/controls moved to preview window tabs (render_preview_tabs)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build skim options: {}", e))?;
    // `.build()` parsed the string binds above into `options.keymap`; layer the
    // preview-tab switches on top as native `Action::Custom` callbacks (skim's
    // string bind API can't express a custom action).
    install_preview_tab_keybindings(&mut options.keymap);
    // Row shortcuts (alt-y copy branch, alt-o open PR/MR URL) — native callbacks
    // that read the selected row off skim's `App` and run the OS action on a
    // background thread. Like the preview-tab keys, they can't be string binds.
    install_shortcut_keybindings(&mut options.keymap, Arc::clone(&shortcut_table));
    // alt-x (remove): a Custom callback that runs the removal synchronously and
    // rebuilds skim's pool in place — no `reload`, so the cursor never flashes to
    // the top. Moves the `AltXRemover` in (the callback must be `Send`).
    install_remove_keybinding(&mut options.keymap, alt_x_remover);
    worktrunk::trace::instant("Picker skim options built");

    // Spawn the collect pipeline (and the `--prs` thread when active). The
    // handler holds the only non-thread `tx` clone; when the bg threads exit,
    // `tx` drops, skim's reader sees EOF, and the picker goes idle. The initial
    // spawn reuses the startup-primed inventory (`false`); every alt-r refresh
    // re-runs `factory.spawn(true)` to re-enumerate — see `PipelineFactory`.
    let SpawnedPipeline {
        rx,
        handler,
        collect_handle,
        prs_handle,
    } = factory.spawn(false)?;
    worktrunk::trace::instant("Picker collect spawned");

    // The dry run keeps the handler: skim never runs there, so the EOF contract
    // doesn't apply, and the dump below reads its rendered rows.
    let dry_run_handler = is_dry_run.then_some(handler);

    // Dry-run / preview-bench: skim is bypassed. Wait for collect (which
    // spawns previews via the handler) to finish, then for the orchestrator's
    // pending tasks to drain on `COLLECT_POOL`. Dry-run additionally
    // drains stashed warnings and dumps the cache inventory; preview-bench
    // returns immediately so the measured wall clock is just "spawn → all
    // preview tasks drained", with no JSON serialization or stderr I/O in
    // the hot path.
    if skip_tui {
        drop(rx);
        let _ = collect_handle.join();
        // Join the `--prs` thread (present only for the dry-run, not the bench)
        // so its forge fetch and row render run to completion before we dump
        // and exit — this normal-exit path is what gives the streaming code its
        // coverage. The PR rows it built went nowhere (`rx` is dropped); the
        // dump is the worktree-preview cache, unchanged.
        if let Some(handle) = prs_handle {
            let _ = handle.join();
        }
        orchestrator.wait_for_idle();
        if is_dry_run {
            drain_stashed_warnings(&stashed_warnings);
            // Final rendered rows (ANSI stripped) — lets tests assert on
            // picker row content without a PTY.
            let rows: Vec<String> = dry_run_handler
                .as_ref()
                .and_then(|h| h.rendered_slots.get())
                .map(|slots| {
                    slots
                        .iter()
                        .map(|slot| slot.lock().unwrap().ansi_strip().trim_end().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let dump = serde_json::json!({
                "rows": rows,
                "entries": orchestrator.cache_entries_json(),
            });
            println!("{}", serde_json::to_string_pretty(&dump)?);
        }
        return Ok(());
    }

    // Run skim (single invocation — alt-r reloads and alt-x resyncs in place, not
    // re-launch). Skim receives items as the bg thread's handler sends them, and the
    // handler pushes repaints through `render_tx` (filled inside `run_skim`)
    // as it mutates rows in place.
    //
    // Don't join `collect_handle` after skim exits: drain may still be running
    // network tasks, and joining would block exit for up to DRAIN_TIMEOUT
    // (120s). Process exit terminates the bg thread; its git subprocesses
    // are read-only.
    let output = run_skim(options, rx, &render_tx);
    drop(collect_handle);
    // Same rationale as `collect_handle`: don't join — the forge call may still be
    // in flight, and process exit terminates the thread (its `gh`/`glab`
    // subprocess is read-only).
    drop(prs_handle);

    // Skim has released the terminal — emit any warnings that collect's bg
    // thread stashed during the run. Late warnings (e.g. drain timeouts)
    // may still be in flight; we capture whatever has landed by now and let
    // the rest fall on the floor with the bg thread.
    drain_stashed_warnings(&stashed_warnings);

    // `run_skim` returns Err only on a genuine TUI init / event-loop failure;
    // a user cancel is `Ok` with `is_abort` set. Surface a real failure.
    let out = output?;

    // Handle selection
    if !out.is_abort {
        // Determine action: create (alt-c) or switch (enter)
        // Remove (alt-x) is handled inline in its keybinding callback — it never
        // reaches accept.
        let action = match &out.final_event {
            Event::Action(Action::Accept(Some(label))) if label == "create" => PickerAction::Create,
            _ => PickerAction::Switch,
        };

        let should_create = matches!(action, PickerAction::Create);

        // Get the switch identifier: the query if creating new, otherwise the
        // selected item. `picker_item_identifier` yields a worktree path for
        // any worktree-backed row and the branch name for a branch-only row
        // (same as `wt switch` from CLI) — never the raw `worktree-path:` token.
        let selected = out.selected_items.first();
        let selected_name = selected.map(|item| picker_item_identifier(item.item.as_ref()));
        let query = out.query.trim().to_string();
        let identifier = resolve_identifier(&action, query, selected_name)?;

        // Load config — reuse the recovered repo if we recovered earlier.
        let repo = if is_recovered {
            repo.clone()
        } else {
            Repository::current().context("Failed to switch worktree")?
        };
        // Clone user config out — `SwitchPipeline` takes `&mut UserConfig` (the
        // bare-repo path-fix offer and the shell-integration offer record onto
        // it). Project config is loaded on demand inside the pipeline.
        let mut config = repo.user_config().clone();

        // Run the switch — the same `SwitchPipeline` as `wt switch <branch>`,
        // so hooks, approval, and output cannot drift from the argument path.
        // The picker has no `--execute`, offers no shell integration, and does
        // not capture pre-switch source identity (`capture_source: false` — an
        // existing switch's `{{ base }}` / `{{ base_worktree_path }}` stay
        // unset; result-derived `base` for creates and `target` still flow).
        SwitchPipeline {
            repo: &repo,
            config: &mut config,
            identifier: &identifier,
            create: should_create,
            base: None,
            clobber: false,
            verify: true,
            yes: false,
            change_dir,
            format,
            is_recovered,
            suggestion_ctx: None,
            capture_source: false,
            execute: None,
            execute_args: &[],
            shell_integration_binary: None,
        }
        .run()?;
    }

    Ok(())
}

/// Install the preview-tab switches into skim's keymap: alt-1…alt-7 jump to a
/// tab, tab / shift-tab cycle forward / backward.
///
/// skim's string bind API only maps keys to its built-in actions, so these go
/// in as `Action::Custom` callbacks that set the process-wide
/// [`PreviewStateData`] mode and return `Event::RunPreview` to repaint. They're
/// native rather than `execute-silent` shell commands, so they behave
/// identically everywhere — the previous `echo`/`tr`/`mv` keybind bodies ran
/// through skim's shell, which on Windows is cmd.exe and has neither `tr` nor
/// `mv`. This is also what lets `wt switch` run its picker on Windows at all.
///
/// Keys are resolved with skim's own `parse_key` so they match exactly what its
/// keymap lookup expects (`KeyMap` is keyed by the crossterm `KeyEvent`
/// `parse_key` produces). Shift-Tab is bound under every spelling crossterm
/// might report (`btab` / `shift-btab` / `shift-tab`), mirroring skim's default
/// keymap, so the cycle-back override holds regardless of terminal.
fn install_preview_tab_keybindings(keymap: &mut skim::binds::KeyMap) {
    use skim::binds::parse_key;

    // alt-N jumps to tab N (1-indexed, matching PreviewMode's discriminant).
    let switch_to = |mode: PreviewMode| {
        Action::Custom(ActionCallback::new_sync(move |_app| {
            PreviewStateData::set_mode(mode);
            Ok(vec![Event::RunPreview])
        }))
    };
    for digit in 1..=7u8 {
        if let Ok(key) = parse_key(&format!("alt-{digit}")) {
            keymap.insert(key, vec![switch_to(PreviewMode::from_u8(digit))]);
        }
    }

    let cycle = |forward: bool| {
        Action::Custom(ActionCallback::new_sync(move |_app| {
            PreviewStateData::rotate(forward);
            Ok(vec![Event::RunPreview])
        }))
    };
    if let Ok(key) = parse_key("tab") {
        keymap.insert(key, vec![cycle(true)]);
    }
    for back in ["btab", "shift-btab", "shift-tab"] {
        if let Ok(key) = parse_key(back) {
            keymap.insert(key, vec![cycle(false)]);
        }
    }
}

/// The branch name `alt-y` copies for the row whose `output()` token is `token`:
/// its `RowShortcutData.branch`. `None` when the token isn't in the table or the
/// row has no branch (a detached worktree), so `alt-y` no-ops. Pulled out of the
/// keybinding closure so the lookup — the part that doesn't need a live skim
/// `App` — is unit-testable.
fn resolve_shortcut_branch(table: &ShortcutTable, token: &str) -> Option<String> {
    table
        .lock()
        .unwrap()
        .get(token)
        .and_then(|d| d.branch.clone())
}

/// The PR/MR URL `alt-o` opens for the row whose `output()` token is `token`.
/// `None` when the token isn't in the table or the row has no URL (a worktree
/// whose PR hasn't resolved, or has none), so `alt-o` no-ops. The counterpart to
/// [`resolve_shortcut_branch`].
fn resolve_shortcut_url(table: &ShortcutTable, token: &str) -> Option<String> {
    table
        .lock()
        .unwrap()
        .get(token)
        .and_then(|d| d.url.resolve())
}

/// Install the `alt-y` (copy branch) and `alt-o` (open PR/MR URL) row shortcuts
/// as native callbacks, alongside the preview-tab keys.
///
/// Both read the selected row off skim's `App` — its `output()` token, looked up
/// in `shortcut_table` for the branch / URL — and run the OS action (clipboard,
/// browser) on a background thread, so skim's event loop never blocks and a slow
/// clipboard or opener can't freeze the frame. Neither touches the list, so
/// there's no reload and the cursor stays put. Both no-op when the row lacks the
/// thing they act on: `alt-y` on a detached worktree (no branch), `alt-o` on a
/// row with no URL (a worktree whose PR hasn't resolved, or has none). Failures
/// are logged, not surfaced — skim owns the terminal.
fn install_shortcut_keybindings(keymap: &mut skim::binds::KeyMap, shortcut_table: ShortcutTable) {
    use skim::binds::parse_key;

    // alt-y: copy the selected row's branch name to the system clipboard.
    if let Ok(key) = parse_key("alt-y") {
        let table = Arc::clone(&shortcut_table);
        keymap.insert(
            key,
            vec![Action::Custom(ActionCallback::new_sync(move |app| {
                let branch = app
                    .item_list
                    .selected()
                    .and_then(|m| resolve_shortcut_branch(&table, m.item.output().as_ref()));
                if let Some(branch) = branch {
                    spawn_shortcut("picker-copy", move || os::copy_to_clipboard(&branch));
                }
                Ok(Vec::new())
            }))],
        );
    }

    // alt-o: open the selected row's PR/MR URL in the browser.
    if let Ok(key) = parse_key("alt-o") {
        let table = Arc::clone(&shortcut_table);
        keymap.insert(
            key,
            vec![Action::Custom(ActionCallback::new_sync(move |app| {
                let url = app
                    .item_list
                    .selected()
                    .and_then(|m| resolve_shortcut_url(&table, m.item.output().as_ref()));
                if let Some(url) = url {
                    spawn_shortcut("picker-open", move || os::open_url(&url));
                }
                Ok(Vec::new())
            }))],
        );
    }
}

/// Install `alt-x` (remove the selected row) as a native binding: a single Custom
/// callback that runs the removal synchronously through [`AltXRemover`].
///
/// `alt-x` no longer goes through skim's `reload`. A `reload` clears the item pool
/// and runs the matcher against it once *before* the new rows arrive, which resets
/// the cursor to the top (`current = 0`) for a frame — the flash this fixes. Here
/// the callback mutates the row list ([`AltXRemover::apply`]) and rebuilds skim's
/// pool itself ([`resync_pool`]) on the same event-loop tick, so the matcher only
/// ever sees the post-removal list and the cursor holds its slot. The
/// [`RemovalEffect`] says how to refresh skim's view: a drop resyncs the pool, a
/// morph repaints the row in place and refreshes its (now-dimmed) preview, a kept
/// row needs nothing.
///
/// The callback owns the `remover` (moved in) — skim requires a `Send` callback,
/// which is why [`AltXRemover`] carries only `Send` state and not the collector's
/// `Rc<PipelineFactory>`. A native keymap insert (not a string bind) is required
/// because a string bind can't express a Rust callback (like the preview-tab and
/// row shortcuts).
fn install_remove_keybinding(keymap: &mut skim::binds::KeyMap, remover: AltXRemover) {
    use skim::binds::parse_key;
    let Ok(key) = parse_key("alt-x") else {
        return;
    };
    let cb = Action::Custom(ActionCallback::new_sync(move |app| {
        // The selected row's `output()` token identifies what to remove. No
        // selection (empty list) → nothing to do.
        let Some(selected) = app.item_list.selected() else {
            return Ok(Vec::new());
        };
        let selected_output = selected.item.output().into_owned();
        match remover.apply(selected_output) {
            RemovalEffect::Dropped => {
                // The row left `items`; rebuild skim's pool from the shrunk list so
                // the matcher re-filters it in place — the cursor holds its index and
                // the row that slid up lands under it (no reset, no flash). skim
                // processes a callback's returned events (then a Render) in order, so
                // the queued resync runs before any repaint — same effect as an inline
                // rebuild, and it shares the one `resync_pool_action` the failed-removal
                // restore also queues. Then a settled-gated `RunPreview`: for a
                // *last*-row drop skim's own preview-on-selection-change can't fire
                // (`current` goes briefly out of range), so the pane would otherwise
                // keep showing the removed row; for a middle-row drop skim already
                // refreshes it, so this is a cheap cache-hit repaint.
                Ok(vec![
                    Event::Action(resync_pool_action(Arc::clone(&remover.items))),
                    Event::Action(run_preview_when_settled(
                        Arc::new(AtomicUsize::new(0)),
                        Arc::new(AtomicUsize::new(0)),
                    )),
                ])
            }
            // The row's content changed in place (same item, no `Replace`), so the
            // cursor doesn't move and skim's auto-preview doesn't fire — repaint the
            // list row and request its (now working-tree-dimmed) preview explicitly.
            RemovalEffect::Morphed => Ok(vec![Event::Render, Event::RunPreview]),
            // The row is unchanged (declined / retained, with a stashed hint shown
            // on exit); the cursor never moved, so nothing to repaint.
            RemovalEffect::Kept => Ok(Vec::new()),
        }
    }));
    keymap.insert(key, vec![cb]);
}

/// Run a row shortcut's OS action on a named background thread, logging any
/// failure — the picker owns the terminal, so an error can't be shown inline.
fn spawn_shortcut<F>(name: &str, action: F)
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    let _ = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            if let Err(e) = action() {
                log::warn!("picker: {e:#}");
            }
        });
}

/// Run skim to completion, exposing its event sender for progressive repaints.
///
/// This inlines what `Skim::run_with` does, plus one addition: after the TUI is
/// initialized we publish `Skim::event_sender()` into `render_tx`. skim 4.x
/// renders on demand, so the background collect thread's in-place row mutations
/// stay invisible until something wakes the event loop — the handler pushes
/// `Event::Render` through that sender (see `progressive_handler`), and the
/// preview-tab keybindings return `Event::RunPreview` from their callbacks.
///
/// `wt` runs no outer tokio runtime, so skim's event loop runs on a fresh
/// multi-thread `Runtime` — the same one `run_with` builds in that case. A user
/// cancel is `Ok(SkimOutput)` with `is_abort` set; only a genuine init /
/// event-loop failure is an `Err`.
///
/// Injecting `Event::Render` / `Event::RunPreview` is safe against clobbering
/// the recorded selection: skim's `Accept` / `Abort` set `should_quit` in the
/// same `tick` that records them as `final_event`, and `run()` breaks before
/// the next `tick`, so a trailing injected event is never processed after the
/// terminal action.
fn run_skim(
    options: SkimOptions,
    rx: SkimItemReceiver,
    render_tx: &Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
) -> anyhow::Result<SkimOutput> {
    let mut skim: Skim = Skim::init(options, Some(rx))
        .map_err(|e| anyhow::anyhow!("failed to initialize picker: {e}"))?;
    skim.start();

    // `should_enter` is false only for skim's filter / select-1 / exit-0 / sync
    // modes — none of which the picker enables — so the TUI is always entered.
    // The guard just keeps the fallback safe (an aborted output) rather than
    // panicking on `event_sender()` if that ever changes.
    if skim.should_enter() {
        skim.init_tui()
            .map_err(|e| anyhow::anyhow!("failed to initialize picker TUI: {e}"))?;
        // event_sender() requires init_tui(); publish it before entering the
        // loop so the handler's in-place updates can request repaints.
        let _ = render_tx.set(skim.event_sender());

        let runtime =
            tokio::runtime::Runtime::new().context("failed to start picker event-loop runtime")?;
        let result = runtime.block_on(async {
            skim.enter().await?;
            skim.run().await
        });
        result.map_err(|e| anyhow::anyhow!("interactive picker failed: {e}"))?;
    }

    Ok(skim.output())
}

/// Resolve the branch identifier from picker output.
///
/// Extracted from the picker's accept handler for testability.
fn resolve_identifier(
    action: &PickerAction,
    query: String,
    selected_name: Option<String>,
) -> anyhow::Result<String> {
    match action {
        PickerAction::Create => {
            if query.is_empty() {
                anyhow::bail!("Cannot create worktree: no branch name entered");
            }
            Ok(query)
        }
        PickerAction::Switch => match selected_name {
            Some(name) => Ok(name),
            None => {
                if query.is_empty() {
                    anyhow::bail!("No worktree selected");
                } else {
                    anyhow::bail!(
                        "No worktree matches '{query}' — use alt-c to create a new worktree"
                    );
                }
            }
        },
    }
}

#[cfg(test)]
pub mod tests {
    use super::items::{LocalCheckout, LocalContent, PickerRow, worktree_output_token};
    use super::{
        AltXRemover, PickerAction, PickerRemovalTarget, RemovalEffect, drain_stashed_warnings,
        install_preview_tab_keybindings, install_shortcut_keybindings, picker_item_identifier,
        resolve_identifier, resolve_shortcut_branch, resolve_shortcut_url,
    };
    use crate::commands::list::model::{BranchScope, ItemKind, ListItem, WorktreeData};
    use crate::commands::worktree::RemoveResult;
    use skim::prelude::SkimItem;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use worktrunk::config::Approvals;
    use worktrunk::git::BranchDeletionMode;

    /// Empties the stash and emits each line. Verifies post-skim drain
    /// semantics without standing up a real picker.
    #[test]
    fn drain_stashed_warnings_empties_the_stash() {
        let stash = Mutex::new(vec!["one".to_string(), "two".to_string()]);
        drain_stashed_warnings(&stash);
        assert!(stash.lock().unwrap().is_empty());
    }

    /// A fresh stash with no warnings is a no-op — exercising the empty path
    /// keeps the loop body covered when the picker exits cleanly.
    #[test]
    fn drain_stashed_warnings_handles_empty_stash() {
        let stash: Mutex<Vec<String>> = Mutex::new(Vec::new());
        drain_stashed_warnings(&stash);
        assert!(stash.lock().unwrap().is_empty());
    }

    #[test]
    fn test_install_preview_tab_keybindings() {
        use skim::binds::{KeyMap, parse_key};
        use skim::prelude::Action;

        // The native preview-tab switches replace skim's default bindings for
        // these keys with exactly one custom action each. This asserts the
        // wiring (keyed via skim's own `parse_key` so the lookup matches its
        // event loop). Which tab each callback selects can't be asserted here
        // (`Action::Custom` has no `Eq`), but the callbacks are built by a
        // uniform `from_u8` loop — `from_u8`/`next`/`prev` are unit-tested in
        // `preview`, and the `switch_picker` PTY tests drive the keys end-to-end.
        let mut keymap = KeyMap::default();
        install_preview_tab_keybindings(&mut keymap);

        let mut specs: Vec<String> = (1..=7).map(|d| format!("alt-{d}")).collect();
        specs.extend(["tab", "btab", "shift-btab", "shift-tab"].map(String::from));
        for spec in specs {
            let key = parse_key(&spec).expect("known key spec parses");
            let chain = keymap
                .get(&key)
                .unwrap_or_else(|| panic!("{spec} not bound"));
            assert_eq!(chain.len(), 1, "{spec} should bind a single action");
            assert!(
                matches!(chain[0], Action::Custom(_)),
                "{spec} should bind a native custom action, got {:?}",
                chain[0]
            );
        }
    }

    #[test]
    fn test_install_shortcut_keybindings() {
        use skim::binds::{KeyMap, parse_key};
        use skim::prelude::Action;

        // alt-y (copy branch) and alt-o (open URL) bind native custom actions
        // that read the selected row off skim's `App` and look it up in the
        // shortcut table — no shell binds, so they work cross-platform. The
        // callback behavior (clipboard / browser) is driven by the `switch_picker`
        // PTY tests; here we just assert the wiring, mirroring the tab test above.
        let mut keymap = KeyMap::default();
        let table = Arc::new(Mutex::new(std::collections::HashMap::new()));
        install_shortcut_keybindings(&mut keymap, table);

        for spec in ["alt-y", "alt-o"] {
            let key = parse_key(spec).expect("known key spec parses");
            let chain = keymap
                .get(&key)
                .unwrap_or_else(|| panic!("{spec} not bound"));
            assert_eq!(chain.len(), 1, "{spec} should bind a single action");
            assert!(
                matches!(chain[0], Action::Custom(_)),
                "{spec} should bind a native custom action, got {:?}",
                chain[0]
            );
        }
    }

    /// The lookup the `alt-y` / `alt-o` closures delegate to — pure table logic,
    /// no live skim `App`. Covers a row with a branch + URL, a detached row (no
    /// branch, no URL — both shortcuts no-op), and a token absent from the table.
    #[test]
    fn resolve_shortcut_branch_and_url() {
        use super::items::{RowShortcutData, RowUrl, ShortcutTable};

        let table: ShortcutTable = Arc::new(Mutex::new(std::collections::HashMap::new()));
        {
            let mut t = table.lock().unwrap();
            t.insert(
                "feat".into(),
                RowShortcutData {
                    branch: Some("feat".into()),
                    url: RowUrl::Static(Some("https://example.test/pr/1".into())),
                    morph: None,
                },
            );
            t.insert(
                "wt".into(),
                RowShortcutData {
                    branch: None,
                    url: RowUrl::Static(None),
                    morph: None,
                },
            );
        }

        assert_eq!(
            resolve_shortcut_branch(&table, "feat").as_deref(),
            Some("feat")
        );
        assert_eq!(resolve_shortcut_branch(&table, "wt"), None); // detached: no branch
        assert_eq!(resolve_shortcut_branch(&table, "missing"), None);

        assert_eq!(
            resolve_shortcut_url(&table, "feat").as_deref(),
            Some("https://example.test/pr/1")
        );
        assert_eq!(resolve_shortcut_url(&table, "wt"), None); // no URL
        assert_eq!(resolve_shortcut_url(&table, "missing"), None);
    }

    #[test]
    fn test_resolve_identifier() {
        // Switch returns the selected name
        let result = resolve_identifier(
            &PickerAction::Switch,
            String::new(),
            Some("feature/foo".into()),
        );
        assert_eq!(result.unwrap(), "feature/foo");

        // Switch with no selection and empty query
        let result = resolve_identifier(&PickerAction::Switch, String::new(), None);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No worktree selected")
        );

        // Switch with no selection but a query — the panic from #1565
        let result = resolve_identifier(&PickerAction::Switch, "nonexistent".into(), None);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No worktree matches 'nonexistent'"));
        assert!(err.contains("alt-c"));

        // Create returns the query
        let result = resolve_identifier(&PickerAction::Create, "new-branch".into(), None);
        assert_eq!(result.unwrap(), "new-branch");

        // Create with empty query is an error
        let result = resolve_identifier(&PickerAction::Create, String::new(), None);
        assert!(result.unwrap_err().to_string().contains("no branch name"));
    }

    /// `from_signal` rejects tokens that carry no usable target: a blank or
    /// whitespace-only signal, and a bare `worktree-path:` prefix with no path
    /// after it. A non-empty branch token and a prefixed path both parse.
    #[test]
    fn test_picker_removal_target_from_signal() {
        assert!(PickerRemovalTarget::from_signal("").is_none());
        assert!(PickerRemovalTarget::from_signal("   ").is_none());
        assert!(PickerRemovalTarget::from_signal("worktree-path:").is_none());

        assert!(matches!(
            PickerRemovalTarget::from_signal("feature/foo"),
            Some(PickerRemovalTarget::Branch(branch)) if branch == "feature/foo"
        ));
        assert!(matches!(
            PickerRemovalTarget::from_signal("worktree-path:/tmp/wt"),
            Some(PickerRemovalTarget::WorktreePath(path)) if path == std::path::Path::new("/tmp/wt")
        ));
    }

    /// `picker_item_identifier` yields the worktree path for every
    /// worktree-backed row — branched as well as detached — and the branch name
    /// for a branch-only row, matching what each row's `output()` token carries.
    #[test]
    fn test_picker_item_identifier() {
        let branched = branched_picker_item("feature/foo", Path::new("/tmp/wt-branched"));
        assert_eq!(
            picker_item_identifier(branched.as_ref()),
            "/tmp/wt-branched"
        );

        let detached = detached_picker_item(Path::new("/tmp/wt-detached"));
        assert_eq!(
            picker_item_identifier(detached.as_ref()),
            "/tmp/wt-detached"
        );

        let branch_only = branch_only_picker_item("feature/bar");
        assert_eq!(picker_item_identifier(branch_only.as_ref()), "feature/bar");
    }

    #[test]
    fn test_do_removal_removes_worktree_and_branch() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");

        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();
        assert!(wt_path.exists());

        let result = RemoveResult::RemovedWorktree {
            main_path: test.path().to_path_buf(),
            worktree_path: wt_path.clone(),
            changed_directory: false,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        AltXRemover::do_removal(&repo, &result, &Approvals::default()).unwrap();
        assert!(!wt_path.exists(), "worktree should be removed");

        let output = repo.run_command(&["branch", "--list", "feature"]).unwrap();
        assert!(output.is_empty(), "branch should be deleted");
    }

    #[test]
    fn test_do_removal_branch_only_deletes_integrated_branch() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        // Create a branch at the same commit (fully integrated into main)
        repo.run_command(&["branch", "feature"]).unwrap();

        let result = RemoveResult::BranchOnly {
            branch_name: "feature".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        AltXRemover::do_removal(&repo, &result, &Approvals::default()).unwrap();

        let output = repo.run_command(&["branch", "--list", "feature"]).unwrap();
        assert!(output.is_empty(), "integrated branch should be deleted");
    }

    #[test]
    fn test_do_removal_branch_only_retains_unmerged_branch() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        // Create a branch with an unmerged commit
        repo.run_command(&["checkout", "-b", "unmerged"]).unwrap();
        fs::write(test.path().join("new.txt"), "unmerged work").unwrap();
        repo.run_command(&["add", "."]).unwrap();
        repo.run_command(&["commit", "-m", "unmerged work"])
            .unwrap();
        repo.run_command(&["checkout", "main"]).unwrap();

        let result = RemoveResult::BranchOnly {
            branch_name: "unmerged".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        AltXRemover::do_removal(&repo, &result, &Approvals::default()).unwrap();

        // Branch should be retained — SafeDelete won't delete unmerged branches
        let output = repo.run_command(&["branch", "--list", "unmerged"]).unwrap();
        assert!(
            !output.is_empty(),
            "unmerged branch should be retained with SafeDelete"
        );
    }

    #[test]
    fn test_do_removal_removes_detached_worktree() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("detached");

        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "to-detach",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();

        // Detach HEAD in the new worktree
        worktrunk::shell_exec::Cmd::new("git")
            .args(["checkout", "--detach", "HEAD"])
            .current_dir(&wt_path)
            .run()
            .unwrap();

        assert!(wt_path.exists());

        let result = RemoveResult::RemovedWorktree {
            main_path: test.path().to_path_buf(),
            worktree_path: wt_path.clone(),
            changed_directory: false,
            branch_name: None,
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        AltXRemover::do_removal(&repo, &result, &Approvals::default()).unwrap();
        assert!(!wt_path.exists(), "detached worktree should be removed");
    }

    /// A branch-only row's signal carries the bare branch name, which
    /// `PickerRemovalTarget::from_signal` decodes to `Branch`; `prepare_removal`
    /// then resolves it to the branch-only disposition.
    #[test]
    fn test_prepare_removal_resolves_branch_only_item() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        // A branch at the same commit as main, with no worktree.
        repo.run_command(&["branch", "branch-only-feature"])
            .unwrap();

        let remover = test_remover(Arc::new(Mutex::new(Vec::new())), repo);

        let target = PickerRemovalTarget::from_signal("branch-only-feature").unwrap();
        let (_planning_repo, result) = remover.prepare_removal(&target).unwrap();
        assert!(
            matches!(&result, RemoveResult::BranchOnly { branch_name, .. } if branch_name == "branch-only-feature"),
            "a branch with no worktree should resolve to BranchOnly"
        );
    }

    /// A selection that names neither a worktree nor a local branch fails the
    /// `prepare_worktree_removal` validation, so `prepare_removal` returns the
    /// error rather than touching the picker list.
    #[test]
    fn test_prepare_removal_errors_on_unknown_target() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        let remover = test_remover(Arc::new(Mutex::new(Vec::new())), repo);

        // `RemoveResult` isn't `Debug`; drop the Ok payload so `unwrap_err`
        // (which needs `T: Debug`) can report a failure cleanly.
        let target = PickerRemovalTarget::from_signal("no-such-branch").unwrap();
        let err = remover
            .prepare_removal(&target)
            .map(|_| ())
            .expect_err("unknown removal target should fail validation");
        assert!(
            err.to_string().contains("no-such-branch"),
            "error should name the unresolved target: {err:#}"
        );
    }

    /// A `pre-remove` hook the user hasn't approved must not run when the
    /// picker removes the worktree — the picker can't prompt mid-render, so
    /// unapproved project commands are skipped. The git removal still happens.
    #[test]
    fn test_do_removal_skips_unapproved_pre_remove_hook() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();

        // A `pre-remove` hook in the invoking worktree's `.config/wt.toml` —
        // the config the picker removal resolves against — that would write a
        // marker if it ever ran.
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("pre-remove-ran");
        fs::create_dir_all(test.path().join(".config")).unwrap();
        fs::write(
            test.path().join(".config/wt.toml"),
            format!("pre-remove = {:?}\n", format!("touch {}", marker.display())),
        )
        .unwrap();

        let result = RemoveResult::RemovedWorktree {
            main_path: test.path().to_path_buf(),
            worktree_path: wt_path.clone(),
            changed_directory: false,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        // Empty approvals → `approve_readonly` drops the unapproved project
        // `pre-remove` pipeline from the plan, so it never runs.
        let approvals = Approvals::default();
        AltXRemover::do_removal(&repo, &result, &approvals).unwrap();
        assert!(!wt_path.exists(), "worktree should be removed");
        assert!(!marker.exists(), "unapproved pre-remove hook must not run");
    }

    /// Build a `PickerRow` from a snapshot `ListItem`.
    fn picker_item(branch_name: &str, item: ListItem) -> Arc<dyn SkimItem> {
        let pr_status = Arc::new(Mutex::new(item.pr_status.clone()));
        let output_token = worktree_output_token(&item, branch_name);
        Arc::new(PickerRow {
            search_base: branch_name.to_string(),
            gutter: '@',
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: branch_name.to_string(),
            output_token,
            preview_cache: Arc::new(dashmap::DashMap::new()),
            pr_status,
            notifier: super::preview_notify::PreviewNotifier::detached(),
            local: Some(LocalCheckout {
                has_upstream: false,
                summaries_enabled: false,
                local_content: Arc::new(Mutex::new(LocalContent::default())),
                morphed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        }) as Arc<dyn SkimItem>
    }

    /// Build a `PickerRow` standing in for a detached-worktree row.
    fn detached_picker_item(path: &Path) -> Arc<dyn SkimItem> {
        let mut item = ListItem::new_branch("abc123".to_string(), "(detached)".to_string());
        item.branch = None;
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: path.to_path_buf(),
            detached: true,
            ..Default::default()
        }));
        picker_item("(detached)", item)
    }

    /// Build a `PickerRow` standing in for a branched-worktree row.
    fn branched_picker_item(branch: &str, path: &Path) -> Arc<dyn SkimItem> {
        let mut item = ListItem::new_branch("abc123".to_string(), branch.to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: path.to_path_buf(),
            ..Default::default()
        }));
        picker_item(branch, item)
    }

    /// Build a `PickerRow` standing in for a branch-only row (no worktree).
    fn branch_only_picker_item(branch: &str) -> Arc<dyn SkimItem> {
        picker_item(
            branch,
            ListItem::new_branch("abc123".to_string(), branch.to_string()),
        )
    }

    /// Build a morphable worktree row and register everything the morph path needs
    /// — a [`MorphHandle`](items::MorphHandle) in the remover's shortcut table,
    /// keyed by the row's `output()` token, and a real layout in its slot — so a
    /// kept-branch removal morphs in place instead of falling back to a drop.
    /// Returns the row, its token, and the shared `rendered` / `morphed` slots the
    /// morph mutates (so a test can assert on them).
    fn setup_morphable_row(
        remover: &AltXRemover,
        branch: &str,
        path: &Path,
    ) -> (
        Arc<dyn SkimItem>,
        String,
        Arc<Mutex<String>>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut item = ListItem::new_branch("abc123".to_string(), branch.to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: path.to_path_buf(),
            ..Default::default()
        }));
        let item_arc = Arc::new(item);
        let rendered = Arc::new(Mutex::new(format!("+ {branch}")));
        let local_content = Arc::new(Mutex::new(LocalContent::default()));
        let morphed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let row: Arc<dyn SkimItem> = Arc::new(PickerRow {
            search_base: branch.to_string(),
            gutter: '+',
            rendered: Arc::clone(&rendered),
            branch_name: branch.to_string(),
            output_token: worktree_output_token(&item_arc, branch),
            preview_cache: Arc::new(dashmap::DashMap::new()),
            pr_status: Arc::new(Mutex::new(None)),
            notifier: super::preview_notify::PreviewNotifier::detached(),
            local: Some(LocalCheckout {
                has_upstream: false,
                summaries_enabled: false,
                local_content: Arc::clone(&local_content),
                morphed: Arc::clone(&morphed),
            }),
        });
        let token = row.output().to_string();

        remover.shortcut_table.lock().unwrap().insert(
            token.clone(),
            super::items::RowShortcutData {
                branch: Some(branch.to_string()),
                url: super::items::RowUrl::Static(None),
                morph: Some(super::items::MorphHandle {
                    item: Arc::clone(&item_arc),
                    rendered: Arc::clone(&rendered),
                    local_content,
                    morphed: Arc::clone(&morphed),
                }),
            },
        );
        *remover.layout_slot.lock().unwrap() =
            Some(crate::commands::list::layout::calculate_layout_with_width(
                std::slice::from_ref(&*item_arc),
                &std::collections::HashSet::new(),
                80,
                Path::new("/test"),
                None,
                None,
                crate::commands::list::layout::ColumnSelection {
                    custom: &[],
                    selected: None,
                },
            ));
        (row, token, rendered, morphed)
    }

    /// A real [`PipelineFactory`] with empty config for the removal / `invoke`
    /// tests. Its `spawn()` is only reached by the refresh verb, which these
    /// tests don't exercise, so the minimal field set is enough to satisfy the
    /// type without standing up a full picker.
    fn test_factory(repo: worktrunk::git::Repository) -> std::rc::Rc<super::PipelineFactory> {
        let render_tx = Arc::new(OnceLock::new());
        let orchestrator = Arc::new(super::preview_orchestrator::PreviewOrchestrator::new(
            repo.clone(),
            Arc::clone(&render_tx),
        ));
        let preview_cache = Arc::clone(&orchestrator.cache);
        std::rc::Rc::new(super::PipelineFactory {
            repo,
            render_tx,
            shared_items: Arc::new(Mutex::new(Vec::new())),
            prs_epoch: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            shortcut_table: Arc::new(Mutex::new(std::collections::HashMap::new())),
            preview_cache,
            orchestrator,
            stashed_warnings: Arc::new(Mutex::new(Vec::new())),
            layout_slot: Arc::new(Mutex::new(None)),
            preview_dims: (80, 24),
            skim_list_width: 80,
            command_timeout: None,
            skip_tasks: std::collections::HashSet::new(),
            llm_command: None,
            summary_hint: None,
            show_branches: false,
            show_remotes: false,
            show_prs: false,
            is_preview_bench: false,
        })
    }

    /// An [`AltXRemover`] for the removal tests, wrapping the given `items` and
    /// `repo`. Its shortcut-table / layout / `stashed_warnings` come from a fresh
    /// `test_factory` so [`setup_morphable_row`] can register a morph handle and a
    /// test can assert on stashed warnings.
    fn test_remover(
        items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
        repo: worktrunk::git::Repository,
    ) -> AltXRemover {
        let factory = test_factory(repo.clone());
        AltXRemover {
            items,
            repo,
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
            stashed_warnings: Arc::clone(&factory.stashed_warnings),
            shortcut_table: Arc::clone(&factory.shortcut_table),
            layout_slot: Arc::clone(&factory.layout_slot),
        }
    }

    /// Two detached worktrees both render the branch label `(detached)`, but
    /// each row's `output()` token carries its unique path. alt-x on the
    /// second row must remove exactly that worktree — not the first detached
    /// one a branch-name match would resolve to — and drop only its row.
    #[test]
    fn test_apply_removes_selected_detached_worktree_by_path_token() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let first_path = wt_dir.path().join("detached-one");
        let second_path = wt_dir.path().join("detached-two");

        for (branch, path) in [
            ("to-detach-one", first_path.as_path()),
            ("to-detach-two", second_path.as_path()),
        ] {
            repo.run_command(&["worktree", "add", "-b", branch, path.to_str().unwrap()])
                .unwrap();
            worktrunk::shell_exec::Cmd::new("git")
                .args(["checkout", "--detach", "HEAD"])
                .current_dir(path)
                .run()
                .unwrap();
        }

        let reported_paths: Vec<_> = repo
            .list_worktrees()
            .unwrap()
            .iter()
            .filter(|wt| wt.branch.is_none())
            .map(|wt| wt.path.clone())
            .collect();
        let first_reported = reported_paths
            .iter()
            .find(|path| path.file_name().is_some_and(|name| name == "detached-one"))
            .unwrap();
        let second_reported = reported_paths
            .iter()
            .find(|path| path.file_name().is_some_and(|name| name == "detached-two"))
            .unwrap();

        let first_item = detached_picker_item(first_reported);
        let second_item = detached_picker_item(second_reported);
        let first_output = first_item.output().to_string();
        let second_output = second_item.output().to_string();
        assert_ne!(first_output, second_output);
        assert_eq!(
            picker_item_identifier(second_item.as_ref()),
            second_reported.to_string_lossy()
        );

        let items = Arc::new(Mutex::new(vec![
            Arc::clone(&first_item),
            Arc::clone(&second_item),
        ]));
        let remover = test_remover(Arc::clone(&items), repo.clone());

        // alt-x's callback hands `apply` the selected row's `output()` token.
        remover.apply(second_output.clone());

        let remaining: Vec<_> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().to_string())
            .collect();
        assert_eq!(remaining, vec![first_output]);

        let deadline = Instant::now() + Duration::from_secs(5);
        while second_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(first_path.exists(), "first detached worktree should remain");
        assert!(
            !second_path.exists(),
            "selected detached worktree should be removed"
        );
    }

    /// A refresh (`alt-r`) re-runs `factory.spawn()`. The factory carries the
    /// `Repository` whose worktree-list cache was primed at picker startup and
    /// is never invalidated, so after a worktree disappears `spawn` must rebuild
    /// a fresh `Repository` rather than re-probe that stale cache — otherwise the
    /// refresh streams a row for the gone worktree and collect's per-worktree git
    /// ops fail against its deleted branch ("fatal: Needed a single revision").
    #[test]
    fn test_spawn_reenumerates_worktrees_after_removal() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        // The repo the factory carries; its cache is primed below, as the
        // picker prelude primes it at startup.
        let factory_repo = worktrunk::git::Repository::at(test.path()).unwrap();

        let wt_dir = tempfile::tempdir().unwrap();
        let doomed_path = wt_dir.path().join("doomed");
        factory_repo
            .run_command(&[
                "worktree",
                "add",
                "-b",
                "doomed",
                doomed_path.to_str().unwrap(),
            ])
            .unwrap();
        let primed_has_doomed = factory_repo
            .list_worktrees()
            .unwrap()
            .iter()
            .any(|wt| wt.branch.as_deref() == Some("doomed"));
        assert!(primed_has_doomed, "cache primed while doomed still present");

        // Remove the worktree through a separate fresh `Repository`, exactly as
        // the picker's background removal does. `factory_repo`'s cache is now
        // stale: it still lists `doomed`.
        let removal_repo = worktrunk::git::Repository::at(test.path()).unwrap();
        removal_repo
            .run_command(&[
                "worktree",
                "remove",
                "--force",
                doomed_path.to_str().unwrap(),
            ])
            .unwrap();
        removal_repo
            .run_command(&["branch", "-D", "doomed"])
            .unwrap();

        let factory = test_factory(factory_repo);
        // `true` models the refresh path (`alt-r`), which rebuilds a fresh repo;
        // the initial spawn (`false`) deliberately reuses the startup inventory.
        let super::SpawnedPipeline {
            rx,
            handler,
            collect_handle,
            ..
        } = factory.spawn(true).unwrap();
        // Drop the returned handler's sender, then wait for the collect thread
        // to finish (dropping its handler clone); the lone senders gone, `rx`
        // hits EOF. The unbounded channel buffered every streamed row.
        drop(handler);
        collect_handle.join().unwrap();
        let outputs: Vec<String> = std::iter::from_fn(|| rx.recv().ok())
            .flatten()
            .map(|item| item.output().into_owned())
            .collect();

        assert!(
            !outputs.is_empty(),
            "the surviving worktree should still stream a row"
        );
        assert!(
            !outputs.iter().any(|out| out.contains("doomed")),
            "refresh must not stream the removed worktree: {outputs:?}"
        );
    }

    /// alt-x with nothing selectable under the cursor hands `apply` an empty token;
    /// `apply` must treat it as a no-op and leave the list intact.
    #[test]
    fn test_apply_empty_token_is_noop() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let item = branch_only_picker_item("some-branch");
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let remover = test_remover(Arc::clone(&items), repo);
        remover.apply(String::new());
        assert_eq!(
            items.lock().unwrap().len(),
            1,
            "empty selection must not remove anything"
        );
    }

    /// alt-x on a target that fails validation (a branch with no worktree and no
    /// local ref) takes `apply`'s error arm: it logs and leaves the list intact —
    /// no drop, no background work.
    #[test]
    fn test_apply_leaves_list_intact_when_prepare_fails() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let item = branch_only_picker_item("real-row");
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let remover = test_remover(Arc::clone(&items), repo);

        // `no-such-branch` parses as a branch target but has no worktree and no
        // local ref, so `prepare_removal` errors before anything is dropped.
        remover.apply("no-such-branch".to_string());

        let outputs: Vec<String> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().into_owned())
            .collect();
        assert_eq!(
            outputs,
            vec![token],
            "a target that fails validation leaves the row untouched"
        );
    }

    /// `restore_failed_removal` puts a dropped row back at its original slot and
    /// stashes a `worktree kept` warning — the correction path that keeps the
    /// alt-x list from showing a removal that didn't happen.
    #[test]
    fn test_restore_failed_removal_reinserts_row_and_stashes_warning() {
        // The list as it stands after `dropped-b` (originally shared_items
        // index 2) was optimistically dropped: a header at 0, two surviving
        // data rows.
        let items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>> = Arc::new(Mutex::new(vec![
            branch_only_picker_item("header"),
            branch_only_picker_item("keep-a"),
            branch_only_picker_item("keep-c"),
        ]));
        // A live sender so the restore queues its resync action rather
        // than the early return.
        let render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<skim::prelude::Event>>> =
            Arc::new(OnceLock::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        render_tx.set(tx).unwrap();
        let stashed = Arc::new(Mutex::new(Vec::new()));

        super::restore_failed_removal(
            &items,
            &render_tx,
            &stashed,
            branch_only_picker_item("dropped-b"),
            2,
            "dropped-b",
            "worktree",
        );

        let outputs: Vec<String> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().into_owned())
            .collect();
        assert_eq!(
            outputs,
            vec!["header", "keep-a", "dropped-b", "keep-c"],
            "row restored at its original slot"
        );
        let warnings = stashed.lock().unwrap();
        assert_eq!(warnings.len(), 1, "one warning stashed");
        assert!(
            warnings[0].contains("dropped-b") && warnings[0].contains("Kept"),
            "warning names the kept worktree: {}",
            warnings[0]
        );
        // The restore re-shows the row by queuing a pool-resync Custom action.
        assert!(
            matches!(rx.try_recv(), Ok(skim::prelude::Event::Action(_))),
            "restore queues a resync action when the sender is live"
        );
    }

    /// Restoring a row that's already back is a no-op — no duplicate, no extra
    /// warning. Guards rapid repeated alt-x racing on the same row.
    #[test]
    fn test_restore_failed_removal_skips_when_already_present() {
        let row = branch_only_picker_item("present");
        let items = Arc::new(Mutex::new(vec![Arc::clone(&row)]));
        let render_tx = Arc::new(OnceLock::new());
        let stashed = Arc::new(Mutex::new(Vec::new()));

        super::restore_failed_removal(&items, &render_tx, &stashed, row, 0, "present", "worktree");

        assert_eq!(items.lock().unwrap().len(), 1, "no duplicate inserted");
        assert!(
            stashed.lock().unwrap().is_empty(),
            "no warning when there's nothing to restore"
        );
    }

    /// `removal_failure_subject` prefers the branch name (falling back to the
    /// worktree path for a detached worktree) and pairs it with the right noun:
    /// `worktree` for a worktree removal, `branch` for a branch-only deletion.
    #[test]
    fn test_removal_failure_subject() {
        let branched = RemoveResult::RemovedWorktree {
            main_path: std::path::PathBuf::from("/tmp/main"),
            worktree_path: std::path::PathBuf::from("/tmp/wt-feature"),
            changed_directory: false,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };
        assert_eq!(
            super::removal_failure_subject(&branched),
            ("feature".to_string(), "worktree")
        );

        let detached = RemoveResult::RemovedWorktree {
            main_path: std::path::PathBuf::from("/tmp/main"),
            worktree_path: std::path::PathBuf::from("/tmp/wt-detached"),
            changed_directory: false,
            branch_name: None,
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };
        assert_eq!(
            super::removal_failure_subject(&detached),
            ("/tmp/wt-detached".to_string(), "worktree")
        );

        let branch_only = RemoveResult::BranchOnly {
            branch_name: "orphan".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        assert_eq!(
            super::removal_failure_subject(&branch_only),
            ("orphan".to_string(), "branch")
        );
    }

    /// End-to-end through `apply`: `prepare_removal` passes (the worktree is
    /// clean and removable), but the background `do_removal` fails on an
    /// approved-yet-failing `pre-remove` hook. The row is dropped optimistically,
    /// then restored when the removal fails — the worktree is preserved and the
    /// list reflects that, instead of leaving a phantom-removed row.
    #[test]
    fn test_apply_restores_row_when_removal_fails() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();

        // A `pre-remove` hook that always fails, in the project config the
        // picker removal resolves against.
        fs::create_dir_all(test.path().join(".config")).unwrap();
        fs::write(
            test.path().join(".config/wt.toml"),
            "pre-remove = \"false\"\n",
        )
        .unwrap();

        // Approve `false` so the hook is selected into the read-only plan and
        // actually runs; an isolated approvals path keeps real config untouched.
        let pid = repo.project_identifier().unwrap();
        let approvals_dir = tempfile::tempdir().unwrap();
        let approvals_path = approvals_dir.path().join("approvals.toml");
        let mut approvals = Approvals::default();
        approvals
            .approve_command(pid, "false".to_string(), &approvals_path)
            .unwrap();

        // Build the row from the git-reported worktree path, not the raw temp
        // path: on macOS `git worktree list` resolves the `/var`→`/private/var`
        // symlink, and `prepare_removal`'s path lookup matches that resolved
        // form.
        let reported_path = repo
            .list_worktrees()
            .unwrap()
            .iter()
            .find(|wt| wt.branch.as_deref() == Some("feature"))
            .map(|wt| wt.path.clone())
            .expect("feature worktree is listed");
        let item = branched_picker_item("feature", &reported_path);
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let stashed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let remover = AltXRemover {
            items: Arc::clone(&items),
            repo: repo.clone(),
            approvals: Arc::new(approvals),
            render_tx: Arc::new(OnceLock::new()),
            stashed_warnings: Arc::clone(&stashed),
            shortcut_table: Arc::new(Mutex::new(std::collections::HashMap::new())),
            layout_slot: Arc::new(Mutex::new(None)),
        };

        remover.apply(token.clone());

        // The background removal fails on the approved-yet-failing hook, so
        // `restore_failed_removal` runs: only that path stashes a warning, so
        // poll on it as the synchronization point.
        let deadline = Instant::now() + Duration::from_secs(5);
        while stashed.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let warnings = stashed.lock().unwrap().clone();
        assert!(
            warnings.iter().any(|w| w.contains("feature")),
            "a failed removal stashes a `kept` warning: {warnings:?}"
        );

        let outputs: Vec<String> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().into_owned())
            .collect();
        assert_eq!(
            outputs,
            vec![token],
            "the row is restored after the removal fails"
        );
        assert!(
            reported_path.exists(),
            "the worktree is preserved when removal fails"
        );
    }

    /// End-to-end through `apply`: alt-x on a worktree whose branch is unmerged
    /// morphs the row to `/ branch` in place. The worktree is removed but the
    /// branch is kept (`SafeDelete` won't delete unmerged work), and the row
    /// never leaves its slot — its `morphed` flag flips, its `output()` becomes
    /// the bare branch token, and its display line is rewritten (no longer the
    /// `+ worktree` line). The morph is applied synchronously in `apply`; only
    /// the git removal runs on the background thread.
    #[test]
    fn test_apply_morphs_unmerged_worktree_to_branch_row() {
        use std::sync::atomic::Ordering;

        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();

        // Make `feature` unmerged: a commit on it that main doesn't have, so
        // SafeDelete retains the branch when the worktree is removed.
        fs::write(wt_path.join("new.txt"), "unmerged work").unwrap();
        worktrunk::shell_exec::Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .run()
            .unwrap();
        worktrunk::shell_exec::Cmd::new("git")
            .args(["commit", "-m", "unmerged work"])
            .current_dir(&wt_path)
            .run()
            .unwrap();

        // Build the row from the git-reported path (macOS resolves the
        // `/var`→`/private/var` symlink, which `prepare_removal`'s lookup matches).
        let reported_path = repo
            .list_worktrees()
            .unwrap()
            .iter()
            .find(|wt| wt.branch.as_deref() == Some("feature"))
            .map(|wt| wt.path.clone())
            .expect("feature worktree is listed");
        let items = Arc::new(Mutex::new(Vec::new()));
        let remover = test_remover(Arc::clone(&items), repo.clone());
        let (row, token, rendered, morphed) =
            setup_morphable_row(&remover, "feature", &reported_path);
        items.lock().unwrap().push(Arc::clone(&row));
        let original_line = rendered.lock().unwrap().clone();

        remover.apply(token.clone());

        // The morph is synchronous, so it's already applied when `apply` returns:
        // the row is now a branch row in place — flag flipped, token rebranded,
        // line rewritten.
        assert!(
            morphed.load(Ordering::Relaxed),
            "the kept-branch worktree row is morphed to a branch row"
        );
        assert_eq!(
            row.output().as_ref(),
            "feature",
            "the morphed row's selection token is the bare branch name"
        );
        assert_ne!(
            *rendered.lock().unwrap(),
            original_line,
            "the morphed row's display line is rewritten to the `/ branch` line"
        );

        // The worktree removal itself runs in the background.
        let deadline = Instant::now() + Duration::from_secs(5);
        while reported_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!reported_path.exists(), "the worktree is removed");
        let branch_list = repo.run_command(&["branch", "--list", "feature"]).unwrap();
        assert!(
            !branch_list.is_empty(),
            "the unmerged branch is retained after its worktree is removed"
        );
        // The removal succeeded, so the morph stands (no revert).
        assert!(
            morphed.load(Ordering::Relaxed),
            "a successful removal leaves the row morphed"
        );
    }

    /// A kept-branch worktree removal whose row carries no `MorphHandle` (or whose
    /// layout hasn't landed) can't morph in place, so `morph_and_remove_in_background`
    /// falls back to the drop path: `apply` reports `Dropped` and the row leaves the
    /// list (the worktree still removes, the branch is still kept). Same setup as
    /// `test_apply_morphs_…` but without `setup_morphable_row`, so the shortcut table
    /// has no morph handle for the row.
    #[test]
    fn test_apply_drops_unmorphable_kept_branch_row() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();
        // Make `feature` unmerged so SafeDelete keeps the branch (the morph premise).
        fs::write(wt_path.join("new.txt"), "unmerged work").unwrap();
        worktrunk::shell_exec::Cmd::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .run()
            .unwrap();
        worktrunk::shell_exec::Cmd::new("git")
            .args(["commit", "-m", "unmerged work"])
            .current_dir(&wt_path)
            .run()
            .unwrap();

        let reported_path = repo
            .list_worktrees()
            .unwrap()
            .iter()
            .find(|wt| wt.branch.as_deref() == Some("feature"))
            .map(|wt| wt.path.clone())
            .expect("feature worktree is listed");
        let item = branched_picker_item("feature", &reported_path);
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        // `test_remover` registers no morph handle, so the kept-branch removal falls
        // back to a drop.
        let remover = test_remover(Arc::clone(&items), repo.clone());

        assert!(
            matches!(remover.apply(token), RemovalEffect::Dropped),
            "an unmorphable kept-branch removal falls back to the drop path"
        );
        assert!(
            items.lock().unwrap().is_empty(),
            "the row drops when it can't morph"
        );

        // The worktree is removed in the background; the unmerged branch is kept.
        let deadline = Instant::now() + Duration::from_secs(5);
        while reported_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!reported_path.exists(), "the worktree is removed");
        assert!(
            !repo
                .run_command(&["branch", "--list", "feature"])
                .unwrap()
                .is_empty(),
            "the unmerged branch is retained"
        );
    }

    /// The negative of the above, end-to-end through `apply`: alt-x on a worktree
    /// whose branch is *integrated* deletes both the worktree and the branch, so
    /// there's no branch to keep — the row drops (it's removed from the list)
    /// rather than morphing. `worktree_removal_keeps_branch` returns `None`, so the
    /// drop path runs, not the morph. Guards against morphing (and resurrecting) a
    /// row whose branch is actually gone.
    #[test]
    fn test_apply_drops_integrated_worktree_row() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        // No extra commit → `feature` sits at main's commit (integrated), so
        // SafeDelete deletes the branch along with the worktree.
        repo.run_command(&[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_path.to_str().unwrap(),
        ])
        .unwrap();

        let reported_path = repo
            .list_worktrees()
            .unwrap()
            .iter()
            .find(|wt| wt.branch.as_deref() == Some("feature"))
            .map(|wt| wt.path.clone())
            .expect("feature worktree is listed");
        let item = branched_picker_item("feature", &reported_path);
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let remover = test_remover(Arc::clone(&items), repo.clone());

        remover.apply(token);

        // The drop is synchronous (the row is removed before the background git
        // work), so the list is already empty when `apply` returns.
        assert!(
            items.lock().unwrap().is_empty(),
            "the integrated worktree row drops instead of morphing"
        );

        // The background removal deletes both the worktree and the branch.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !repo
            .run_command(&["branch", "--list", "feature"])
            .unwrap()
            .is_empty()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!reported_path.exists(), "the worktree is removed");
        assert!(
            repo.run_command(&["branch", "--list", "feature"])
                .unwrap()
                .is_empty(),
            "the integrated branch is deleted (nothing to keep)"
        );
    }

    /// `worktree_removal_keeps_branch` predicts the morph: a `RemovedWorktree`
    /// whose `SafeDelete` would retain the branch (unmerged) yields the branch
    /// name; an integrated one (deletes the branch) and a force-delete both yield
    /// `None`. Built from real refs so the prediction runs the same
    /// `integration_reason` the actual delete does.
    #[test]
    fn test_worktree_removal_keeps_branch() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        repo.run_command(&["branch", "integrated"]).unwrap();
        // `unmerged` carries a commit main lacks.
        repo.run_command(&["checkout", "-b", "unmerged"]).unwrap();
        fs::write(test.path().join("new.txt"), "work").unwrap();
        repo.run_command(&["add", "."]).unwrap();
        repo.run_command(&["commit", "-m", "work"]).unwrap();
        repo.run_command(&["checkout", "main"]).unwrap();

        let result = |branch: &str, mode| RemoveResult::RemovedWorktree {
            main_path: test.path().to_path_buf(),
            worktree_path: test.path().join("gone"),
            changed_directory: false,
            branch_name: Some(branch.to_string()),
            deletion_mode: mode,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        assert_eq!(
            super::worktree_removal_keeps_branch(
                &repo,
                &result("unmerged", BranchDeletionMode::SafeDelete)
            )
            .as_deref(),
            Some("unmerged"),
            "an unmerged branch is kept, so the row morphs"
        );
        assert_eq!(
            super::worktree_removal_keeps_branch(
                &repo,
                &result("integrated", BranchDeletionMode::SafeDelete)
            ),
            None,
            "an integrated branch is deleted, so the row drops"
        );
        assert_eq!(
            super::worktree_removal_keeps_branch(
                &repo,
                &result("unmerged", BranchDeletionMode::ForceDelete)
            ),
            None,
            "force-delete removes even an unmerged branch, so the row drops"
        );
    }

    /// `build_morph_branch_row` renders the `/ branch` line a morph swaps in: the
    /// worktree row's model demoted to a local branch on the live layout — gutter
    /// `/`, no path — and a `LocalContent` whose `working_tree` reads empty (no
    /// worktree to diff), which dims the `working_tree` preview tab.
    #[test]
    fn test_build_morph_branch_row() {
        use ansi_str::AnsiStr;

        let mut worktree_item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        worktree_item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: Path::new("/tmp/wt.feature").to_path_buf(),
            ..Default::default()
        }));
        let layout = crate::commands::list::layout::calculate_layout_with_width(
            std::slice::from_ref(&worktree_item),
            &std::collections::HashSet::new(),
            80,
            Path::new("/test"),
            None,
            None,
            crate::commands::list::layout::ColumnSelection {
                custom: &[],
                selected: None,
            },
        );

        let (line, local) = super::build_morph_branch_row(&layout, &worktree_item, Some("main"));
        let plain = line.ansi_strip();
        assert!(
            plain.trim_start().starts_with('/'),
            "the morphed line leads with the local-branch gutter `/`: {plain:?}"
        );
        assert!(
            plain.contains("feature"),
            "the morphed line shows the branch name: {plain:?}"
        );
        assert!(
            !plain.contains("/tmp/wt.feature"),
            "the morphed branch row has no worktree path: {plain:?}"
        );
        assert_eq!(
            local,
            LocalContent::from_item(&{
                let mut b = ListItem::new_branch("abc123".to_string(), "feature".to_string());
                b.kind = ItemKind::Branch(BranchScope::Local);
                b
            }),
            "the morphed row's diff signals are the branch's (working_tree empty)"
        );
    }

    /// `removal_target_still_present` observes reality: a worktree dir or a branch
    /// ref that's gone reads as removed; one still on disk / in the ref store reads
    /// as present (the restore trigger).
    #[test]
    fn test_removal_target_still_present() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        let worktree_result = |path: std::path::PathBuf| RemoveResult::RemovedWorktree {
            main_path: test.path().to_path_buf(),
            worktree_path: path,
            changed_directory: false,
            branch_name: Some("x".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };
        assert!(super::removal_target_still_present(
            &repo,
            &worktree_result(test.path().to_path_buf()) // still on disk
        ));
        assert!(!super::removal_target_still_present(
            &repo,
            &worktree_result(test.path().join("does-not-exist"))
        ));

        repo.run_command(&["branch", "live-branch"]).unwrap();
        let present_branch = RemoveResult::BranchOnly {
            branch_name: "live-branch".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        assert!(super::removal_target_still_present(&repo, &present_branch));

        let gone_branch = RemoveResult::BranchOnly {
            branch_name: "no-such-branch".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            pruned: false,
            target_branch: None,
            integration_reason: None,
        };
        assert!(!super::removal_target_still_present(&repo, &gone_branch));
    }

    /// `removal_will_remove_target` predicts removal from the prepared result
    /// alone: a worktree always removes (it passed `ensure_clean`); a branch-only
    /// row removes only when the branch is integrated or force-deleted, and never
    /// under `Keep` — mirroring `delete_branch_if_safe` so the up-front prediction
    /// can't drift from what `do_removal` does.
    #[test]
    fn test_removal_will_remove_target() {
        use worktrunk::git::IntegrationReason;

        let branch_only = |mode: BranchDeletionMode, integration: Option<IntegrationReason>| {
            RemoveResult::BranchOnly {
                branch_name: "b".to_string(),
                deletion_mode: mode,
                pruned: false,
                target_branch: None,
                integration_reason: integration,
            }
        };

        let worktree = RemoveResult::RemovedWorktree {
            main_path: std::path::PathBuf::from("/repo"),
            worktree_path: std::path::PathBuf::from("/repo.feature"),
            changed_directory: false,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };
        assert!(
            super::removal_will_remove_target(&worktree),
            "a worktree removal always drops the row"
        );

        assert!(
            super::removal_will_remove_target(&branch_only(
                BranchDeletionMode::SafeDelete,
                Some(IntegrationReason::SameCommit)
            )),
            "an integrated branch is safe-deleted"
        );
        assert!(
            !super::removal_will_remove_target(&branch_only(BranchDeletionMode::SafeDelete, None)),
            "an unmerged branch is kept, so the row stays"
        );
        assert!(
            super::removal_will_remove_target(&branch_only(BranchDeletionMode::ForceDelete, None)),
            "force-delete removes even an unmerged branch"
        );
        assert!(
            !super::removal_will_remove_target(&branch_only(
                BranchDeletionMode::Keep,
                Some(IntegrationReason::SameCommit)
            )),
            "Keep never deletes, even when integrated"
        );
    }

    /// `removal_targets_current_worktree` fires only for a `RemovedWorktree` whose
    /// `changed_directory` flag is set (the worktree the picker was launched from);
    /// a non-current worktree and any `BranchOnly` row read as `false`.
    #[test]
    fn test_removal_targets_current_worktree() {
        let path = std::path::PathBuf::from("/repo.feature");
        let worktree = |changed_directory| RemoveResult::RemovedWorktree {
            main_path: std::path::PathBuf::from("/repo"),
            worktree_path: path.clone(),
            changed_directory,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };
        assert!(
            super::removal_targets_current_worktree(&worktree(true)),
            "removing the worktree the picker was launched from"
        );
        assert!(
            !super::removal_targets_current_worktree(&worktree(false)),
            "removing some other worktree"
        );
        assert!(
            !super::removal_targets_current_worktree(&RemoveResult::BranchOnly {
                branch_name: "feature".to_string(),
                deletion_mode: BranchDeletionMode::SafeDelete,
                pruned: false,
                target_branch: None,
                integration_reason: None,
            }),
            "a branch-only row has no worktree to be standing in"
        );
    }

    /// `keep_current_worktree_row` keeps the row in place and stashes the
    /// can't-remove-current-worktree info + switch-away hint — alt-x on the current
    /// worktree never removes it and never spawns a background removal.
    #[test]
    fn test_keep_current_worktree_row() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        let item = branched_picker_item("current", &test.path().join("current"));
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let remover = test_remover(Arc::clone(&items), repo.clone());

        remover.keep_current_worktree_row();

        assert_eq!(
            items
                .lock()
                .unwrap()
                .iter()
                .map(|item| item.output().into_owned())
                .collect::<Vec<_>>(),
            vec![token.clone()],
            "the current worktree row is kept, not removed"
        );
        let warnings = remover.stashed_warnings.lock().unwrap().clone();
        assert!(
            warnings.iter().any(|w| w.contains("current worktree")),
            "stashes the can't-remove-current-worktree info: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("Switch to another worktree")),
            "stashes the switch-away hint: {warnings:?}"
        );

        // A second alt-x on the same kept row dedups — the stash doesn't grow.
        remover.keep_current_worktree_row();
        assert_eq!(
            remover.stashed_warnings.lock().unwrap().len(),
            warnings.len(),
            "repeated alt-x on the current worktree stashes the hint only once"
        );
    }

    /// alt-x on an unremovable target surfaces the same diagnostic `wt remove`
    /// prints rather than swallowing it: `prepare_removal` errors (here the main
    /// worktree can't be removed), so the dispatch's `Err` arm stashes the rendered
    /// reason and keeps the row in place — no silent dead keypress.
    #[test]
    fn test_apply_surfaces_unremovable_diagnostic() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        // The repo root is the main worktree — `prepare_worktree_removal` rejects it.
        let item = branched_picker_item("main", test.path());
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let remover = test_remover(Arc::clone(&items), repo.clone());

        remover.apply(token.clone());

        // Nothing was removed, so the row stays...
        assert_eq!(
            items
                .lock()
                .unwrap()
                .iter()
                .map(|item| item.output().into_owned())
                .collect::<Vec<_>>(),
            vec![token],
            "an unremovable row is never dropped"
        );
        // ...and the reason is surfaced, not swallowed.
        let warnings = remover.stashed_warnings.lock().unwrap().clone();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("main worktree cannot be removed")),
            "the unremovable diagnostic is stashed for the user: {warnings:?}"
        );
    }

    /// alt-x on an unmerged branch-only row never drops it (no flicker): an
    /// unmerged branch with no worktree resolves to `BranchOnly` with no
    /// integration reason, so `removal_will_remove_target` predicts `SafeDelete`
    /// keeps it. Decided synchronously in `invoke` — no background removal — so the
    /// row stays and a one-time `kept … branch` hint is stashed. Driven end-to-end.
    #[test]
    fn test_apply_keeps_unmerged_branch_only_row() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();

        // An unmerged branch (a commit not on main) with no worktree — SafeDelete
        // keeps it.
        repo.run_command(&["checkout", "-b", "unmerged"]).unwrap();
        fs::write(test.path().join("new.txt"), "unmerged work").unwrap();
        repo.run_command(&["add", "."]).unwrap();
        repo.run_command(&["commit", "-m", "unmerged work"])
            .unwrap();
        repo.run_command(&["checkout", "main"]).unwrap();

        let item = branch_only_picker_item("unmerged");
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let stashed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let remover = AltXRemover {
            items: Arc::clone(&items),
            repo: repo.clone(),
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
            stashed_warnings: Arc::clone(&stashed),
            shortcut_table: Arc::new(Mutex::new(std::collections::HashMap::new())),
            layout_slot: Arc::new(Mutex::new(None)),
        };

        remover.apply(token.clone());

        // The keep path is synchronous (no background thread), so by the time
        // `apply` returns the row is still present and the hint is stashed.
        let outputs: Vec<String> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().into_owned())
            .collect();
        assert_eq!(
            outputs,
            vec![token.clone()],
            "the unmerged branch-only row is never dropped"
        );
        let warnings = stashed.lock().unwrap().clone();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unmerged") && w.contains("retained")),
            "a kept unmerged branch stashes a `retained` info line: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("wt remove -D unmerged")),
            "a kept unmerged branch stashes the actionable `-D` hint: {warnings:?}"
        );

        // A second alt-x on the same kept row dedups — the stash doesn't grow.
        remover.apply(token);
        assert_eq!(
            stashed.lock().unwrap().clone(),
            warnings,
            "repeated alt-x on the same kept row stashes the hint only once"
        );

        let branch_list = repo.run_command(&["branch", "--list", "unmerged"]).unwrap();
        assert!(!branch_list.is_empty(), "the unmerged branch is preserved");
    }

    // Note: skim's `as_any().downcast_ref::<PickerRow>()` can fail at
    // runtime due to a TypeId mismatch between skim's reader thread and the main
    // compilation unit. The invoke() code path uses output() matching instead.
    // Full TUI tests require interactive skim — verified via tmux-cli during
    // development.
}
