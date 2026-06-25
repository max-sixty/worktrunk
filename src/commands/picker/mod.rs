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
use worktrunk::git::{Repository, current_or_recover};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, warning_message};

use super::hook_plan::{ApprovedHookPlan, HookPlanBuilder};
use super::hooks::HookAnnouncer;
use super::list::collect;
use super::list::progressive::RenderTarget;
use super::repository_ext::{RemoveTarget, RepositoryCliExt};
use super::worktree::{RemoveResult, SwitchPipeline};
use crate::cli::SwitchFormat;
use crate::output::{BackgroundFallbackMode, handle_remove_output};
use worktrunk::git::{BranchDeletionMode, delete_branch_if_safe};

use items::{PreviewCache, ShortcutTable, WORKTREE_OUTPUT_PREFIX};
use preview::{PreviewLayout, PreviewMode, PreviewState, PreviewStateData};
use preview_orchestrator::PreviewOrchestrator;

/// Drain stashed warnings to stderr. Called after skim has released the
/// terminal (or in the dry-run path after the bg thread joins) — eprintln
/// during the picker would corrupt skim's frame, so collect routes warnings
/// through `PickerProgressHandler::stash_warning` and we emit them here.
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
/// Decoding `output()` rather than `downcast_ref::<WorktreeSkimItem>()` also
/// sidesteps skim's cross-thread `TypeId` mismatch, which can make the
/// downcast fail when the item originates on the reader thread.
fn picker_item_identifier(item: &dyn SkimItem) -> String {
    let output = item.output().to_string();
    match PickerRemovalTarget::from_signal(&output) {
        Some(PickerRemovalTarget::WorktreePath(path)) => path.to_string_lossy().into_owned(),
        _ => output,
    }
}

/// Custom command collector for skim's `reload` action.
///
/// When alt-x is pressed, skim's `reload(remove {})` action expands `{}` to the
/// selected row's output() token and invokes this collector with it. The
/// collector parses the token, removes that item from the list, and streams the
/// remaining items back to skim — all without leaving the picker. (alt-r's
/// `reload(refresh)` re-enters the same collector to re-run collect — see
/// [`PickerCollector::invoke`].)
///
/// The token rides the reload command itself rather than a side-channel file:
/// skim 4.x's `execute-silent` is fire-and-forget, so the old
/// `execute-silent(echo {} > file)+reload` chain raced — the reader read the
/// file before the echo landed and removed nothing (or, on a repeat press, the
/// wrong worktree).
///
/// Git operations (worktree removal, branch deletion) are deferred to a background
/// thread because skim calls `invoke()` on the main event loop thread.
/// Blocking it freezes the TUI.
///
/// skim resets the cursor to the top on every reload (`handle_reload` clears
/// `item_list` before the new rows stream in — skim #1695). To keep the cursor
/// sticky, `invoke` injects a [`reposition_cursor_action`] Custom action that
/// moves the cursor back to the slot the removed row occupied once the reloaded
/// rows land.
///
/// The row is dropped optimistically (before the background removal runs), so
/// the list can't show a removal that didn't happen: once `do_removal` returns,
/// the thread checks whether the target still exists
/// ([`removal_target_still_present`]) and, if so, calls
/// [`restore_failed_removal`] to put the row back and surface why. Observing the
/// target — rather than trusting `do_removal`'s `Result` — handles both a removal
/// that errors *after* the worktree is gone and a branch-only safe-delete refusal
/// that returns `Ok` while keeping the branch.
struct PickerCollector {
    items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    repo: Repository,
    /// Approvals snapshot, loaded once at picker startup. A queued removal runs
    /// its `pre-remove` / `post-remove` / `post-switch` hooks only when every
    /// one is in here — the picker can't show an approval prompt mid-render, so
    /// unapproved project commands are skipped, never run. See
    /// [`approved_removal_plan`].
    approvals: Arc<Approvals>,
    /// skim's event sender, published once the TUI is initialized (same
    /// `OnceLock` the progressive handler pushes `Event::Render` through). alt-x
    /// removals inject a [`reposition_cursor_action`] through it to restore the
    /// cursor after the reload. `None` until the TUI is up — but a reload can
    /// only fire after skim is showing rows, so it's always set by then.
    render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    /// Re-runs the collect pipeline for the `alt-r` refresh: `reload(refresh)`
    /// routes here, and [`PipelineFactory::spawn`] streams a fresh item list
    /// back. Shared (`Rc`) with `handle_picker`, which used it for the initial
    /// spawn.
    factory: Rc<PipelineFactory>,
    /// Same warning stash the progressive handler fills (drained to stderr once
    /// skim releases the terminal). A failed background removal pushes a
    /// `worktree kept` warning here so the user learns the row that flickered
    /// back didn't actually go away. See [`restore_failed_removal`].
    stashed_warnings: Arc<Mutex<Vec<String>>>,
}

