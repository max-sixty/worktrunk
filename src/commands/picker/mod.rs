//! Interactive branch/worktree selector.
//!
//! A skim-based TUI for selecting and switching between worktrees.

mod items;
mod log_formatter;
mod pager;
mod preview;
mod preview_orchestrator;
mod summary;

use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
// bounded/unbounded/Sender are re-exported by skim::prelude
use skim::prelude::*;
use skim::reader::CommandCollector;
use worktrunk::git::{Repository, current_or_recover};

use super::command_executor::FailureStrategy;
use super::handle_switch::{
    approve_switch_hooks, run_pre_switch_hooks, spawn_switch_background_hooks, switch_extra_vars,
};
use super::hooks::{execute_hook, prepare_background_hooks, spawn_hook_pipeline};
use super::list::collect;
use super::repository_ext::{RemoveTarget, RepositoryCliExt};
use super::worktree::hooks::PostRemoveContext;
use super::worktree::{
    RemoveResult, SwitchBranchInfo, SwitchResult, execute_switch,
    offer_bare_repo_worktree_path_fix, path_mismatch, plan_switch,
};
use crate::commands::command_executor::CommandContext;
use crate::output::handle_switch_output;
use worktrunk::git::{
    BranchDeletionMode, RemoveOptions, delete_branch_if_safe, remove_worktree_with_cleanup,
};

use items::{HeaderSkimItem, PreviewCache, WorktreeSkimItem};
use preview::{PreviewLayout, PreviewMode, PreviewState};
use preview_orchestrator::PreviewOrchestrator;

/// Action selected by the user in the picker.
enum PickerAction {
    /// Switch to the selected worktree (Enter key).
    Switch,
    /// Create a new worktree from the search query (alt-c).
    Create,
}

/// Custom command collector for skim's `reload` action.
///
/// When alt-r is pressed, skim runs `execute-silent` to write the selected branch
/// name to a signal file, then `reload` invokes this collector. The collector reads
/// the signal file, removes the item from the list, and streams the remaining items
/// back to skim — all without leaving the picker.
///
/// Git operations (worktree removal, branch deletion) are deferred to a background
/// thread because skim 0.20 calls `invoke()` on the main event loop thread.
/// Blocking it freezes the TUI.
///
/// Cursor position resets to the first item after reload (skim 0.20 limitation,
/// tracked in #1695).
struct PickerCollector {
    items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    signal_path: PathBuf,
    repo: Repository,
}

impl PickerCollector {
    /// Execute removal in background: pre-remove hooks + worktree + branch + post-remove hooks.
    ///
    /// Called from a background thread after the picker optimistically removes the item
    /// from the list. The entire operation runs off skim's event loop so the TUI stays
    /// responsive. If pre-remove hooks fail, the removal is aborted (but the item is
    /// already gone from the picker — a tradeoff until we can show in-progress state).
    ///
    /// `repo` is only used for `BranchOnly` deletion. `RemovedWorktree` constructs
    /// its own from `main_path` (which may differ from the picker's startup repo in
    /// bare-repo setups).
    fn do_removal(repo: &Repository, result: &RemoveResult) -> anyhow::Result<()> {
        match result {
            RemoveResult::RemovedWorktree {
                main_path,
                worktree_path,
                branch_name,
                deletion_mode,
                target_branch,
                force_worktree,
                removed_commit,
                ..
            } => {
                let repo = Repository::at(main_path)?;
                let config = repo.user_config();
                let hook_branch = branch_name.as_deref().unwrap_or("HEAD");

                // Run pre-remove hooks (synchronously in this background thread).
                // Non-zero exit aborts the removal, matching `wt remove` semantics.
                let target_ref = repo
                    .worktree_at(main_path)
                    .branch()
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let target_path_str = worktrunk::path::to_posix_path(&main_path.to_string_lossy());
                let extra_vars: Vec<(&str, &str)> = vec![
                    ("target", &target_ref),
                    ("target_worktree_path", &target_path_str),
                ];
                let pre_ctx =
                    CommandContext::new(&repo, config, Some(hook_branch), worktree_path, false);
                execute_hook(
                    &pre_ctx,
                    worktrunk::HookType::PreRemove,
                    &extra_vars,
                    FailureStrategy::FailFast,
                    &[],
                    None, // no display path in TUI context
                )?;

                let output = remove_worktree_with_cleanup(
                    &repo,
                    worktree_path,
                    RemoveOptions {
                        branch: branch_name.clone(),
                        deletion_mode: *deletion_mode,
                        target_branch: target_branch.clone(),
                        force_worktree: *force_worktree,
                    },
                )?;
                if let Some(staged) = output.staged_path {
                    let _ = std::fs::remove_dir_all(&staged);
                }

                // Spawn post-remove hooks in background (log to files, no terminal output).
                let post_ctx =
                    CommandContext::new(&repo, config, Some(hook_branch), main_path, false);
                let remove_vars = PostRemoveContext::new(
                    worktree_path,
                    removed_commit.as_deref(),
                    main_path,
                    &repo,
                );
                let extra_vars = remove_vars.extra_vars(hook_branch);
                for steps in prepare_background_hooks(
                    &post_ctx,
                    worktrunk::HookType::PostRemove,
                    &extra_vars,
                    None, // no display path in TUI context
                )? {
                    spawn_hook_pipeline(&post_ctx, steps)?;
                }
            }
            RemoveResult::BranchOnly {
                branch_name,
                deletion_mode,
                ..
            } => {
                if !deletion_mode.should_keep() {
                    let default_branch = repo.default_branch();
                    let target = default_branch.as_deref().unwrap_or("HEAD");
                    let _ =
                        delete_branch_if_safe(repo, branch_name, target, deletion_mode.is_force());
                }
            }
        }
        Ok(())
    }
}

