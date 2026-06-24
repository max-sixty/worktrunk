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
//! 2. `PreviewState::new` — auto-detects Right vs Down layout.
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
use std::time::{Duration, Instant};

use ansi_str::AnsiStr;
use anyhow::Context;
// bounded/unbounded/Sender are re-exported by skim::prelude
use skim::prelude::*;
use skim::reader::CommandCollector;
use skim::tui::event::ActionCallback;
use worktrunk::HookType;
use worktrunk::config::Approvals;
use worktrunk::git::{Repository, current_or_recover};
use worktrunk::styling::eprintln;

use super::hook_plan::{ApprovedHookPlan, HookPlanBuilder};
use super::hooks::HookAnnouncer;
use super::list::collect;
use super::list::progressive::RenderTarget;
use super::repository_ext::{RemoveTarget, RepositoryCliExt};
use super::worktree::{RemoveResult, SwitchPipeline};
use crate::cli::SwitchFormat;
use crate::output::{BackgroundFallbackMode, handle_remove_output};
use worktrunk::git::{BranchDeletionMode, delete_branch_if_safe};

use items::{PreviewCache, WORKTREE_OUTPUT_PREFIX};
use preview::{PreviewLayout, PreviewMode, PreviewState};
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

/// The alt-r removal target parsed back out of a row's `output()` token.
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
/// When alt-r is pressed, skim's `reload(remove {})` action expands `{}` to the
/// selected row's output() token and invokes this collector with it. The
/// collector parses the token, removes that item from the list, and streams the
/// remaining items back to skim — all without leaving the picker.
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
    /// `OnceLock` the progressive handler pushes `Event::Render` through). alt-r
    /// removals inject a [`reposition_cursor_action`] through it to restore the
    /// cursor after the reload. `None` until the TUI is up — but a reload can
    /// only fire after skim is showing rows, so it's always set by then.
    render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
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
    /// and the TUI stays responsive. A removal failure is logged; the item stays
    /// gone from the picker — a tradeoff until we can show in-progress state.
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
                    if let Ok(snapshot) = repo.capture_refs() {
                        let _ = delete_branch_if_safe(
                            repo,
                            &snapshot,
                            branch_name,
                            target,
                            deletion_mode.is_force(),
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

/// Pull the selected row's `output()` token out of the `remove <token>` reload
/// command skim builds for alt-r. skim expands `{}` to `output()` and shell-
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

/// The `item_list` row the cursor should land on after an alt-r removal, given
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
fn reposition_cursor_action(
    target: usize,
    attempts: Arc<AtomicUsize>,
    settled_streak: Arc<AtomicUsize>,
) -> Action {
    Action::Custom(ActionCallback::new_sync(
        move |app| -> Result<Vec<Event>, Box<dyn std::error::Error + Send + Sync>> {
            // Rows are in: land the cursor on the removed row's slot.
            if app.item_list.count() > 0 {
                app.item_list.jump_to_first();
                app.item_list
                    .scroll_by(i32::try_from(target).unwrap_or(i32::MAX));
                return Ok(Vec::new());
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

impl CommandCollector for PickerCollector {
    fn invoke(
        &mut self,
        cmd: &str,
        components_to_stop: Arc<AtomicUsize>,
    ) -> (SkimItemReceiver, Sender<i32>) {
        // skim's `reload(remove {})` expands `{}` to the selected row's
        // shell-quoted output() token; pull it back out (see
        // `parse_reload_remove_token`). No signal file — that raced the reader.
        {
            let selected_output = parse_reload_remove_token(cmd);
            if let Some(removal_target) = PickerRemovalTarget::from_signal(&selected_output) {
                let preparation = self.prepare_removal(&removal_target);

                match preparation {
                    Ok((planning_repo, result)) => {
                        // Removal validated — remove the selected item from the
                        // picker list. The `output()` token is unique per row
                        // (a `worktree-path:` path for worktrees), so this
                        // drops exactly the selected row even when several
                        // detached rows share the `(detached)` branch label.
                        //
                        // Note: skim's `as_any().downcast_ref::<WorktreeSkimItem>()` can
                        // fail at runtime (TypeId mismatch between reader thread and main
                        // thread compilation units). All item lookups use output()
                        // matching instead.
                        //
                        // Capture the removed row's position before dropping it so
                        // the cursor can be restored to that slot (the row that
                        // slides up) after the reload — see `reposition_cursor_action`.
                        let reposition_target = {
                            let mut items = self.items.lock().unwrap();
                            let removed_pos = items
                                .iter()
                                .position(|item| item.output().as_ref() == selected_output);
                            items.retain(|item| item.output().as_ref() != selected_output);
                            let remaining_data_rows =
                                items.len().saturating_sub(PICKER_HEADER_ROWS);
                            removed_pos
                                .and_then(|pos| sticky_reposition_target(pos, remaining_data_rows))
                        };

                        // If removing the current worktree, cd to home so skim and git
                        // commands continue to work after the directory disappears.
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

                        // Defer actual git removal to a background thread so skim's
                        // event loop stays responsive.
                        let repo = planning_repo.clone();
                        let approvals = Arc::clone(&self.approvals);
                        let _ = std::thread::Builder::new()
                            .name(format!("picker-remove-{selected_output}"))
                            .spawn(move || {
                                if let Err(e) = Self::do_removal(&repo, &result, &approvals) {
                                    log::warn!(
                                        "picker: failed to remove '{selected_output}': {e:#}"
                                    );
                                }
                            });

                        // Restore the cursor near the removed row. skim resets it
                        // to the top on every reload; inject a Custom action that
                        // moves it back to the removed row's slot once the reloaded
                        // rows land (`render_tx` is skim's event sender, set once
                        // the TUI is up — always present by the time a reload fires).
                        if let Some(target) = reposition_target
                            && let Some(event_tx) = self.render_tx.get()
                        {
                            let _ = event_tx.try_send(Event::Action(reposition_cursor_action(
                                target,
                                Arc::new(AtomicUsize::new(0)),
                                Arc::new(AtomicUsize::new(0)),
                            )));
                        }
                    }
                    Err(e) => {
                        log::info!("picker: cannot remove '{selected_output}': {e:#}");
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
        // The reader's collect_item thread handles its own components_to_stop accounting;
        // we just need a valid Sender to satisfy the trait signature.
        let _ = components_to_stop;
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

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();
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
        let dims = state.initial_layout.preview_dimensions(0);
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

    // List width depends on the preview position. Right splits the terminal
    // ~50/50; Down gives the list the full width. Passed to `collect` so
    // the skeleton layout matches the picker's actual render width.
    // The picker requires a TTY, so detection essentially always succeeds;
    // the unlimited-width fallback just keeps the math total. Skim
    // prefixes every line with a 2-column cursor gutter ("> "), so rows that
    // use the full width would otherwise spill into its ".." truncation.
    let terminal_width = crate::display::terminal_width().unwrap_or(usize::MAX);
    let skim_list_width = match state.initial_layout {
        PreviewLayout::Right => terminal_width / 2,
        PreviewLayout::Down => terminal_width,
    }
    .saturating_sub(2);

    // Estimate item count for the preview window spec (only the Down
    // layout depends on it). The Down layout caps visible rows at
    // `max_visible_items(available)`; every row past that cap is a no-op
    // for the height computation, so we short-circuit once the estimate
    // reaches it.
    let num_items_estimate = {
        let cap = {
            let term_height = terminal_size::terminal_size()
                .map(|(_, terminal_size::Height(h))| h as usize)
                .unwrap_or(24);
            preview::max_visible_items(preview::available_height(term_height))
        };
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
    let preview_window_spec = state
        .initial_layout
        .to_preview_window_spec(num_items_estimate);
    let preview_dims = state.initial_layout.preview_dimensions(num_items_estimate);

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
    // by `PickerCollector` on alt-r reload. Starts empty — the collector's
    // `invoke` only fires after skim has displayed items, by which time
    // the handler has already published them.
    let shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>> = Arc::new(Mutex::new(Vec::new()));

    // Approvals snapshot for the session: alt-r removals consult it read-only
    // to filter the hook plan; see `approved_removal_plan`.
    let approvals = Arc::new(Approvals::load().context("Failed to load approvals")?);

    // skim 4.x repaints on demand, so the collect handler needs a handle to
    // skim's event loop to surface in-place row updates. The picker fills this
    // once `Skim::init_tui` has run (inside `run_skim`); until then the handler
    // simply skips its render pokes. The alt-r collector shares it too, to inject
    // the cursor-reposition action after a removal. See `progressive_handler`
    // module docstring.
    let render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>> = Arc::new(OnceLock::new());

    let collector = PickerCollector {
        items: Arc::clone(&shared_items),
        repo: repo.clone(),
        approvals,
        render_tx: Arc::clone(&render_tx),
    };

    // Get state path for key bindings (shell-escaped for safety)
    let state_path_display = state.path.display().to_string();
    let state_path_str = shell_escape::unix::escape(state_path_display.into()).into_owned();

    // Half-page preview scroll: half of skim's usable height.
    let half_page = terminal_size::terminal_size()
        .map(|(_, terminal_size::Height(h))| (preview::available_height(h as usize) / 2).max(5))
        .unwrap_or(10);

    // Configure skim options with Rust-based preview and mode switching keybindings
    let options = SkimOptionsBuilder::default()
        .height("90%".to_string())
        .reverse(true)
        // Fill the whole selected row with the `current` background (set via
        // `current_bg` in `.color(...)` below). skim 4.x applies the current-row
        // style at the line level only when this is on; without it the selection
        // shows just the `>` pointer (the row's own `display()` ANSI spans carry
        // no background). skim 0.20's tuikit backend highlighted the row for free.
        .highlight_line(true)
        // Draw a scrollbar thumb on the item list when it overflows the view.
        // skim's `▐` default is the clap `default_value`, gated on skim's `cli`
        // feature; with `default-features = false` the library `Default` for
        // this `String` field is empty, which skim reads as "no scrollbar".
        // Setting it explicitly restores the thumb — without it a long worktree
        // (or `--prs`) list scrolls with no position cue, made worse by
        // `no_info(true)` below hiding the matched/total counter.
        .scrollbar("▐".to_string())
        // First line (header) non-selectable. `PICKER_HEADER_ROWS` mirrors this
        // count so the alt-r cursor-reposition math stays in sync — keep them one.
        .header_lines(PICKER_HEADER_ROWS)
        .multi(false)
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
            // Preview-tab switching. Bare digits 1-7 are intentionally NOT
            // bound — they flow to the query input so a number can be typed
            // (a PR number, or digits within a branch name). Two ways to
            // switch tabs remain:
            //   * alt-1..alt-7 jump straight to a tab. skim 4.x parses
            //     `alt-<digit>` natively via crossterm; an unparsable bind is
            //     just logged and dropped.
            //   * tab / shift-tab cycle forward / backward (below).
            format!(
                "alt-1:execute-silent(echo 1 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-2:execute-silent(echo 2 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-3:execute-silent(echo 3 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-4:execute-silent(echo 4 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-5:execute-silent(echo 5 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-6:execute-silent(echo 6 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "alt-7:execute-silent(echo 7 > {0})+refresh-preview",
                state_path_str
            ),
            // Cycle tabs with tab / shift-tab. The state file holds the current
            // digit; `tr` rotates it (1→2→…→7→1 forward, the reverse for btab)
            // with wraparound, via a temp file + rename so the read and write
            // don't race on one path. Two hard constraints shape this:
            //   * Paren-free — skim 4.x parses an `execute-silent(…)` body by
            //     splitting at the first `(` and trimming the trailing `)`, and
            //     splits the action chain on `+`. So the body must contain no
            //     `+` and must not end in `)`; `$(...)` / `$(( ))` would do both.
            //     Keeping it paren-free entirely (the embedded `{0}` temp path
            //     is — `std::env::temp_dir()` paths have none) satisfies this,
            //     which the alt-r/alt-N bindings share.
            //   * Shell-agnostic — skim runs it under the user's $SHELL
            //     (fish/zsh/sh), so no shell-specific syntax: `tr` + `mv` are
            //     external and behave identically everywhere.
            // This overrides skim's default Tab (toggle-select + cursor down)
            // and Shift-Tab (toggle-select + cursor up); `bind` replaces the
            // chain wholesale, and both are inert in this single-select picker.
            //
            // Shift-Tab needs three bindings, not one. skim parses `btab` to
            // `KeyEvent{BackTab, NONE}`, but crossterm delivers Shift-Tab with
            // the SHIFT modifier set, so that lone bind never matches and skim's
            // built-in Shift-Tab (cursor up) wins — a back-cycle that silently
            // moved the selection instead. skim's own default keymap hedges the
            // same ambiguity across `BackTab+SHIFT`, `Tab+SHIFT`, and
            // `BackTab+all()`; we bind every representation crossterm might
            // report (`btab` / `shift-btab` / `shift-tab`) so the override holds
            // regardless of terminal. (Plain Tab is unambiguous — `Tab+NONE`.)
            format!(
                "tab:execute-silent(tr 1234567 2345671 < {0} > {0}.tmp; mv {0}.tmp {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "btab:execute-silent(tr 1234567 7123456 < {0} > {0}.tmp; mv {0}.tmp {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "shift-btab:execute-silent(tr 1234567 7123456 < {0} > {0}.tmp; mv {0}.tmp {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "shift-tab:execute-silent(tr 1234567 7123456 < {0} > {0}.tmp; mv {0}.tmp {0})+refresh-preview",
                state_path_str
            ),
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
            "alt-r:reload(remove {})".to_string(),
            // Preview toggle (alt-p shows/hides preview)
            // Note: skim doesn't support change-preview-window like fzf, only toggle
            "alt-p:toggle-preview".to_string(),
            // Preview scrolling (half-page based on terminal height)
            format!("ctrl-u:preview-up({half_page})"),
            format!("ctrl-d:preview-down({half_page})"),
        ])
        // Legend/controls moved to preview window tabs (render_preview_tabs)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build skim options: {}", e))?;
    worktrunk::trace::instant("Picker skim options built");

    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();

    // Shared between the bg-thread handler (which pushes warnings while
    // skim owns the terminal) and the main thread (which drains them after
    // `Skim::run_with` returns and stderr is safe again).
    let stashed_warnings: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Column-geometry handoff: the collect thread fills it at skeleton time,
    // the `--prs` thread reads it to align PR rows with the worktree rows.
    let grid_slot = Arc::new(prs::GridSlot::new());

    // `--prs` loading flag, shared between the header (shows a "loading…"
    // marker while true) and the `--prs` thread (clears it when the forge call
    // resolves). `Some` exactly when that thread spawns below, so the marker
    // tracks its lifetime.
    let prs_loading: Option<Arc<AtomicBool>> =
        (show_prs && !is_preview_bench).then(|| Arc::new(AtomicBool::new(true)));

    // Concrete type so the dry-run dump can read the handler's rendered rows.
    let handler: Arc<progressive_handler::PickerHandler> =
        Arc::new(progressive_handler::PickerHandler {
            tx: tx.clone(),
            render_tx: Arc::clone(&render_tx),
            last_render_poke: Mutex::new(Instant::now()),
            shared_items: Arc::clone(&shared_items),
            rendered_slots: std::sync::OnceLock::new(),
            pr_status_slots: std::sync::OnceLock::new(),
            preview_cache: Arc::clone(&preview_cache),
            orchestrator: Arc::clone(&orchestrator),
            preview_dims,
            llm_command,
            summary_hint,
            stashed_warnings: Arc::clone(&stashed_warnings),
            deferred_items: std::sync::OnceLock::new(),
            grid_slot: Arc::clone(&grid_slot),
            prs_loading: prs_loading.clone(),
        });

    // Spawn collect on a background thread. The handler holds the only
    // remaining `tx` clone; when the bg thread exits, `tx` drops, skim's reader
    // sees EOF, and the picker goes idle. Contract: background work done →
    // picker idle.
    let bg_handler: Arc<dyn collect::PickerProgressHandler> = handler.clone();
    let bg_repo = repo.clone();
    let bg_skip_tasks = skip_tasks.clone();
    let bg_handle = std::thread::Builder::new()
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
    worktrunk::trace::instant("Picker collect spawned");

    // PR/MR streaming (`--prs`). One forge call on its own thread that holds
    // another `tx` clone, so the picker frame paints from local worktree data
    // immediately and PR rows stream in (~1s) when the call returns. The
    // clone defers EOF: skim's reader sees end-of-stream only once both this
    // thread and the collect thread drop their senders. The dry-run runs it
    // (joined below) so the fetch/render path is exercised headlessly — the only
    // way it gets coverage, since the interactive picker's skim-abort exit never
    // flushes a profile. Only the preview-bench skips it: that path measures the
    // preview workload and must not reach the network.
    let prs_handle = if let Some(prs_loading) = prs_loading.clone() {
        let prs_tx = tx.clone();
        let prs_repo = repo.clone();
        let prs_warnings = Arc::clone(&stashed_warnings);
        let prs_grid = Arc::clone(&grid_slot);
        let prs_orchestrator = Arc::clone(&orchestrator);
        let prs_render_tx = Arc::clone(&render_tx);
        Some(
            std::thread::Builder::new()
                .name("picker-prs".into())
                .spawn(move || {
                    prs::stream_open_prs(
                        &prs_repo,
                        &prs::PrsLayout {
                            list_width: skim_list_width,
                            preview_dims,
                        },
                        &prs_tx,
                        &prs_warnings,
                        &prs_grid,
                        &prs_orchestrator,
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

    // Drop main-thread copies so the bg threads' `tx` clones are the last
    // senders (their drop is what signals EOF to skim's reader). The dry run
    // keeps the handler: skim never runs there, so the EOF contract doesn't
    // apply, and the dump below reads its rendered rows.
    drop(tx);
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
        let _ = bg_handle.join();
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

    // Run skim (single invocation — alt-r uses reload, not re-launch).
    // Skim receives items as the bg thread's handler sends them, and the
    // handler pushes repaints through `render_tx` (filled inside `run_skim`)
    // as it mutates rows in place.
    //
    // Don't join `bg_handle` after skim exits: drain may still be running
    // network tasks, and joining would block exit for up to DRAIN_TIMEOUT
    // (120s). Process exit terminates the bg thread; its git subprocesses
    // are read-only.
    let output = run_skim(options, rx, &render_tx, &state.path);
    drop(bg_handle);
    // Same rationale as `bg_handle`: don't join — the forge call may still be
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

    // Handle selection (signal file cleaned up by PreviewState::Drop)
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

/// Background poller that nudges skim to re-run its preview when the
/// preview-mode state file changes. See [`run_skim`] for why; it lives only for
/// one picker session and stops when [`ModeWatcher::stop`] is called.
struct ModeWatcher {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ModeWatcher {
    fn spawn(event_tx: tokio::sync::mpsc::Sender<Event>, path: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("picker-mode-watcher".into())
            .spawn(move || {
                // Seed with the current contents so the initial mode isn't
                // treated as a change.
                let mut last = std::fs::read(&path).ok();
                while !stop_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                    let current = std::fs::read(&path).ok();
                    if current != last {
                        last = current;
                        // Channel closed (skim exited) just makes this a no-op;
                        // the loop then ends on the next `stop` check.
                        let _ = event_tx.try_send(Event::RunPreview);
                    }
                }
            })
            .ok();
        Self { stop, handle }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Run skim to completion, exposing its event sender for progressive repaints.
///
/// This inlines what `Skim::run_with` does, with two additions: after the TUI is
/// initialized we publish `Skim::event_sender()` into `render_tx`, and we start
/// a [`ModeWatcher`]. skim 4.x renders on demand, so the background collect
/// thread's in-place row mutations stay invisible until something wakes the
/// event loop — the handler pushes `Event::Render` through that sender (see
/// `progressive_handler`).
///
/// `preview_mode_path` is the file the alt-1…5 / tab keybinds rewrite to switch
/// preview tabs; `ModeWatcher` re-runs the preview once the write lands, fixing
/// the one-step tab lag skim 0.20's heartbeat used to mask.
///
/// `wt` runs no outer tokio runtime, so skim's event loop runs on a fresh
/// multi-thread `Runtime` — the same one `run_with` builds in that case. A user
/// cancel is `Ok(SkimOutput)` with `is_abort` set; only a genuine init /
/// event-loop failure is an `Err`.
///
/// Injecting `Event::Render` / `Event::RunPreview` from the handler and the
/// watcher is safe against clobbering the recorded selection: skim's `Accept` /
/// `Abort` set `should_quit` in the same `tick` that records them as
/// `final_event`, and `run()` breaks before the next `tick`, so a trailing
/// injected event is never processed after the terminal action.
fn run_skim(
    options: SkimOptions,
    rx: SkimItemReceiver,
    render_tx: &Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    preview_mode_path: &Path,
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
        let event_tx = skim.event_sender();
        let _ = render_tx.set(event_tx.clone());

        // Build the runtime before spawning the watcher: it's the only fallible
        // step here, and ordering it first keeps the infallible spawn paired
        // with the unconditional `watcher.stop()` below (no early return can
        // leak the watcher thread).
        let runtime =
            tokio::runtime::Runtime::new().context("failed to start picker event-loop runtime")?;
        let watcher = ModeWatcher::spawn(event_tx, preview_mode_path.to_path_buf());
        let result = runtime.block_on(async {
            skim.enter().await?;
            skim.run().await
        });
        watcher.stop();
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
    use super::items::WorktreeSkimItem;
    use super::preview::{PreviewLayout, PreviewMode, PreviewStateData};
    use super::{
        PickerAction, PickerCollector, PickerRemovalTarget, drain_stashed_warnings,
        parse_reload_remove_token, picker_item_identifier, resolve_identifier,
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
    fn test_preview_state_data_roundtrip() {
        let state_path = PreviewStateData::state_path();

        // Write and read back various modes
        let _ = fs::write(&state_path, "1");
        assert_eq!(PreviewStateData::read_mode(), PreviewMode::WorkingTree);

        let _ = fs::write(&state_path, "2");
        assert_eq!(PreviewStateData::read_mode(), PreviewMode::Log);

        let _ = fs::write(&state_path, "3");
        assert_eq!(PreviewStateData::read_mode(), PreviewMode::BranchDiff);

        let _ = fs::write(&state_path, "4");
        assert_eq!(PreviewStateData::read_mode(), PreviewMode::UpstreamDiff);

        let _ = fs::write(&state_path, "5");
        assert_eq!(PreviewStateData::read_mode(), PreviewMode::Summary);

        // Cleanup
        let _ = fs::remove_file(&state_path);
    }

    #[test]
    fn test_preview_layout() {
        // Right uses absolute width derived from terminal size
        let spec = PreviewLayout::Right.to_preview_window_spec(10);
        assert!(spec.starts_with("right:"));

        // Down calculates based on item count
        let spec = PreviewLayout::Down.to_preview_window_spec(5);
        assert!(spec.starts_with("down:"));
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

        let collector = PickerCollector {
            items: Arc::new(Mutex::new(Vec::new())),
            repo,
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
        };

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

        let collector = PickerCollector {
            items: Arc::new(Mutex::new(Vec::new())),
            repo,
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
        };

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

    /// Two detached worktrees both render the branch label `(detached)`, but
    /// each row's `output()` token carries its unique path. alt-r on the
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
        let mut collector = PickerCollector {
            items: Arc::clone(&items),
            repo: repo.clone(),
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
        };

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

    /// alt-r with nothing selectable under the cursor expands to `remove ''`;
    /// `invoke` must treat the empty token as a no-op and leave the list intact.
    #[test]
    fn test_invoke_empty_selection_is_noop() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.path()).unwrap();
        let item = branch_only_picker_item("some-branch");
        let items = Arc::new(Mutex::new(vec![Arc::clone(&item)]));
        let mut collector = PickerCollector {
            items: Arc::clone(&items),
            repo,
            approvals: Arc::new(Approvals::default()),
            render_tx: Arc::new(OnceLock::new()),
        };
        let (_rx, _interrupt) = collector.invoke("remove ''", Arc::new(AtomicUsize::new(0)));
        assert_eq!(
            items.lock().unwrap().len(),
            1,
            "empty selection must not remove anything"
        );
    }

    // Note: skim's `as_any().downcast_ref::<WorktreeSkimItem>()` can fail at
    // runtime due to a TypeId mismatch between skim's reader thread and the main
    // compilation unit. The invoke() code path uses output() matching instead.
    // Full TUI tests require interactive skim — verified via tmux-cli during
    // development.
}