impl PickerCollector {
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
    /// For a removal [`removal_will_remove_target`] predicts will actually remove
    /// the target (a clean worktree, or an integrated / force-deleted branch). The
    /// `output()` token is unique per row (a `worktree-path:` path for worktrees),
    /// so this drops exactly the selected row even when several detached rows share
    /// the `(detached)` branch label.
    ///
    /// The row drops optimistically so the list stays snappy; the git work runs on
    /// a background thread off skim's event loop. The dropped row is restored only
    /// when the target survives — observed directly ([`removal_target_still_present`]),
    /// not inferred from `do_removal`'s `Result`, which is `Err` after a successful
    /// removal whose `post-remove` hook fails to render/spawn, and `Ok` after an
    /// integrated→unmerged race leaves the branch in place. This keeps the list
    /// from showing a removal that didn't happen without ever resurrecting a row
    /// for a target that's actually gone.
    fn drop_and_remove_in_background(
        &mut self,
        selected_output: String,
        planning_repo: Repository,
        result: RemoveResult,
    ) {
        // Capture the removed row (and its position) before dropping it: the
        // position restores the cursor to that slot (the row that slides up) after
        // the reload — see `reposition_cursor_action` — and the row itself is
        // handed to the background thread so it can put the row back if the removal
        // fails (see `restore_failed_removal`).
        let (removed, reposition_target) = {
            let mut items = self.items.lock().unwrap();
            let removed = items
                .iter()
                .position(|item| item.output().as_ref() == selected_output)
                .map(|pos| (Arc::clone(&items[pos]), pos));
            items.retain(|item| item.output().as_ref() != selected_output);
            let remaining_data_rows = items.len().saturating_sub(PICKER_HEADER_ROWS);
            let target = removed
                .as_ref()
                .and_then(|(_, pos)| sticky_reposition_target(*pos, remaining_data_rows));
            (removed, target)
        };

        // If removing the current worktree, cd to home so skim and git commands
        // continue to work after the directory disappears.
        if matches!(
            &result,
            RemoveResult::RemovedWorktree {
                changed_directory: true,
                ..
            }
        ) && let Some(home) = result.destination_path()
        {
            let _ = std::env::set_current_dir(home);
            if let Ok(repo) = Repository::at(home) {
                self.repo = repo;
            }
        }

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

        // Restore the cursor near the removed row. skim resets it to the top on
        // every reload; inject a Custom action that moves it back to the removed
        // row's slot once the reloaded rows land (`render_tx` is skim's event
        // sender, set once the TUI is up — always present by the time a reload
        // fires).
        if let Some(target) = reposition_target {
            send_reposition(&self.render_tx, target);
        }
    }

    /// Keep the selected row in place and explain why its target wasn't removed.
    ///
    /// Called from `invoke` when [`removal_will_remove_target`] predicts the
    /// removal would keep the target — a branch-only row whose branch is unmerged,
    /// which `SafeDelete` declines to delete (data safety). Deciding this up front
    /// from `prepare_removal`'s already-computed integration check means the row
    /// never drops (no flicker) and no background `do_removal` runs for a no-op.
    /// alt-x's reload still resets the cursor to the top, so this lands it back on
    /// the kept row and stashes the canonical "retained; unmerged" info + hint pair
    /// `wt remove` itself prints (see `print_retained_unmerged_branch`), deduped
    /// and drained to stderr when the picker exits. (This is a by-design retain,
    /// not a failure — distinct from [`restore_failed_removal`]'s `kept … could
    /// not remove it` warning.)
    fn keep_unremovable_row(&self, selected_output: &str, branch_name: &str) {
        // The canonical "retained; unmerged" info + hint `wt remove` prints,
        // shared so the picker copy can't drift (see
        // `retained_unmerged_branch_messages`). Taking the branch name (not the
        // whole `RemoveResult`) makes it unrepresentable for this keep path to be
        // handed a `RemovedWorktree`, which always removes — see the dispatch in
        // `invoke` and [`removal_will_remove_target`].
        let (info, hint) = crate::output::retained_unmerged_branch_messages(branch_name);
        {
            let mut stashed = self.stashed_warnings.lock().unwrap();
            // Dedup on repeated alt-x of the same kept row.
            if !stashed.contains(&info) {
                stashed.push(info);
                stashed.push(hint);
            }
        }

        // The row never moved, so land the cursor back on it (alt-x's reload reset
        // it to the top).
        let reposition_target = {
            let items = self.items.lock().unwrap();
            items
                .iter()
                .position(|item| item.output().as_ref() == selected_output)
                .and_then(|pos| {
                    let remaining = items.len().saturating_sub(PICKER_HEADER_ROWS);
                    sticky_reposition_target(pos, remaining)
                })
        };
        if let Some(target) = reposition_target {
            send_reposition(&self.render_tx, target);
        }
    }
}

/// Pull the selected row's `output()` token out of the `remove <token>` reload
/// command skim builds for alt-x. skim expands `{}` to `output()` and shell-
/// quotes it via single quotes (`'…'`, with any embedded `'` written as
/// `'\''`); this reverses exactly that. An empty selection yields `''` →
/// empty string, which `from_signal` treats as "nothing to remove".
fn parse_reload_remove_token(cmd: &str) -> String {
    let arg = cmd.strip_prefix("remove ").unwrap_or("").trim();
    let unquoted = arg
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .unwrap_or(arg);
    unquoted.replace("'\\''", "'")
}