impl CommandCollector for PickerCollector {
    fn invoke(
        &mut self,
        _cmd: &str,
        components_to_stop: Arc<AtomicUsize>,
    ) -> (SkimItemReceiver, Sender<i32>) {
        // Read the removal signal (item output text written by execute-silent)
        if let Ok(signal) = std::fs::read_to_string(&self.signal_path) {
            let selected_output = signal.trim().to_string();
            if !selected_output.is_empty() {
                // Validate removal before touching the list. prepare_worktree_removal
                // runs a few git commands (~15-20ms) — acceptable on skim's event loop.
                // Only remove the item and spawn background deletion if this succeeds.
                let caller_path = self.repo.current_worktree().root().ok();
                let config = self.repo.user_config();

                // Resolve removal target by path when possible (handles both
                // branched and detached worktrees). Branch-only items won't
                // match any worktree path, so they fall through to Branch.
                let worktree_path = self.repo.list_worktrees().ok().and_then(|wts| {
                    // Match by branch first, then fall back to detached (branch: None).
                    let by_branch = wts
                        .iter()
                        .find(|wt| wt.branch.as_deref() == Some(selected_output.as_str()));
                    let matched = by_branch.or_else(|| wts.iter().find(|wt| wt.branch.is_none()));
                    matched.map(|wt| wt.path.clone())
                });
                let target = match &worktree_path {
                    Some(path) => RemoveTarget::Path(path),
                    None => RemoveTarget::Branch(&selected_output),
                };

                let preparation = self.repo.prepare_worktree_removal(
                    target,
                    BranchDeletionMode::SafeDelete,
                    false,
                    config,
                    caller_path,
                    None,
                );

                match preparation {
                    Ok(result) => {
                        // Removal validated — remove item from the picker list.
                        //
                        // Note: skim's `as_any().downcast_ref::<WorktreeSkimItem>()` fails
                        // at runtime (TypeId mismatch between reader thread and main thread
                        // compilation units in skim 0.20). All item lookups use output()
                        // matching instead.
                        {
                            let mut items = self.items.lock().unwrap();
                            items.retain(|item| item.output().as_ref() != selected_output);
                        }

                        // If removing the current worktree, cd to home so skim and git
                        // commands continue to work after the directory disappears.
                        if matches!(
                            &result,
                            RemoveResult::RemovedWorktree {
                                changed_directory: true,
                                ..
                            }
                        ) && let Ok(home) = self.repo.home_path()
                        {
                            let _ = std::env::set_current_dir(&home);
                        }

                        // Defer actual git removal to a background thread so skim's
                        // event loop stays responsive.
                        let repo = self.repo.clone();
                        let _ = std::thread::Builder::new()
                            .name(format!("picker-remove-{selected_output}"))
                            .spawn(move || {
                                if let Err(e) = Self::do_removal(&repo, &result) {
                                    log::warn!(
                                        "picker: failed to remove '{selected_output}': {e:#}"
                                    );
                                }
                            });
                    }
                    Err(e) => {
                        log::info!("picker: cannot remove '{selected_output}': {e:#}");
                    }
                }

                // Clear signal for next removal
                let _ = std::fs::write(&self.signal_path, "");
            }
        }

        // Stream remaining items through a channel for skim to consume.
        // Uses unbounded channel so all items are sent immediately without blocking.
        let items = self.items.lock().unwrap();
        let (tx, rx) = unbounded();
        for item in items.iter() {
            let _ = tx.send(Arc::clone(item));
        }
        drop(tx);

        // Dummy interrupt channel — no subprocess to kill.
        // The reader's collect_item thread handles its own components_to_stop accounting;
        // we just need a valid Sender to satisfy the trait signature.
        let _ = components_to_stop;
        let (tx_interrupt, _rx_interrupt) = bounded(1);
        (rx, tx_interrupt)
    }
}

