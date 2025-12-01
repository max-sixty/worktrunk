//! wt-bridge: Zellij plugin for worktrunk workspace management.
//!
//! # Role
//!
//! This plugin is the "inside zellij" component of the workspace integration.
//! It receives pipe messages from the CLI and manipulates panes accordingly.
//!
//! # How It Fits Together
//!
//! ```text
//! Outside Zellij              │  Inside Zellij (this plugin)
//! ────────────────────────────┼────────────────────────────────────
//! wt ui                       │
//!   └─► zellij attach/create  │
//!                             │
//! wt switch feature           │
//!   └─► zellij pipe ──────────┼──► pipe() receives "select|/path"
//!         --name wt           │      └─► focus_terminal_pane() or
//!         "select|/path"      │          open_terminal("/path")
//! ```
//!
//! # Loading
//!
//! The plugin loads via `load_plugins` in `~/.config/zellij/config.kdl`:
//!
//! ```kdl
//! load_plugins {
//!     "file:~/.config/zellij/plugins/wt-bridge.wasm"
//! }
//! ```
//!
//! Zellij shows a permission dialog on first session start. After granting,
//! the plugin runs silently in the background (no visible pane).
//!
//! # Message Protocol
//!
//! Messages arrive via `zellij pipe --name wt -- "<action>|<data>"`:
//!
//! - `select|/path/to/worktree` - Focus existing seat or create new one
//!
//! # Seat Tracking
//!
//! The plugin maintains `seats: HashMap<PathBuf, u32>` mapping worktree paths
//! to pane IDs. When `open_terminal()` is called, zellij auto-focuses the new
//! pane. The next `PaneUpdate` event captures the focused pane's ID.
//!
//! # Debugging
//!
//! The plugin uses `eprintln!` for logging. To see output:
//!
//! 1. Run zellij with logging: `ZELLIJ_LOG=debug zellij ...`
//! 2. Check `/tmp/zellij-*/zellij.log` for `[wt-bridge]` messages
//!
//! # Known Limitations
//!
//! - **Race condition**: User switching focus between `open_terminal()` and
//!   `PaneUpdate` may cause the wrong pane ID to be captured.
//! - **Single pending seat**: Rapid `select` messages may overwrite pending
//!   tracking state.
//!
//! # Testing
//!
//! Manual testing inside zellij:
//!
//! ```bash
//! # After entering workspace with `wt ui`:
//! zellij pipe --name wt -- "select|/tmp"
//!
//! # Expected: New pane opens in /tmp (first time) or focuses (subsequent)
//! ```
//!
//! Check logs if nothing happens:
//! - `[wt-bridge] Permissions granted` should appear after dialog
//! - `[wt-bridge] Creating new seat for "/tmp"` on select
//! - `[wt-bridge] Tracking seat: "/tmp" -> pane N` on PaneUpdate

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use zellij_tile::prelude::*;

/// The pipe message name we listen for.
const PIPE_NAME: &str = "wt";

/// Plugin state.
#[derive(Default)]
struct WtBridge {
    /// Mapping from worktree path to pane ID.
    seats: HashMap<PathBuf, u32>,

    /// Worktree path waiting to be associated with a pane ID.
    /// Set when we call `open_terminal()`, cleared when we receive `PaneUpdate`.
    pending_seat: Option<PathBuf>,

    /// Current tab position, needed to find focused pane.
    current_tab: usize,

    /// Whether we have received permission to operate.
    has_permission: bool,
}

register_plugin!(WtBridge);

impl ZellijPlugin for WtBridge {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // Request all permissions we need
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
        ]);

        // Subscribe to events including permission result
        subscribe(&[
            EventType::PaneClosed,
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::PermissionRequestResult,
        ]);
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name != PIPE_NAME {
            return false;
        }

        // Don't process commands until we have permission
        if !self.has_permission {
            eprintln!("[wt-bridge] Ignoring pipe message - waiting for permissions");
            return true;
        }

        let payload = match &pipe_message.payload {
            Some(p) => p.as_str(),
            None => {
                eprintln!("[wt-bridge] Received pipe message with no payload");
                return true;
            }
        };

        let (action, data) = match payload.split_once('|') {
            Some(pair) => pair,
            None => {
                eprintln!("[wt-bridge] Invalid message format: {}", payload);
                return true;
            }
        };

        match action {
            "select" => self.handle_select(data),
            _ => eprintln!("[wt-bridge] Unknown action: {}", action),
        }

        true
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(result) => {
                if result == PermissionStatus::Granted {
                    self.has_permission = true;
                    eprintln!("[wt-bridge] Permissions granted");
                }
            }
            Event::TabUpdate(tabs) => {
                // Track current tab position for finding focused pane
                if let Some(tab) = get_focused_tab(&tabs) {
                    self.current_tab = tab.position;
                }
            }
            Event::PaneUpdate(manifest) => {
                // If we just opened a terminal, capture its pane ID
                if let Some(worktree_path) = self.pending_seat.take() {
                    match get_focused_pane(self.current_tab, &manifest) {
                        Some(pane) if !pane.is_plugin => {
                            eprintln!(
                                "[wt-bridge] Tracking seat: {:?} -> pane {}",
                                worktree_path, pane.id
                            );
                            self.seats.insert(worktree_path, pane.id);
                        }
                        Some(_) => {
                            eprintln!(
                                "[wt-bridge] Focused pane is a plugin, can't track seat for {:?}",
                                worktree_path
                            );
                        }
                        None => {
                            eprintln!(
                                "[wt-bridge] No focused pane found, can't track seat for {:?}",
                                worktree_path
                            );
                        }
                    }
                }
            }
            Event::PaneClosed(PaneId::Terminal(terminal_id)) => {
                self.seats.retain(|path, &mut id| {
                    if id == terminal_id {
                        eprintln!("[wt-bridge] Seat closed: {:?}", path);
                        false
                    } else {
                        true
                    }
                });
            }
            _ => {}
        }

        false
    }

    // No render() needed - plugin runs in the background without a pane.
    // Zellij handles the permission dialog automatically via load_plugins.
}

impl WtBridge {
    /// Handle the "select" action: focus or create a seat for the given worktree.
    fn handle_select(&mut self, worktree_path: &str) {
        let path = PathBuf::from(worktree_path);

        // Focus existing seat if we have one
        if let Some(&pane_id) = self.seats.get(&path) {
            eprintln!(
                "[wt-bridge] Focusing existing seat for {:?} -> pane {}",
                path, pane_id
            );
            focus_terminal_pane(pane_id, false);
            return;
        }

        // Create new terminal pane for this worktree
        eprintln!("[wt-bridge] Creating new seat for {:?}", path);
        open_terminal(worktree_path);

        // Mark this path as pending - we'll capture the pane ID on next PaneUpdate
        if let Some(old) = self.pending_seat.replace(path) {
            eprintln!(
                "[wt-bridge] Overwriting pending seat {:?} (rapid requests)",
                old
            );
        }
    }
}