/// Number of leading non-selectable header rows the picker streams (the single
/// `HeaderSkimItem`). The skim options pass this to `.header_lines(...)`: skim
/// reserves these from the item pool into its own Header widget, so `item_list`
/// — what the cursor moves over — holds data rows only, indexed from 0.
const PICKER_HEADER_ROWS: usize = 1;

/// Consecutive "matcher settled but list still empty" observations
/// [`reposition_cursor_action`] waits out before concluding the reloaded list is
/// genuinely empty (no row to land on). `item_list.count()` lags the matcher by
/// one render — the matcher writes its result, then a later `Event::Render`
/// applies it — so a bare `settled && count() == 0` could give up while matches
/// are still one render away. Each re-arm queues a Render (skim appends one after
/// every action), so three consecutive settled observations guarantee the
/// matcher's result has been applied before giving up.
const REPOSITION_SETTLED_RENDERS: usize = 3;

/// Hard backstop on [`reposition_cursor_action`] re-arms, far above the handful a
/// normal reload needs. The `settled` check is the real stop condition; this only
/// guards against an unforeseen state where the matcher never settles.
const REPOSITION_MAX_ATTEMPTS: usize = 1000;

/// The `item_list` row the cursor should land on after an alt-x removal, given
/// the removed row's position in `shared_items` (header included) and how many
/// data rows remain.
///
/// The removed row sat at `removed_pos`; the row that slides up into its slot is
/// the next one, which lands at the same `item_list` index once the header is
/// subtracted. Returns `None` when there's nothing to land on (the list is now
/// header-only) or the position was the header itself. The caller clamps to the
/// list's last row via `scroll_by`, so a removed-last-row target just overshoots
/// and snaps back to the new last row.
fn sticky_reposition_target(removed_pos: usize, remaining_data_rows: usize) -> Option<usize> {
    if remaining_data_rows == 0 {
        return None;
    }
    removed_pos.checked_sub(PICKER_HEADER_ROWS)
}

/// A skim `Custom` action that moves the cursor to `target` once the reloaded
/// item list is populated.
///
/// skim has no "set cursor to index N" action and resets the cursor to the top
/// on reload, so this drives the move through `ItemList`'s public cursor API
/// with `&mut App` in hand: `jump_to_first` + `scroll_by(target)` lands on
/// `target`, clamped to the last row.
///
/// The reload repopulates `item_list` asynchronously — the reader refills
/// `item_pool`, then the matcher filters it into `item_list` — so the first
/// invocation usually runs before the rows exist. It re-arms (returns another
/// copy of itself; skim queues a Render after each) until the rows land. Stopping
/// is gated on the matcher, not a blind count: once it has settled on an empty
/// result (a removal that emptied the list, or an active query now matching
/// nothing) for [`REPOSITION_SETTLED_RENDERS`] checks, there's nothing to land
/// on. Sleeping instead of re-arming would hold `&mut App` across the await and
/// starve the render that loads the rows.
///
/// `target` is an `item_list` index (data rows only — see [`PICKER_HEADER_ROWS`])
/// from [`sticky_reposition_target`]. Under an active fuzzy query the displayed
/// order diverges from `shared_items` order, so the landing row is approximate
/// (a valid nearby row) rather than the exact next row.
///
/// On landing, it returns [`Event::RunPreview`] so the preview pane repaints for
/// the row the cursor settled on. skim only repaints the preview on a
/// selection-*change* event (`on_selection_changed`), and moving the cursor
/// through the `ItemList` API here is not one — without this, the pane keeps
/// showing the row skim last previewed (the current worktree, which the reload
/// briefly reset the cursor to) until the next keystroke.
fn reposition_cursor_action(
    target: usize,
    attempts: Arc<AtomicUsize>,
    settled_streak: Arc<AtomicUsize>,
) -> Action {
    Action::Custom(ActionCallback::new_sync(
        move |app| -> Result<Vec<Event>, Box<dyn std::error::Error + Send + Sync>> {
            // Rows are in: land the cursor on the removed row's slot, then
            // repaint the preview for it (the cursor move alone doesn't).
            if app.item_list.count() > 0 {
                app.item_list.jump_to_first();
                app.item_list
                    .scroll_by(i32::try_from(target).unwrap_or(i32::MAX));
                return Ok(vec![Event::RunPreview]);
            }
            // No rows yet. The matcher has "settled" once it has stopped with the
            // reloaded items taken and a non-empty pool (the empty pool is the
            // pre-refill transient). A settled-but-empty `item_list` means the
            // reload produced no matchable rows — wait out the count() render lag,
            // then give up. `attempts` is a hard backstop for an unforeseen
            // never-settles state.
            let matcher_settled = app.matcher_control.stopped()
                && !app.item_pool.is_empty()
                && app.item_pool.num_not_taken() == 0;
            let streak = if matcher_settled {
                settled_streak.fetch_add(1, Ordering::Relaxed) + 1
            } else {
                settled_streak.store(0, Ordering::Relaxed);
                0
            };
            if streak >= REPOSITION_SETTLED_RENDERS
                || attempts.fetch_add(1, Ordering::Relaxed) >= REPOSITION_MAX_ATTEMPTS
            {
                return Ok(Vec::new());
            }
            Ok(vec![Event::Action(reposition_cursor_action(
                target,
                Arc::clone(&attempts),
                Arc::clone(&settled_streak),
            ))])
        },
    ))
}

