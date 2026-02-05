//! Interactive branch/worktree selector.
//!
//! A skim-based TUI for selecting and switching between worktrees.

mod items;
mod log_formatter;
mod pager;
mod preview;

use std::io::{IsTerminal, stderr};
use std::sync::Arc;

use anyhow::Context;
use crossterm::{execute, terminal};
use skim::prelude::*;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;

use super::list::collect;
use super::worktree::{execute_switch, plan_switch};
use crate::output::handle_switch_output;

use items::{HeaderSkimItem, WorktreeSkimItem};
use preview::{PreviewLayout, PreviewState};

pub fn handle_select(
    show_branches: bool,
    show_remotes: bool,
    config: &UserConfig,
) -> anyhow::Result<()> {
    // Interactive picker requires a terminal for the TUI
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("Interactive picker requires an interactive terminal");
    }

    let repo = Repository::current()?;

    // Initialize preview mode state file (auto-cleanup on drop)
    let state = PreviewState::new();

    // Gather list data using simplified collection (buffered mode)
    // Skip expensive operations not needed for select UI
    let skip_tasks = [
        collect::TaskKind::BranchDiff,
        collect::TaskKind::CiStatus,
        collect::TaskKind::MergeTreeConflicts,
    ]
    .into_iter()
    .collect();

    // Use 500ms timeout for git commands to show TUI faster on large repos.
    // Typical slow operations: merge-tree ~400-1800ms, rev-list ~200-600ms.
    // 500ms allows most operations to complete while cutting off tail latency.
    // Operations that timeout fail silently (data not shown), but TUI stays responsive.
    let command_timeout = Some(std::time::Duration::from_millis(500));

    let Some(list_data) = collect::collect(
        &repo,
        show_branches,
        show_remotes,
        &skip_tasks,
        false, // show_progress (no progress bars)
        false, // render_table (select renders its own UI)
        config,
        command_timeout,
        true, // skip_expensive_for_stale (faster for repos with many stale branches)
    )?
    else {
        return Ok(());
    };

    // Use the same layout system as `wt list` for proper column alignment
    // List width depends on preview position:
    // - Right layout: skim splits ~50% for list, ~50% for preview
    // - Down layout: list gets full width, preview is below
    let terminal_width = crate::display::get_terminal_width();
    let skim_list_width = match state.initial_layout {
        PreviewLayout::Right => terminal_width / 2,
        PreviewLayout::Down => terminal_width,
    };
    let layout = super::list::layout::calculate_layout_with_width(
        &list_data.items,
        &skip_tasks,
        skim_list_width,
        &list_data.main_worktree_path,
        None, // URL column not shown in select
    );

    // Render header using layout system (need both plain and styled text for skim)
    let header_line = layout.render_header_line();
    let header_display_text = header_line.render();
    let header_plain_text = header_line.plain_text();

    // Convert to skim items using the layout system for rendering
    let mut items: Vec<Arc<dyn SkimItem>> = list_data
        .items
        .into_iter()
        .map(|item| {
            let branch_name = item.branch_name().to_string();

            // Use layout system to render the line - this handles all column alignment
            let rendered_line = layout.render_list_item_line(&item);
            let display_text_with_ansi = rendered_line.render();
            let display_text = rendered_line.plain_text();

            Arc::new(WorktreeSkimItem {
                display_text,
                display_text_with_ansi,
                branch_name,
                item: Arc::new(item),
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
    let state_path_str = shlex::try_quote(&state_path_display)
        .map(|s| s.into_owned())
        .unwrap_or(state_path_display);

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
            // Mode switching (1/2/3/4 keys change preview content)
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
            // Create new worktree with query as branch name (alt-c for "create")
            "alt-c:accept(create)".to_string(),
            // Preview toggle (alt-p shows/hides preview)
            // Note: skim doesn't support change-preview-window like fzf, only toggle
            "alt-p:toggle-preview".to_string(),
            // Preview scrolling (half-page based on terminal height)
            format!("ctrl-u:preview-up({half_page})"),
            format!("ctrl-d:preview-down({half_page})"),
        ])
        // Legend/controls moved to preview window tabs (render_preview_tabs)
        .no_clear(true) // Prevent skim from clearing screen, we'll do it manually
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build skim options: {}", e))?;

    // Create item receiver
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items {
        tx.send(item)
            .map_err(|e| anyhow::anyhow!("Failed to send item to skim: {}", e))?;
    }
    drop(tx);

    // Run skim
    let output = Skim::run_with(&options, Some(rx));

    // Handle selection
    if let Some(out) = output
        && !out.is_abort
    {
        // Determine if user wants to create a new worktree (alt-n) or switch to existing (enter)
        let create_new =
            matches!(out.final_event, Event::EvActAccept(Some(ref label)) if label == "create");

        // Get branch name: from query if creating new, from selected item if switching
        let (identifier, should_create) = if create_new {
            let query = out.query.trim().to_string();
            if query.is_empty() {
                anyhow::bail!("Cannot create worktree: no branch name entered");
            }
            (query, true)
        } else {
            // Enter pressed: skim accept always includes a selection (abort handled above)
            let selected = out
                .selected_items
                .first()
                .expect("skim accept has selection");
            (selected.output().to_string(), false)
        };

        // Load config
        let config = UserConfig::load().context("Failed to load config")?;
        let repo = Repository::current().context("Failed to switch worktree")?;

        // Switch to existing worktree or create new one
        let plan = plan_switch(&repo, &identifier, should_create, None, false, &config)?;
        let (result, branch_info) = execute_switch(&repo, plan, &config, false, true)?;

        // Clear the terminal screen after skim exits to prevent artifacts
        // Use stderr for terminal control - stdout is reserved for data output
        execute!(stderr(), terminal::Clear(terminal::ClearType::All))?;
        execute!(stderr(), crossterm::cursor::MoveTo(0, 0))?;

        // Show success message; emit cd directive if shell integration is active
        handle_switch_output(&result, &branch_info)?;
    }

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::preview::{PreviewLayout, PreviewMode, PreviewStateData};
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

        // Cleanup
        let _ = fs::remove_file(&state_path);
    }

    #[test]
    fn test_preview_layout() {
        // Right is always 50%
        assert_eq!(PreviewLayout::Right.to_preview_window_spec(10), "right:50%");

        // Down calculates based on item count
        let spec = PreviewLayout::Down.to_preview_window_spec(5);
        assert!(spec.starts_with("down:"));
    }
}
