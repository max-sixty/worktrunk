//! wt-bridge: Zellij plugin for worktrunk workspace management.
//!
//! # Role
//!
//! This plugin is the "inside zellij" component of the workspace integration.
//! It receives pipe messages from the CLI and manipulates tabs accordingly.
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
//!         --name wt           │      └─► go_to_tab_name() or
//!         "select|/path"      │          new_tab(name, cwd)
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
//! - `select|<name>|/path/to/worktree` - Focus existing tab or create new one
//!
//! # Tab Tracking
//!
//! Each worktree gets its own tab, named after the branch/worktree name.
//! The plugin tracks known tabs via TabUpdate events to determine whether
//! to create a new tab or focus an existing one.
//!
//! # Debugging
//!
//! The plugin uses `eprintln!` for logging. To see output:
//!
//! 1. Run zellij with logging: `ZELLIJ_LOG=debug zellij ...`
//! 2. Check `/var/folders/.../zellij.log` for `[wt-bridge]` messages
//!
//! # Testing
//!
//! Manual testing inside zellij:
//!
//! ```bash
//! # After entering workspace with `wt ui`:
//! zellij pipe --name wt -- "select|feature|/path/to/feature"
//!
//! # Expected: New tab "feature" opens with cwd /path/to/feature
//! ```

use std::collections::{BTreeMap, HashSet};
use zellij_tile::prelude::*;

/// The pipe message name we listen for.
const PIPE_NAME: &str = "wt";

/// Plugin state.
#[derive(Default)]
struct WtBridge {
    /// Set of known tab names (from TabUpdate events).
    known_tabs: HashSet<String>,

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
            PermissionType::OpenTerminalsOrPlugins,
        ]);

        // Subscribe to events
        subscribe(&[EventType::TabUpdate, EventType::PermissionRequestResult]);
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name != PIPE_NAME {
            return false;
        }

        // Don't process commands until we have permission
        if !self.has_permission {
            eprintln!("[wt-bridge] Ignoring pipe message - waiting for permissions");
            return false;
        }

        let payload = match &pipe_message.payload {
            Some(p) => p.as_str(),
            None => {
                eprintln!("[wt-bridge] Received pipe message with no payload");
                return false;
            }
        };

        // Parse: "action|name|path"
        let parts: Vec<&str> = payload.splitn(3, '|').collect();
        if parts.len() < 2 {
            eprintln!("[wt-bridge] Invalid message format: {}", payload);
            return false;
        }

        match parts[0] {
            "select" => {
                let name = parts[1];
                let cwd = parts.get(2).copied();
                self.handle_select(name, cwd);
            }
            _ => eprintln!("[wt-bridge] Unknown action: {}", parts[0]),
        }

        false
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
                // Update our set of known tab names
                self.known_tabs.clear();
                for tab in &tabs {
                    self.known_tabs.insert(tab.name.clone());
                }
                eprintln!("[wt-bridge] TabUpdate: known_tabs={:?}", self.known_tabs);
            }
            _ => {}
        }

        false
    }
}

impl WtBridge {
    /// Handle the "select" action: focus existing tab or create new one.
    fn handle_select(&mut self, name: &str, cwd: Option<&str>) {
        eprintln!(
            "[wt-bridge] select: name={:?}, known_tabs={:?}",
            name, self.known_tabs
        );

        // Check if tab already exists
        if self.known_tabs.contains(name) {
            eprintln!("[wt-bridge] Focusing existing tab: {}", name);
            go_to_tab_name(name);
            return;
        }

        // Create new tab with the given name and cwd
        eprintln!("[wt-bridge] Creating new tab: {} (cwd: {:?})", name, cwd);
        new_tab(Some(name), cwd);
    }
}