/// Inject a cursor reposition onto `target` through skim's event sender, with
/// fresh attempt/streak counters (see [`reposition_cursor_action`]). The single
/// path every alt-x outcome uses to move the cursor after its reload — the drop
/// (cursor onto the slide-up row), the keep (back onto the row), and the restore
/// (back onto the re-inserted row). A no-op before `render_tx` is set or once the
/// receiver is gone (teardown); the queued action is dropped if the channel is
/// full.
///
/// Rapid alt-r (or a background restore overtaking the optimistic drop) can leave
/// more than one chain in flight. Each carries its own counters and self-terminates
/// once the rows land, so the last to run wins — under a burst the cursor may
/// briefly sit on a superseded (but valid) row's slot, corrected on the next
/// render. Bounding this to only the newest reposition would take a generation
/// token threaded through every chain (and every `PickerCollector` construction
/// site); the self-correcting transient isn't worth that cross-chain state.
fn send_reposition(render_tx: &OnceLock<tokio::sync::mpsc::Sender<Event>>, target: usize) {
    if let Some(event_tx) = render_tx.get() {
        let _ = event_tx.try_send(Event::Action(reposition_cursor_action(
            target,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        )));
    }
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
/// it never drops (no flicker) — see [`PickerCollector::keep_unremovable_row`].
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
fn removal_target_still_present(repo: &Repository, result: &RemoveResult) -> bool {
    match result {
        RemoveResult::RemovedWorktree { worktree_path, .. } => worktree_path.exists(),
        RemoveResult::BranchOnly { branch_name, .. } => {
            repo.branch(branch_name).exists_locally().unwrap_or(false)
        }
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
/// `shared_items` at its original slot, stashes
/// a `kept` warning (drained to stderr once skim releases the terminal; the full
/// error, if any, is in the `tracing::warn!` the caller emits), then reloads the
/// picker to re-stream the restored list and lands the cursor back on the row.
///
/// The reload command is any string that is neither `remove <token>` nor the
/// `refresh` re-collect: `invoke` re-streams `shared_items` without removing
/// anything for those (see [`parse_reload_remove_token`]), so a plain `restore`
/// reload repaints the re-inserted row — the same reload→reposition path the
/// happy alt-x case uses.
fn restore_failed_removal(
    items: &Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    render_tx: &Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    stashed_warnings: &Arc<Mutex<Vec<String>>>,
    removed_item: Arc<dyn SkimItem>,
    removed_pos: usize,
    label: &str,
    noun: &str,
) {
    let reposition_target = {
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
        let remaining_data_rows = items.len().saturating_sub(PICKER_HEADER_ROWS);
        sticky_reposition_target(insert_at, remaining_data_rows)
    };

    stashed_warnings.lock().unwrap().push(
        warning_message(cformat!(
            "Kept <bold>{label}</> {noun} — could not remove it"
        ))
        .to_string(),
    );

    let Some(event_tx) = render_tx.get() else {
        return;
    };
    // Re-stream the restored list, then land the cursor back on the row.
    let _ = event_tx.try_send(Event::Reload("restore".to_string()));
    if let Some(target) = reposition_target {
        send_reposition(render_tx, target);
    }
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
        if cmd.trim() == "refresh" {
            match self.factory.spawn() {
                Ok(SpawnedPipeline { rx, .. }) => {
                    let (tx_interrupt, _rx_interrupt) = bounded(1);
                    return (rx, tx_interrupt);
                }
                Err(e) => log::warn!("picker: refresh failed: {e:#}"),
            }
        }

        // skim's `reload(remove {})` expands `{}` to the selected row's
        // shell-quoted output() token; pull it back out (see
        // `parse_reload_remove_token`). No signal file — that raced the reader.
        {
            let selected_output = parse_reload_remove_token(cmd);
            if let Some(removal_target) = PickerRemovalTarget::from_signal(&selected_output) {
                let preparation = self.prepare_removal(&removal_target);

                match preparation {
                    Ok((planning_repo, result)) => {
                        // Decide up front, from the already-computed result, whether
                        // this removal will actually remove the target. Only an
                        // outcome that removes drops the row; a branch-only row whose
                        // branch is unmerged stays put and is explained, so the list
                        // never flickers a row off and back on (see
                        // `removal_will_remove_target`).
                        if removal_will_remove_target(&result) {
                            self.drop_and_remove_in_background(
                                selected_output,
                                planning_repo,
                                result,
                            );
                        } else if let RemoveResult::BranchOnly { branch_name, .. } = &result {
                            // The only non-removing outcome: `removal_will_remove_target`
                            // returns false solely for an unmerged `BranchOnly` row (a
                            // `RemovedWorktree` always removes, so it never reaches here).
                            // `keep_unremovable_row` taking the branch name — not the whole
                            // result — keeps that narrowing at the type level.
                            self.keep_unremovable_row(&selected_output, branch_name);
                        }
                    }
                    Err(e) => {
                        tracing::info!(selected_output = %selected_output, error = %e, "picker: cannot remove '{selected_output}': {e:#}");
                    }
                }
            }
        }

        // Stream remaining items through a channel for skim to consume. skim
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
/// warm), `shared_items` and `shortcut_table` (which `on_skeleton` overwrites),
/// and skim's `render_tx`. Held by [`PickerCollector`] so a refresh can
/// re-enter the pipeline.
struct PipelineFactory {
    repo: Repository,
    render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    shortcut_table: ShortcutTable,
    preview_cache: PreviewCache,
    orchestrator: Arc<PreviewOrchestrator>,
    stashed_warnings: Arc<Mutex<Vec<String>>>,
    grid_slot: Arc<prs::GridSlot>,
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
    fn spawn(&self) -> anyhow::Result<SpawnedPipeline> {
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();

        // Fresh per spawn: the header shows a "loading…" marker keyed to this
        // flag while the forge call is in flight.
        let prs_loading: Option<Arc<AtomicBool>> =
            (self.show_prs && !self.is_preview_bench).then(|| Arc::new(AtomicBool::new(true)));

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
                orchestrator: Arc::clone(&self.orchestrator),
                preview_dims: self.preview_dims,
                llm_command: self.llm_command.clone(),
                summary_hint: self.summary_hint.clone(),
                stashed_warnings: Arc::clone(&self.stashed_warnings),
                deferred_items: OnceLock::new(),
                grid_slot: Arc::clone(&self.grid_slot),
                prs_loading: prs_loading.clone(),
            });

        let bg_handler: Arc<dyn collect::PickerProgressHandler> = handler.clone();
        // Fresh `Repository` per spawn so a refresh re-enumerates worktrees.
        // The factory's `repo` was primed with `git worktree list` at picker
        // startup, and `RepoCache.worktrees` is a `OnceCell` that is never
        // invalidated; cloning would re-probe that stale list through the same
        // cache. After an in-picker removal the removed worktrees would still
        // appear, and collect's per-worktree git ops would fail against the
        // gone branches ("fatal: Needed a single revision"). `Repository::at`
        // rebuilds the cache, which is exactly the post-mutation discipline the
        // `RepoCache` docs and `prepare_removal` already require. `bg_repo` and
        // `prs_repo` share this one fresh inventory via clone.
        let spawn_repo = Repository::at(self.repo.discovery_path())?;
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
            let prs_shared = prs::PrsShared {
                grid_slot: Arc::clone(&self.grid_slot),
                shortcut_table: Arc::clone(&self.shortcut_table),
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
    let orchestrator = Arc::new(PreviewOrchestrator::new(repo.clone()));
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

    // Skip BranchDiff — it walks history per item for a column the picker
    // doesn't surface. Keep the CiStatus task: the picker primes its CI cells
    // from the local cache so the first frame shows cached status (see
    // `populate_from_cache`), then this task fetches live and streams each row's
    // status in behind the frame — the same 30–60s-TTL cache plus live fetch as
    // `wt list --full`. The picker's lifetime is bounded by the user, so a slow forge call
    // never blocks anything (see the "Network Access" notes in CLAUDE.md). The
    // `pr` preview tab reads the same live status. `--prs` rows carry their own
    // number from the explicit `--prs` forge call.
    let skip_tasks: std::collections::HashSet<collect::TaskKind> =
        [collect::TaskKind::BranchDiff].into_iter().collect();

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
            let hint = if !config.commit_generation.is_configured() {
                "Configure [commit.generation] command to enable LLM summaries.\n\n\
                 Example in ~/.config/worktrunk/config.toml:\n\n\
                 [commit.generation]\n\
                 command = \"llm -m haiku\"\n\n\
                 [list]\n\
                 summary = true\n"
            } else {
                "Enable summaries in ~/.config/worktrunk/config.toml:\n\n\
                 [list]\n\
                 summary = true\n"
            };
            (None, Some(hint.to_string()))
        };

    // Shared items list: populated by the handler's `on_skeleton` and read
    // by `PickerCollector` on alt-x reload. Starts empty — the collector's
    // `invoke` only fires after skim has displayed items, by which time
    // the handler has already published them.
    let shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>> = Arc::new(Mutex::new(Vec::new()));

    // `alt-y` / `alt-o` lookup table (token → branch + URL). The collect handler
    // fills it with worktree/branch rows and the `--prs` thread extends it; the
    // shortcut keybinding callbacks read it. See `ShortcutTable`.
    let shortcut_table: ShortcutTable = Arc::new(Mutex::new(std::collections::HashMap::new()));

    // Approvals snapshot for the session: alt-x removals consult it read-only
    // to filter the hook plan; see `approved_removal_plan`.
    let approvals = Arc::new(Approvals::load().context("Failed to load approvals")?);

    // skim 4.x repaints on demand, so the collect handler needs a handle to
    // skim's event loop to surface in-place row updates. The picker fills this
    // once `Skim::init_tui` has run (inside `run_skim`); until then the handler
    // simply skips its render pokes. The alt-x collector shares it too, to inject
    // the cursor-reposition action after a removal. See `progressive_handler`
    // module docstring.
    let render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>> = Arc::new(OnceLock::new());

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
        shortcut_table: Arc::clone(&shortcut_table),
        preview_cache: Arc::clone(&preview_cache),
        orchestrator: Arc::clone(&orchestrator),
        stashed_warnings: Arc::clone(&stashed_warnings),
        // Column-geometry handoff: the collect thread fills it at skeleton time,
        // the `--prs` thread reads it to align PR rows with the worktree rows.
        grid_slot: Arc::new(prs::GridSlot::new()),
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

    let collector = PickerCollector {
        items: Arc::clone(&shared_items),
        repo: repo.clone(),
        approvals,
        render_tx: Arc::clone(&render_tx),
        factory: Rc::clone(&factory),
        stashed_warnings: Arc::clone(&stashed_warnings),
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
        // First line (header) non-selectable. `PICKER_HEADER_ROWS` mirrors this
        // count so the alt-x cursor-reposition math stays in sync — keep them one.
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
            // Remove selected worktree: `reload(remove {})` hands the selected
            // row's output() token to PickerCollector, which performs the removal
            // and streams updated items back — all without leaving the picker.
            // Passing the token through the reload cmd (not an execute-silent +
            // file write) sidesteps skim 4.x's fire-and-forget execute-silent,
            // which raced the reader and removed nothing. The collector also
            // re-positions the cursor onto the removed row's slot afterward —
            // reload otherwise snaps it back to the top (see PickerCollector).
            // alt-x for "remove" — alt-r is the refresh key (below), and putting
            // the destructive action on a less-reflexive key guards against a
            // mis-hit, the safe direction being a stray refresh.
            "alt-x:reload(remove {})".to_string(),
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
    worktrunk::trace::instant("Picker skim options built");

    // Spawn the collect pipeline (and the `--prs` thread when active). The
    // handler holds the only non-thread `tx` clone; when the bg threads exit,
    // `tx` drops, skim's reader sees EOF, and the picker goes idle. Every alt-r
    // refresh re-runs this same `factory.spawn()` — see `PipelineFactory`.
    let SpawnedPipeline {
        rx,
        handler,
        collect_handle,
        prs_handle,
    } = factory.spawn()?;
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

    // Run skim (single invocation — alt-x/alt-r use reload, not re-launch).
    // Skim receives items as the bg thread's handler sends them, and the
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
        // Remove is handled inline via reload — it never reaches accept.
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
    use super::items::{LocalContent, WorktreeSkimItem};
    use super::{
        PickerAction, PickerCollector, PickerRemovalTarget, drain_stashed_warnings,
        install_preview_tab_keybindings, install_shortcut_keybindings, parse_reload_remove_token,
        picker_item_identifier, resolve_identifier, resolve_shortcut_branch, resolve_shortcut_url,
    };
    use crate::commands::list::model::{ItemKind, ListItem, WorktreeData};
    use crate::commands::worktree::RemoveResult;
    use skim::prelude::SkimItem;
    use skim::reader::CommandCollector;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
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
                },
            );
            t.insert(
                "wt".into(),
                RowShortcutData {
                    branch: None,
                    url: RowUrl::Static(None),
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

    /// `parse_reload_remove_token` reverses skim's `remove {}` expansion: it
    /// strips the `remove ` verb and the single-quote wrapping skim adds, and
    /// undoes the `'\''` escaping. An empty selection (`''`) yields "".
    #[test]
    fn test_parse_reload_remove_token() {
        assert_eq!(
            parse_reload_remove_token("remove 'worktree-path:/tmp/wt foo'"),
            "worktree-path:/tmp/wt foo"
        );
        assert_eq!(parse_reload_remove_token("remove 'feature/x'"), "feature/x");
        assert_eq!(parse_reload_remove_token("remove ''"), "");
        // embedded single quote: skim writes ' as '\''
        assert_eq!(parse_reload_remove_token("remove 'it'\\''s'"), "it's");
        // missing verb / unquoted fall back to the trimmed remainder
        assert_eq!(parse_reload_remove_token("remove plain"), "plain");
    }

    /// `sticky_reposition_target` maps a removed row's `shared_items` position
    /// (header at index 0) to the `item_list` index the cursor should land on —
    /// the data-row index of the slot the removed row vacated. It declines when
    /// the list is now header-only (nothing to land on); `scroll_by` clamps the
    /// removed-last-row overshoot, so the helper itself never caps the target.
    #[test]
    fn test_sticky_reposition_target() {
        // First data row (shared_items index 1) → item_list index 0.
        assert_eq!(super::sticky_reposition_target(1, 3), Some(0));
        // Third data row (index 3) → item_list index 2, with rows remaining.
        assert_eq!(super::sticky_reposition_target(3, 2), Some(2));
        // Removed the only data row → header-only list, nothing to land on.
        assert_eq!(super::sticky_reposition_target(1, 0), None);
        // The header position itself never repositions.
        assert_eq!(super::sticky_reposition_target(0, 2), None);
        // Removed-last-row target may exceed the remaining rows; the helper
        // returns it verbatim and leaves clamping to `scroll_by`.
        assert_eq!(super::sticky_reposition_target(4, 3), Some(3));
    }

    /// `send_reposition` (the single path the drop/keep/restore cursor moves
    /// share) queues an `Event::Action` through skim's sender when the TUI is up,
    /// and is a no-op before the sender is set.
    #[test]
    fn test_send_reposition_emits_action_when_render_tx_set() {
        let render_tx: OnceLock<tokio::sync::mpsc::Sender<skim::prelude::Event>> = OnceLock::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        render_tx.set(tx).unwrap();

        super::send_reposition(&render_tx, 2);
        assert!(
            matches!(rx.try_recv(), Ok(skim::prelude::Event::Action(_))),
            "a set sender receives a reposition Action"
        );

        // No sender set → no panic, nothing emitted.
        let empty: OnceLock<tokio::sync::mpsc::Sender<skim::prelude::Event>> = OnceLock::new();
        super::send_reposition(&empty, 0);
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

        PickerCollector::do_removal(&repo, &result, &Approvals::default()).unwrap();
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
        PickerCollector::do_removal(&repo, &result, &Approvals::default()).unwrap();

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
        PickerCollector::do_removal(&repo, &result, &Approvals::default()).unwrap();

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

        PickerCollector::do_removal(&repo, &result, &Approvals::default()).unwrap();
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

        let collector = test_collector(Arc::new(Mutex::new(Vec::new())), repo);

        let target = PickerRemovalTarget::from_signal("branch-only-feature").unwrap();
        let (_planning_repo, result) = collector.prepare_removal(&target).unwrap();
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

        let collector = test_collector(Arc::new(Mutex::new(Vec::new())), repo);

        // `RemoveResult` isn't `Debug`; drop the Ok payload so `unwrap_err`
        // (which needs `T: Debug`) can report a failure cleanly.
        let target = PickerRemovalTarget::from_signal("no-such-branch").unwrap();
        let err = collector
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
        PickerCollector::do_removal(&repo, &result, &approvals).unwrap();
        assert!(!wt_path.exists(), "worktree should be removed");
        assert!(!marker.exists(), "unapproved pre-remove hook must not run");
    }

    /// Build a `WorktreeSkimItem` from a snapshot `ListItem`.
    fn picker_item(branch_name: &str, item: ListItem) -> Arc<dyn SkimItem> {
        let pr_status = Arc::new(Mutex::new(item.pr_status.clone()));
        Arc::new(WorktreeSkimItem {
            search_text: branch_name.to_string(),
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: branch_name.to_string(),
            item: Arc::new(item),
            preview_cache: Arc::new(dashmap::DashMap::new()),
            has_upstream: false,
            summaries_enabled: false,
            pr_status,
            local_content: Arc::new(Mutex::new(LocalContent::default())),
        }) as Arc<dyn SkimItem>
    }

    /// Build a `WorktreeSkimItem` standing in for a detached-worktree row.
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

    /// Build a `WorktreeSkimItem` standing in for a branched-worktree row.
    fn branched_picker_item(branch: &str, path: &Path) -> Arc<dyn SkimItem> {
        let mut item = ListItem::new_branch("abc123".to_string(), branch.to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: path.to_path_buf(),
            ..Default::default()
        }));
        picker_item(branch, item)
    }

    /// Build a `WorktreeSkimItem` standing in for a branch-only row (no worktree).
    fn branch_only_picker_item(branch: &str) -> Arc<dyn SkimItem> {
        picker_item(
            branch,
            ListItem::new_branch("abc123".to_string(), branch.to_string()),
        )
    }

    /// A real [`PipelineFactory`] with empty config for the removal / `invoke`
    /// tests. Its `spawn()` is only reached by the refresh verb, which these
    /// tests don't exercise, so the minimal field set is enough to satisfy the
    /// type without standing up a full picker.
    fn test_factory(repo: worktrunk::git::Repository) -> std::rc::Rc<super::PipelineFactory> {
        let orchestrator = Arc::new(super::preview_orchestrator::PreviewOrchestrator::new(
            repo.clone(),
        ));
        let preview_cache = Arc::clone(&orchestrator.cache);
        std::rc::Rc::new(super::PipelineFactory {
            repo,
            render_tx: Arc::new(OnceLock::new()),
            shared_items: Arc::new(Mutex::new(Vec::new())),
            shortcut_table: Arc::new(Mutex::new(std::collections::HashMap::new())),
            preview_cache,
            orchestrator,
            stashed_warnings: Arc::new(Mutex::new(Vec::new())),
            grid_slot: Arc::new(super::prs::GridSlot::new()),
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

    /// A [`PickerCollector`] for the removal / `invoke` tests, wrapping the given
    /// `items` and `repo`. Shares the factory's `stashed_warnings` so a test can
    /// assert on warnings the collector stashes.
    fn test_collector(
        items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
        repo: worktrunk::git::Repository,
    ) -> PickerCollector {
        let factory = test_factory(repo.clone());
        let stashed_warnings = Arc::clone(&factory.stashed_warnings);
        PickerCollector {
            items,
            repo,
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
            factory,
            stashed_warnings,
        }
    }

    /// Two detached worktrees both render the branch label `(detached)`, but
    /// each row's `output()` token carries its unique path. alt-x on the
    /// second row must remove exactly that worktree — not the first detached
    /// one a branch-name match would resolve to — and drop only its row.
    #[test]
    fn test_invoke_removes_selected_detached_worktree_by_path_token() {
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
        let mut collector = test_collector(Arc::clone(&items), repo.clone());

        // skim's `reload(remove {})` hands invoke `remove <single-quoted token>`.
        let cmd = format!("remove '{second_output}'");
        let (_rx, _interrupt) = collector.invoke(&cmd, Arc::new(AtomicUsize::new(0)));

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
        let super::SpawnedPipeline {
            rx,
            handler,
            collect_handle,
            ..
        } = factory.spawn().unwrap();
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

    /// alt-x with nothing selectable under the cursor expands to `remove ''`;
    /// `invoke` must treat the empty token as a no-op and leave the list intact.
    #[test]
    fn test_invoke_empty_selection_is_noop() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let item = branch_only_picker_item("some-branch");
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let mut collector = test_collector(Arc::clone(&items), repo);
        let (_rx, _interrupt) = collector.invoke("remove ''", Arc::new(AtomicUsize::new(0)));
        assert_eq!(
            items.lock().unwrap().len(),
            1,
            "empty selection must not remove anything"
        );
    }

    /// alt-x on a target that fails validation (a branch with no worktree and no
    /// local ref) takes `invoke`'s error arm: it logs and leaves the list intact —
    /// no drop, no background work.
    #[test]
    fn test_invoke_leaves_list_intact_when_prepare_fails() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let item = branch_only_picker_item("real-row");
        let token = item.output().to_string();
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let mut collector = test_collector(Arc::clone(&items), repo);

        // `no-such-branch` parses as a branch target but has no worktree and no
        // local ref, so `prepare_removal` errors before anything is dropped.
        let (_rx, _interrupt) =
            collector.invoke("remove 'no-such-branch'", Arc::new(AtomicUsize::new(0)));

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
        // A live sender so the restore takes its reload + reposition path rather
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
        // The restore re-streams the list: a reload, then the cursor reposition.
        assert!(
            matches!(rx.try_recv(), Ok(skim::prelude::Event::Reload(_))),
            "restore queues a reload when the sender is live"
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

    /// End-to-end through `invoke`: `prepare_removal` passes (the worktree is
    /// clean and removable), but the background `do_removal` fails on an
    /// approved-yet-failing `pre-remove` hook. The row is dropped optimistically,
    /// then restored when the removal fails — the worktree is preserved and the
    /// list reflects that, instead of leaving a phantom-removed row.
    #[test]
    fn test_invoke_restores_row_when_removal_fails() {
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
        let mut collector = PickerCollector {
            factory: test_factory(repo.clone()),
            items: Arc::clone(&items),
            repo: repo.clone(),
            approvals: Arc::new(approvals),
            render_tx: Arc::new(OnceLock::new()),
            stashed_warnings: Arc::clone(&stashed),
        };

        let cmd = format!("remove '{token}'");
        let (_rx, _interrupt) = collector.invoke(&cmd, Arc::new(AtomicUsize::new(0)));

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

    /// `removal_target_still_present` observes reality: a worktree dir or a
    /// branch ref that's gone reads as removed; one still on disk / in the
    /// ref store reads as present (the restore trigger).
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
            &worktree_result(test.path().to_path_buf()) // exists
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

    /// alt-x on an unmerged branch-only row never drops it (no flicker): an
    /// unmerged branch with no worktree resolves to `BranchOnly` with no
    /// integration reason, so `removal_will_remove_target` predicts `SafeDelete`
    /// keeps it. Decided synchronously in `invoke` — no background removal — so the
    /// row stays and a one-time `kept … branch` hint is stashed. Driven end-to-end.
    #[test]
    fn test_invoke_keeps_unmerged_branch_only_row() {
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
        let mut collector = PickerCollector {
            factory: test_factory(repo.clone()),
            items: Arc::clone(&items),
            repo: repo.clone(),
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
            stashed_warnings: Arc::clone(&stashed),
        };

        let cmd = format!("remove '{token}'");
        let (_rx, _interrupt) = collector.invoke(&cmd, Arc::new(AtomicUsize::new(0)));

        // The keep path is synchronous (no background thread), so by the time
        // `invoke` returns the row is still present and the hint is stashed.
        let outputs: Vec<String> = items
            .lock()
            .unwrap()
            .iter()
            .map(|item| item.output().into_owned())
            .collect();
        assert_eq!(
            outputs,
            vec![token],
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
        let (_rx, _interrupt) = collector.invoke(&cmd, Arc::new(AtomicUsize::new(0)));
        assert_eq!(
            stashed.lock().unwrap().clone(),
            warnings,
            "repeated alt-x on the same kept row stashes the hint only once"
        );

        let branch_list = repo.run_command(&["branch", "--list", "unmerged"]).unwrap();
        assert!(!branch_list.is_empty(), "the unmerged branch is preserved");
    }

    // Note: skim's `as_any().downcast_ref::<WorktreeSkimItem>()` can fail at
    // runtime due to a TypeId mismatch between skim's reader thread and the main
    // compilation unit. The invoke() code path uses output() matching instead.
    // Full TUI tests require interactive skim — verified via tmux-cli during
    // development.
}
