#![cfg(all(feature = "plugin", target_arch = "wasm32"))]

//! wt-bridge: Zellij plugin for worktrunk workspace management.
//!
//! # Purpose
//!
//! Routes tab focus requests from the CLI to the correct tab by index, even when
//! multiple tabs share the same display name (e.g., two repos both have `main`).
//!
//! # Architecture
//!
//! The plugin maintains a mapping from worktree paths to tab indices. The CLI and
//! plugin communicate via bidirectional pipe messages:
//!
//! ```text
//! CLI                                Plugin
//!  │                                   │
//!  │ "select|main|/path/to/wt"         │
//!  │──────────────────────────────────►│ lookup path_to_tab["/path/to/wt"]
//!  │                                   │
//!  │◄──────────────────────────────────│ "focused" (if found, after go_to_tab)
//!  │  OR                               │
//!  │◄──────────────────────────────────│ "not_found:main·d5e3" (unique name to use)
//!  │                                   │
//!  │ [CLI creates tab with zellij action new-tab --name "main·d5e3" --cwd /path]
//!  │                                   │
//!  │ "register|main·d5e3|/path/to/wt"  │
//!  │──────────────────────────────────►│ add to tracking
//! ```
//!
//! # Protocol
//!
//! **Request: `sync|{path}`**
//! - Register the currently active tab with the given path
//! - If path already tracked: no-op, respond "synced"
//! - Otherwise: track active tab with this path, respond "synced"
//! - Use case: call before `select` to ensure current tab is tracked
//!
//! **Request: `select|{display_name}|{path}`**
//! - If path is tracked: respond "focused:{N}" where N is the 1-based tab index
//! - If not tracked: respond "not_found:{unique_name}" where unique_name may have
//!   a hash suffix if the display_name collides with existing tabs
//!
//! **Request: `register|{tab_name}|{path}`**
//! - Add path to tracking with the given tab name
//! - Respond "registered"
//!
//! # Loading
//!
//! ```kdl
//! load_plugins {
//!     "file:~/.config/zellij/plugins/wt-bridge.wasm"
//! }
//! ```

use std::collections::BTreeMap;
use wt_bridge::{Host, PipeSourceId, PluginEvent, TabInfo, WtBridgePlugin};
use zellij_tile::prelude::*;

/// Zellij host implementation - calls actual zellij APIs.
struct ZellijHost;

impl Host for ZellijHost {
    fn cli_pipe_output(&mut self, pipe_id: &str, msg: &str) {
        cli_pipe_output(pipe_id, msg);
    }

    fn unblock_cli_pipe_input(&mut self, pipe_id: &str) {
        unblock_cli_pipe_input(pipe_id);
    }
}

/// Plugin state - wraps the testable plugin with zellij-specific render.
struct WtBridge {
    plugin: WtBridgePlugin<ZellijHost>,
}

impl Default for WtBridge {
    fn default() -> Self {
        Self {
            plugin: WtBridgePlugin::new(ZellijHost),
        }
    }
}

register_plugin!(WtBridge);

impl ZellijPlugin for WtBridge {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::ReadCliPipes,
        ]);

        subscribe(&[EventType::TabUpdate, EventType::PermissionRequestResult]);
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        println!("wt-bridge");
        println!("  tracked paths: {}", self.plugin.core.path_to_tab.len());
        println!("  tabs: {}", self.plugin.core.tabs.len());

        // Show actual tabs state for debugging
        if !self.plugin.core.tabs.is_empty() {
            println!();
            println!("Tabs:");
            for (i, tab) in self.plugin.core.tabs.iter().enumerate() {
                let active_marker = if tab.active { " *" } else { "" };
                println!(
                    "  [{}] {} (pos={}){}",
                    i, tab.name, tab.position, active_marker
                );
            }
        }

        if !self.plugin.core.path_to_tab.is_empty() {
            println!();
            println!("Tracked:");
            for (path, entry) in &self.plugin.core.path_to_tab {
                println!("  {} -> tab {} ({})", path, entry.index, entry.name);
            }
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let source = match &pipe_message.source {
            PipeSource::Cli(id) => PipeSourceId::Cli(id.clone()),
            _ => PipeSourceId::Plugin,
        };

        self.plugin
            .handle_pipe(source, &pipe_message.name, pipe_message.payload)
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.plugin.handle_event(PluginEvent::PermissionGranted);
            }
            Event::TabUpdate(tabs) => {
                let tab_infos: Vec<TabInfo> = tabs
                    .iter()
                    .map(|t| TabInfo {
                        name: t.name.clone(),
                        active: t.active,
                        position: t.position,
                    })
                    .collect();
                self.plugin
                    .handle_event(PluginEvent::TabsUpdated(tab_infos));
            }
            _ => {}
        }
        false
    }
}
