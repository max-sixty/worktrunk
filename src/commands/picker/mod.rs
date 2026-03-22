//! Interactive branch/worktree selector.
//!
//! A skim-based TUI for selecting and switching between worktrees.

mod items;
mod log_formatter;
mod pager;
mod preview;
mod summary;

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use dashmap::DashMap;
use skim::prelude::*;
use worktrunk::git::{Repository, current_or_recover};

use super::handle_switch::{
    approve_switch_hooks, run_pre_switch_hooks, spawn_switch_background_hooks, switch_extra_vars,
};
use super::list::collect;
use super::worktree::{
    SwitchBranchInfo, SwitchResult, execute_switch, handle_remove,
    offer_bare_repo_worktree_path_fix, path_mismatch, plan_switch,
};
use crate::output::{handle_remove_output, handle_switch_output};

use items::{HeaderSkimItem, PreviewCache, WorktreeSkimItem};
use preview::{PreviewLayout, PreviewMode, PreviewState};

/// Action selected by the user in the picker.
enum PickerAction {
    /// Switch to the selected worktree (Enter key).
    Switch,
    /// Create a new worktree from the search query (alt-c).
    Create,
    /// Remove the selected worktree (alt-r for "remove").
    Remove,
}

pub fn handle_picker(
    cli_branches: bool,
    cli_remotes: bool,
    change_dir_flag: Option<bool>,
) -> anyhow::Result<()> {
    // Interactive picker requires a terminal for the TUI
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("Interactive picker requires an interactive terminal");
    }

    let (repo, is_recovered) = current_or_recover()?;

    // Merge CLI flags with resolved config (project-specific config is now available)
    let config = repo.config();
    let change_dir = change_dir_flag.unwrap_or_else(|| !config.switch.no_cd());
    let show_branches = cli_branches || config.list.branches();
    let show_remotes = cli_remotes || config.list.remotes();

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();

    // Gather list data using simplified collection (buffered mode)
    // Skip expensive operations not needed for picker UI
    let skip_tasks: std::collections::HashSet<collect::TaskKind> = [
        collect::TaskKind::BranchDiff,
        collect::TaskKind::CiStatus,
        collect::TaskKind::MergeTreeConflicts,
    ]
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
        true,  // skip_expensive_for_stale (faster for repos with many stale branches)
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

    // Create shared cache for all preview modes (pre-computed in background)
    let preview_cache: PreviewCache = Arc::new(DashMap::new());

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
            // ("·") for all items — it signals "data not available" rather than the
            // ellipsis ("⋯") which implies data is still loading.
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
            // Remove selected worktree (alt-r for "remove")
            "alt-r:accept(remove)".to_string(),
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

    // Create item receiver
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items {
        tx.send(item)
            .map_err(|e| anyhow::anyhow!("Failed to send item to skim: {}", e))?;
    }
    drop(tx);

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

    for item in &items_for_precompute {
        for mode in modes {
            let cache = Arc::clone(&preview_cache);
            let item = Arc::clone(item);
            rayon::spawn(move || {
                let cache_key = (item.branch_name().to_string(), mode);
                cache.entry(cache_key).or_insert_with(|| {
                    WorktreeSkimItem::compute_preview(&item, mode, preview_width, preview_height)
                });
            });
        }
    }

    // Queue summary generation after tabs 1-4 so git previews get rayon priority.
    if config.list.summary() && config.commit_generation.is_configured() {
        let llm_command = config.commit_generation.command.clone().unwrap();
        for item in &items_for_precompute {
            let item = Arc::clone(item);
            let cache = Arc::clone(&preview_cache);
            let cmd = llm_command.clone();
            let repo = repo.clone();
            rayon::spawn(move || {
                summary::generate_and_cache_summary(&item, &cmd, &cache, &repo);
            });
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

    // Run skim
    let output = Skim::run_with(&options, Some(rx));

    // Handle selection
    if let Some(out) = output
        && !out.is_abort
    {
        // Determine action: create (alt-c), remove (alt-r), or switch (enter)
        let action = match &out.final_event {
            Event::EvActAccept(Some(label)) if label == "create" => PickerAction::Create,
            Event::EvActAccept(Some(label)) if label == "remove" => PickerAction::Remove,
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

        match action {
            PickerAction::Remove => {
                // Get the selected worktree's branch name
                let selected_name = out
                    .selected_items
                    .first()
                    .map(|item| item.output().to_string());
                let branch_name = resolve_identifier(&action, String::new(), selected_name)?;

                let config = repo.user_config();

                // Safe removal: no force-delete (-D), no force-worktree (-f)
                let result = handle_remove(
                    &branch_name,
                    false, // keep_branch: delete branch (default behavior)
                    false, // force_delete: no -D
                    false, // force_worktree: no -f
                    config,
                )
                .context("Failed to remove worktree")?;

                // Execute removal in foreground, no hooks, not quiet
                handle_remove_output(&result, true, false, false)?;
            }
            PickerAction::Create | PickerAction::Switch => {
                let should_create = matches!(action, PickerAction::Create);

                // Get branch name: from query if creating new, from selected item if switching
                let selected_name = out
                    .selected_items
                    .first()
                    .map(|item| item.output().to_string());
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
                let mut config =
                    worktrunk::config::UserConfig::load().context("Failed to load config")?;
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
                let (result, branch_info) =
                    execute_switch(&repo, plan, &config, false, hooks_approved)?;

                // Compute path mismatch lazily (deferred from plan_switch for existing worktrees)
                let branch_info = match &result {
                    SwitchResult::Existing { path } | SwitchResult::AlreadyAt(path) => {
                        let expected_path =
                            path_mismatch(&repo, &branch_info.branch, path, &config);
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
                let hooks_display_path = handle_switch_output(
                    &result,
                    &branch_info,
                    change_dir,
                    Some(&source_root),
                    &cwd,
                )?;

                // Spawn background hooks after success message
                if hooks_approved {
                    let extra_vars = switch_extra_vars(&result);
                    spawn_switch_background_hooks(
                        &repo,
                        &config,
                        &result,
                        &branch_info.branch,
                        false,
                        &extra_vars,
                        hooks_display_path.as_deref(),
                    )?;
                }
            }
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
        PickerAction::Remove => match selected_name {
            Some(name) => Ok(name),
            None => {
                anyhow::bail!(
                    "No worktree selected — type a name that matches an existing worktree"
                );
            }
        },
    }
}

#[cfg(test)]
pub mod tests {
    use super::preview::{PreviewLayout, PreviewMode, PreviewStateData};
    use super::{PickerAction, resolve_identifier};
    use std::fs;

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

        // Remove returns the selected name
        let result = resolve_identifier(&PickerAction::Remove, String::new(), Some("main".into()));
        assert_eq!(result.unwrap(), "main");

        // Remove with no selection is an error
        let result = resolve_identifier(&PickerAction::Remove, String::new(), None);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No worktree selected")
        );
    }
}