pub fn handle_picker(
    cli_branches: bool,
    cli_remotes: bool,
    change_dir_flag: Option<bool>,
) -> anyhow::Result<()> {
    // Interactive picker requires a terminal for the TUI. The dry-run path
    // bypasses skim entirely, so no TTY is required — useful for tests and
    // for diagnosing the pre-compute pipeline from scripts.
    if std::env::var_os("WORKTRUNK_PICKER_DRY_RUN").is_none() && !std::io::stdin().is_terminal() {
        anyhow::bail!("Interactive picker requires an interactive terminal");
    }

    let (repo, is_recovered) = current_or_recover()?;

    // Merge CLI flags with resolved config (project-specific config is now available)
    let config = repo.config();
    let change_dir = change_dir_flag.unwrap_or_else(|| config.switch.cd());
    let show_branches = cli_branches || config.list.branches();
    let show_remotes = cli_remotes || config.list.remotes();

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();

    // Preview cache + dedicated pool are created up-front so the speculative
    // first-item preview can run in parallel with `collect::collect` below.
    let orchestrator = PreviewOrchestrator::new();
    let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);

    // Speculative warm-up: the picker sorts the current worktree first, and
    // the default tab (WorkingTree = `git diff HEAD` in that worktree) is
    // what skim will render first. Kicking this off before `collect::collect`
    // overlaps preview compute with list collection (up to 500ms budget).
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

    // Gather list data using simplified collection (buffered mode)
    // Skip expensive operations not needed for picker UI
    let skip_tasks: std::collections::HashSet<collect::TaskKind> =
        [collect::TaskKind::BranchDiff, collect::TaskKind::CiStatus]
            .into_iter()
            .collect();

    // Per-task command timeout from shared [list] config.
    let command_timeout = config.list.task_timeout();

    // Wall-clock budget for the entire collect phase (default: 500ms).
    let collect_deadline = config.switch_picker.timeout().map(|d| Instant::now() + d);

    let Some(list_data) = collect::collect(
        &repo,
        collect::ShowConfig::Resolved {
            show_branches,
            show_remotes,
            skip_tasks: skip_tasks.clone(),
            command_timeout,
            collect_deadline,
        },
        false, // show_progress (no progress bars)
        false, // render_table (picker renders its own UI)
    )?
    else {
        return Ok(());
    };

    // Use the same layout system as `wt list` for proper column alignment
    // List width depends on preview position:
    // - Right layout: skim splits ~50% for list, ~50% for preview
    // - Down layout: list gets full width, preview is below
    let terminal_width = crate::display::terminal_width();
    let skim_list_width = match state.initial_layout {
        PreviewLayout::Right => terminal_width / 2,
        PreviewLayout::Down => terminal_width,
    };
    let layout = super::list::layout::calculate_layout_with_width(
        &list_data.items,
        &list_data.skip_tasks,
        skim_list_width,
        &list_data.main_worktree_path,
        None, // URL column not shown in picker
    );

    // Render header using layout system (need both plain and styled text for skim)
    let header_line = layout.render_header_line();
    let header_display_text = header_line.render();
    let header_plain_text = header_line.plain_text();

    // Convert to skim items using the layout system for rendering
    // Keep Arc<ListItem> refs for background pre-computation
    let mut items_for_precompute: Vec<Arc<super::list::model::ListItem>> = Vec::new();
    let mut items: Vec<Arc<dyn SkimItem>> = list_data
        .items
        .into_iter()
        .map(|item| {
            let branch_name = item.branch_name().to_string();

            // The picker doesn't update progressively, so any column whose data
            // didn't arrive in time won't fill in later. Use the stale placeholder
            // entry point; its glyph is the same `·` as the loading placeholder
            // today but the semantic split is preserved for a future re-divergence.
            let rendered_line = layout.render_list_item_stale(&item);
            let display_text_with_ansi = rendered_line.render();
            let display_text = rendered_line.plain_text();

            let item = Arc::new(item);
            items_for_precompute.push(Arc::clone(&item));

            Arc::new(WorktreeSkimItem {
                display_text,
                display_text_with_ansi,
                branch_name,
                item,
                preview_cache: Arc::clone(&preview_cache),
            }) as Arc<dyn SkimItem>
        })
        .collect();

    // Insert header row at the beginning (will be non-selectable via header_lines option)
    items.insert(
        0,
        Arc::new(HeaderSkimItem {
            display_text: header_plain_text,
            display_text_with_ansi: header_display_text,
        }) as Arc<dyn SkimItem>,
    );

    // Get state path for key bindings (shell-escaped for safety)
    let state_path_display = state.path.display().to_string();
    let state_path_str = shell_escape::escape(state_path_display.into()).into_owned();

    // Calculate half-page scroll: skim uses 90% of terminal height, half of that = 45%
    let half_page = terminal_size::terminal_size()
        .map(|(_, terminal_size::Height(h))| (h as usize * 45 / 100).max(5))
        .unwrap_or(10);

    // Calculate preview window spec based on auto-detected layout
    // items.len() - 1 because we added a header row
    let num_items = items.len().saturating_sub(1);
    let preview_window_spec = state.initial_layout.to_preview_window_spec(num_items);

    // Signal file for alt-r removal communication. execute-silent writes the branch
    // name here; the PickerCollector reads it on reload. Cleaned up in PreviewState::Drop.
    let signal_path = state.path.with_extension("remove");

    // Shared items list: the PickerCollector reads and modifies this on reload.
    let shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>> = Arc::new(Mutex::new(items));

    // Custom collector for skim's reload action — performs removal and streams
    // updated items back, all without leaving the picker.
    let collector = PickerCollector {
        items: Arc::clone(&shared_items),
        signal_path: signal_path.clone(),
        repo: repo.clone(),
    };

    let signal_path_escaped =
        shell_escape::escape(signal_path.display().to_string().into()).into_owned();

    // Configure skim options with Rust-based preview and mode switching keybindings
    let options = SkimOptionsBuilder::default()
        .height("90%".to_string())
        // Workaround for skim-tuikit bug: partial-height mode skips smcup but
        // cleanup still sends rmcup, leaving artifacts. no_clear_start forces
        // cursor_goto + erase_down cleanup instead. See skim-rs/skim#880.
        .no_clear_start(true)
        .layout("reverse".to_string())
        .header_lines(1) // Make first line (header) non-selectable
        .multi(false)
        .no_info(true) // Hide info line (matched/total counter)
        .preview(Some("".to_string())) // Enable preview (empty string means use SkimItem::preview())
        .preview_window(preview_window_spec)
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
        .color(Some(
            "fg:-1,bg:-1,header:-1,matched:108,current:237,current_bg:251,current_match:108"
                .to_string(),
        ))
        .cmd_collector(Rc::new(RefCell::new(collector)) as Rc<RefCell<dyn CommandCollector>>)
        .bind(vec![
            // Mode switching (1/2/3/4/5 keys change preview content)
            format!(
                "1:execute-silent(echo 1 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "2:execute-silent(echo 2 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "3:execute-silent(echo 3 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "4:execute-silent(echo 4 > {0})+refresh-preview",
                state_path_str
            ),
            format!(
                "5:execute-silent(echo 5 > {0})+refresh-preview",
                state_path_str
            ),
            // Create new worktree with query as branch name (alt-c for "create")
            "alt-c:accept(create)".to_string(),
            // Remove selected worktree: write branch name to signal file, then
            // reload triggers PickerCollector which performs the removal and
            // streams updated items back — all without leaving the picker.
            format!(
                "alt-r:execute-silent(echo {{}} > {0})+reload(remove)",
                signal_path_escaped
            ),
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

    // Send initial items to skim via channel
    let items = shared_items.lock().unwrap();
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items.iter() {
        tx.send(Arc::clone(item))
            .map_err(|e| anyhow::anyhow!("Failed to send item to skim: {}", e))?;
    }
    drop(tx);
    drop(items);

    // Pre-compute all preview modes for all worktrees in parallel via rayon.
    // Each (worktree, mode) pair is a separate rayon task, allowing the thread pool
    // to overlap I/O-bound git commands. Tasks are fire-and-forget — ongoing
    // git commands are harmless read-only operations even if skim exits early.
    let (preview_width, preview_height) = state.initial_layout.preview_dimensions(num_items);

    let modes = [
        PreviewMode::WorkingTree,
        PreviewMode::Log,
        PreviewMode::BranchDiff,
        PreviewMode::UpstreamDiff,
    ];

    // Spawn order (rayon dispatches FIFO):
    // 1. First item's modes — user lands here and may tab-cycle immediately.
    // 2. Mode-major for remaining items — the default tab (WorkingTree)
    //    warms across the full list before any off-default mode starts.
    let dims = (preview_width, preview_height);
    if let Some(first) = items_for_precompute.first() {
        for mode in modes {
            orchestrator.spawn_preview(Arc::clone(first), mode, dims);
        }
    }
    for mode in modes {
        for item in items_for_precompute.iter().skip(1) {
            orchestrator.spawn_preview(Arc::clone(item), mode, dims);
        }
    }

    // Summaries run last: each LLM call can take seconds, so queueing them
    // behind fast git previews keeps preview tabs warming promptly. First
    // item's summary still goes first within the summary batch so a user
    // who sits on the top entry gets a head start.
    if config.list.summary() && config.commit_generation.is_configured() {
        let llm_command = config.commit_generation.command.clone().unwrap();
        if let Some(first) = items_for_precompute.first() {
            orchestrator.spawn_summary(Arc::clone(first), llm_command.clone(), repo.clone());
        }
        for item in items_for_precompute.iter().skip(1) {
            orchestrator.spawn_summary(Arc::clone(item), llm_command.clone(), repo.clone());
        }
    } else {
        // No LLM configured or summaries disabled — insert config hint so the
        // tab shows a useful message instead of a perpetual "Generating..." placeholder.
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
        for item in &items_for_precompute {
            let branch = item.branch_name().to_string();
            preview_cache.insert((branch, PreviewMode::Summary), hint.to_string());
        }
    }

    // Dry-run: wait for all pre-compute tasks and dump the cache as JSON
    // instead of launching skim. Used by tests and for diagnosing
    // "previews never load" bugs without a TTY.
    if std::env::var_os("WORKTRUNK_PICKER_DRY_RUN").is_some() {
        orchestrator.wait_for_idle();
        println!("{}", orchestrator.dump_cache_json());
        return Ok(());
    }

    // Run skim (single invocation — alt-r uses reload, not re-launch)
    let output = Skim::run_with(&options, Some(rx));

    // Handle selection (signal file cleaned up by PreviewState::Drop)
    if let Some(out) = output
        && !out.is_abort
    {
        // Determine action: create (alt-c) or switch (enter)
        // Remove is handled inline via reload — it never reaches accept.
        let action = match &out.final_event {
            Event::EvActAccept(Some(label)) if label == "create" => PickerAction::Create,
            _ => PickerAction::Switch,
        };

        // --no-cd: just output the selected branch name and exit (read-only, no side effects)
        if !change_dir {
            let selected_name = out
                .selected_items
                .first()
                .map(|item| item.output().to_string());
            let query = out.query.trim().to_string();
            let identifier = resolve_identifier(&action, query, selected_name)?;
            println!("{identifier}");
            return Ok(());
        }

        let should_create = matches!(action, PickerAction::Create);

        // Get branch name: from query if creating new, from selected item if switching.
        // For detached worktrees, use the path (same as `wt switch /path` from CLI).
        let selected = out.selected_items.first();
        let selected_name = selected.map(|item| {
            if !should_create
                && let Some(data) = item
                    .as_any()
                    .downcast_ref::<WorktreeSkimItem>()
                    .and_then(|s| s.item.worktree_data())
                    .filter(|d| d.detached)
            {
                return data.path.to_string_lossy().into_owned();
            }
            item.output().to_string()
        });
        let query = out.query.trim().to_string();
        let identifier = resolve_identifier(&action, query, selected_name)?;

        // Load config — reuse recovered repo if we recovered earlier
        let repo = if is_recovered {
            repo.clone()
        } else {
            Repository::current().context("Failed to switch worktree")?
        };
        // Load config, offering bare repo worktree-path fix if needed.
        // Reload from disk so mutations are picked up by plan_switch.
        let mut config = worktrunk::config::UserConfig::load().context("Failed to load config")?;
        offer_bare_repo_worktree_path_fix(&repo, &mut config)?;

        // Run pre-switch hooks before branch resolution or worktree creation.
        // {{ branch }} receives the raw user input (before resolution).
        // Skip when recovered — the source worktree is gone, nothing to run hooks against.
        if !is_recovered {
            run_pre_switch_hooks(&repo, &config, &identifier, true)?;
        }

        // Switch to existing worktree or create new one
        let plan = plan_switch(&repo, &identifier, should_create, None, false, &config)?;
        let hooks_approved = approve_switch_hooks(&repo, &config, &plan, false, true)?;
        let (result, branch_info) = execute_switch(&repo, plan, &config, false, hooks_approved)?;

        // Compute path mismatch lazily (deferred from plan_switch for existing worktrees).
        // Skip for detached HEAD worktrees (branch is None).
        let branch_info = match &result {
            SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
                let expected_path = branch_info
                    .branch
                    .as_deref()
                    .and_then(|b| path_mismatch(&repo, b, path, &config));
                SwitchBranchInfo {
                    expected_path,
                    ..branch_info
                }
            }
            _ => branch_info,
        };

        // Show success message; emit cd directive if shell integration is active
        // When recovered from a deleted worktree, fall back to repo_path().
        let fallback_path = repo.repo_path()?.to_path_buf();
        let cwd = std::env::current_dir().unwrap_or(fallback_path.clone());
        let source_root = repo.current_worktree().root().unwrap_or(fallback_path);
        let hooks_display_path =
            handle_switch_output(&result, &branch_info, change_dir, Some(&source_root), &cwd)?;

        // Spawn background hooks after success message
        if hooks_approved {
            let extra_vars = switch_extra_vars(&result);
            spawn_switch_background_hooks(
                &repo,
                &config,
                &result,
                branch_info.branch.as_deref(),
                false,
                &extra_vars,
                hooks_display_path.as_deref(),
            )?;
        }
    }

    Ok(())
}

/// Resolve the branch identifier from picker output.
///
/// Extracted from the picker callback for testability. Used by both the
/// interactive path and the `--no-cd` print path.
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
    use super::preview::{PreviewLayout, PreviewMode, PreviewStateData};
    use super::{PickerAction, PickerCollector, resolve_identifier};
    use crate::commands::worktree::RemoveResult;
    use std::fs;
    use worktrunk::git::BranchDeletionMode;

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

    #[test]
    fn test_execute_removal_removes_worktree_and_branch() {
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
            integration_reason: None,
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        PickerCollector::do_removal(&repo, &result).unwrap();
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
        PickerCollector::do_removal(&repo, &result).unwrap();

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
        PickerCollector::do_removal(&repo, &result).unwrap();

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
            integration_reason: None,
            force_worktree: false,
            expected_path: None,
            removed_commit: None,
        };

        PickerCollector::do_removal(&repo, &result).unwrap();
        assert!(!wt_path.exists(), "detached worktree should be removed");
    }

    // Note: skim's `as_any().downcast_ref::<WorktreeSkimItem>()` fails at
    // runtime due to TypeId mismatch between skim's reader thread and the main
    // compilation unit (skim 0.20 bug). The invoke() code path uses output()
    // matching instead. Full invoke() tests require interactive skim — verified
    // via tmux-cli during development.
}
