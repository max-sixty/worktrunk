//! wt-bridge: Zellij plugin for worktrunk permission caching.
//!
//! # Role
//!
//! This plugin's primary purpose is to cache zellij permissions. When loaded via
//! `load_plugins` in config.kdl, zellij prompts the user to grant permissions once
//! per session. This avoids repeated permission dialogs.
//!
//! Tab management is handled by the CLI using direct `zellij action` commands,
//! which don't require plugin involvement.
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
//! # Debugging
//!
//! The plugin uses `eprintln!` for logging. To see output:
//!
//! 1. Run zellij with logging: `ZELLIJ_LOG=debug zellij ...`
//! 2. Check `/var/folders/.../zellij.log` for `[wt-bridge]` messages

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// Plugin state.
#[derive(Default)]
struct WtBridge {
    /// Whether we have received permission to operate.
    has_permission: bool,
}

register_plugin!(WtBridge);

impl ZellijPlugin for WtBridge {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // Request permissions to ensure they're cached for the session
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::OpenTerminalsOrPlugins,
        ]);

        // Subscribe to permission result
        subscribe(&[EventType::PermissionRequestResult]);
    }

    fn update(&mut self, event: Event) -> bool {
        if let Event::PermissionRequestResult(PermissionStatus::Granted) = event {
            self.has_permission = true;
            eprintln!("[wt-bridge] Permissions granted");
        }
        false
    }
}
