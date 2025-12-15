//! wt-bridge core logic (testable without zellij).
//!
//! This module contains the state machine and protocol logic for the wt-bridge plugin.
//! The actual zellij integration is in main.rs which uses this as a library.

use std::collections::BTreeMap;

/// Grace period (in pipe calls) for newly registered entries before they can be
/// removed by TabUpdate reconciliation. This handles the race condition where
/// register is called but the tab hasn't appeared in TabUpdate yet.
pub const GRACE_PERIOD_PIPE_CALLS: usize = 5;

/// Entry tracking a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEntry {
    /// Current tab index (0-based position).
    pub index: usize,
    /// The tab name (may include hash suffix for uniqueness).
    pub name: String,
    /// When this entry was registered (pipe call count).
    pub registered_at_pipe_call: usize,
}

/// Minimal tab info for testing (mirrors zellij's TabInfo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub name: String,
    pub active: bool,
    pub position: usize,
}

/// Response from handling a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Successfully synced the current tab with a path.
    Synced,
    /// Successfully focused an existing tab.
    Focused { tab_index: u32 },
    /// Tab not found, create with this unique name.
    NotFound { unique_name: String },
    /// Successfully registered a new tab.
    Registered,
    /// Debug info response.
    Debug {
        tabs_len: usize,
        tracked: Vec<String>,
    },
    /// Error response.
    Error(String),
}

impl Response {
    /// Convert to protocol string.
    pub fn to_protocol(&self) -> String {
        match self {
            Response::Synced => "synced".to_string(),
            Response::Focused { tab_index } => format!("focused:{}", tab_index),
            Response::NotFound { unique_name } => format!("not_found:{}", unique_name),
            Response::Registered => "registered".to_string(),
            Response::Debug { tabs_len, tracked } => {
                format!("debug:tabs={},tracked={:?}", tabs_len, tracked)
            }
            Response::Error(msg) => msg.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Host abstraction for testable wrapper logic
// ─────────────────────────────────────────────────────────────────────────────

/// Host trait abstracting zellij APIs for testing.
pub trait Host {
    /// Send output to a CLI pipe.
    fn cli_pipe_output(&mut self, pipe_id: &str, msg: &str);

    /// Unblock a CLI pipe to signal completion.
    fn unblock_cli_pipe_input(&mut self, pipe_id: &str);
}

/// Pipe message source identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeSourceId {
    /// Message from CLI with pipe ID (string, may be numeric or UUID).
    Cli(String),
    /// Message from another plugin (ignored).
    Plugin,
}

/// Events that the plugin can receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    /// Permissions were granted.
    PermissionGranted,
    /// Tab list was updated.
    TabsUpdated(Vec<TabInfo>),
}

/// Plugin wrapper with permission handling and message queuing.
///
/// This wraps `WtBridgeCore` with the logic that lives in `main.rs`:
/// - Permission state tracking
/// - Message queuing before permissions granted
/// - Host API calls (go_to_tab, pipe output)
pub struct WtBridgePlugin<H: Host> {
    /// Core state machine logic.
    pub core: WtBridgeCore,
    /// Whether permissions have been granted.
    has_permission: bool,
    /// Messages queued before permissions were granted.
    queued_messages: Vec<(PipeSourceId, String)>,
    /// Host implementation for zellij API calls.
    pub host: H,
}

impl<H: Host> WtBridgePlugin<H> {
    /// Create a new plugin with the given host.
    pub fn new(host: H) -> Self {
        Self {
            core: WtBridgeCore::new(),
            has_permission: false,
            queued_messages: Vec::new(),
            host,
        }
    }

    /// Handle a pipe message.
    ///
    /// Returns `true` if the message was handled (name matched "wt").
    pub fn handle_pipe(
        &mut self,
        source: PipeSourceId,
        pipe_name: &str,
        payload: Option<String>,
    ) -> bool {
        self.core.increment_pipe_count();

        // Ignore messages for other pipe names
        if pipe_name != "wt" {
            return false;
        }

        let payload = match payload {
            Some(p) => p,
            None => {
                self.respond(&source, "error:no_payload");
                return true;
            }
        };

        if !self.has_permission {
            self.queued_messages.push((source, payload));
            return true;
        }

        self.handle_message(source, payload);
        true
    }

    /// Handle a plugin event.
    pub fn handle_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::PermissionGranted => {
                self.has_permission = true;
                let queued = std::mem::take(&mut self.queued_messages);
                for (source, payload) in queued {
                    self.handle_message(source, payload);
                }
            }
            PluginEvent::TabsUpdated(tabs) => {
                self.core.update_tabs(tabs);
            }
        }
    }

    /// Handle a message using the core logic.
    fn handle_message(&mut self, source: PipeSourceId, payload: String) {
        if let Some(response) = self.core.handle_message(&payload) {
            self.respond(&source, &response.to_protocol());
        }
    }

    /// Send a response back to the source.
    fn respond(&mut self, source: &PipeSourceId, msg: &str) {
        if let PipeSourceId::Cli(id) = source {
            self.host.cli_pipe_output(id, &format!("{msg}\n"));
            self.host.unblock_cli_pipe_input(id);
        }
    }
}

/// Core plugin state and logic.
#[derive(Debug)]
pub struct WtBridgeCore {
    /// Unique instance ID for debugging.
    pub instance_id: u64,

    /// Mapping from worktree path to tab entry.
    pub path_to_tab: BTreeMap<String, TabEntry>,

    /// Current tab list from TabUpdate events.
    pub tabs: Vec<TabInfo>,

    /// Counter to track pipe calls (for grace period calculation).
    pub pipe_call_count: usize,
}

impl Default for WtBridgeCore {
    fn default() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let instance_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            instance_id,
            path_to_tab: BTreeMap::new(),
            tabs: Vec::new(),
            pipe_call_count: 0,
        }
    }
}

impl WtBridgeCore {
    /// Create a new instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the pipe call counter (called on each pipe message).
    pub fn increment_pipe_count(&mut self) {
        self.pipe_call_count += 1;
    }

    /// Handle a protocol message and return the response.
    ///
    /// Returns `None` if the message is malformed or empty.
    pub fn handle_message(&mut self, payload: &str) -> Option<Response> {
        // Trim trailing whitespace (stdin-based payloads may have trailing newline)
        let payload = payload.trim_end();
        let parts: Vec<&str> = payload.splitn(3, '|').collect();
        if parts.is_empty() {
            return None;
        }

        Some(match parts[0] {
            "select" if parts.len() >= 3 => self.handle_select(parts[1], parts[2]),
            "register" if parts.len() >= 3 => self.handle_register(parts[1], parts[2]),
            "sync" if parts.len() >= 2 => self.handle_sync(parts[1]),
            "debug" => self.handle_debug(),
            _ => Response::Error(format!("error:unknown_action:{}", parts[0])),
        })
    }

    /// Handle select: focus existing tab or respond with name for creation.
    pub fn handle_select(&self, display_name: &str, path: &str) -> Response {
        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        eprintln!(
            "wt-bridge[{}]: select display_name={} path={} tracked={:?}",
            self.instance_id % 10000, // Short ID for readability
            display_name,
            path,
            self.path_to_tab.keys().collect::<Vec<_>>()
        );
        if let Some(entry) = self.path_to_tab.get(path) {
            #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
            eprintln!(
                "wt-bridge: found entry index={} name={} -> go_to_tab({})",
                entry.index,
                entry.name,
                entry.index as u32 + 1
            );
            // zellij uses 1-based tab indices for go_to_tab
            Response::Focused {
                tab_index: entry.index as u32 + 1,
            }
        } else {
            // Not found - generate unique name and tell CLI to create
            let unique_name = self.generate_unique_name(display_name, path);
            Response::NotFound { unique_name }
        }
    }

    /// Handle register: add a newly created tab to tracking.
    pub fn handle_register(&mut self, tab_name: &str, path: &str) -> Response {
        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        eprintln!(
            "wt-bridge[{}]: register tab_name={} path={} tabs={:?}",
            self.instance_id % 10000,
            tab_name,
            path,
            self.tabs.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        // Find tab by name and use its position
        if let Some(tab) = self.tabs.iter().find(|t| t.name == tab_name) {
            #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
            eprintln!(
                "wt-bridge: register found tab, using position={}",
                tab.position
            );
            self.path_to_tab.insert(
                path.to_string(),
                TabEntry {
                    index: tab.position, // Use tab.position for go_to_tab API
                    name: tab.name.clone(),
                    registered_at_pipe_call: self.pipe_call_count,
                },
            );
            #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
            eprintln!(
                "wt-bridge: register done, tracked={:?}",
                self.path_to_tab.keys().collect::<Vec<_>>()
            );
            Response::Registered
        } else {
            #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
            eprintln!(
                "wt-bridge: register tab not found, using estimated position={}",
                self.tabs.len()
            );
            // Tab not found yet - store with estimated position
            // (will be corrected on next TabUpdate)
            self.path_to_tab.insert(
                path.to_string(),
                TabEntry {
                    index: self.tabs.len(),
                    name: tab_name.to_string(),
                    registered_at_pipe_call: self.pipe_call_count,
                },
            );
            #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
            eprintln!(
                "wt-bridge: register done, tracked={:?}",
                self.path_to_tab.keys().collect::<Vec<_>>()
            );
            Response::Registered
        }
    }

    /// Handle sync: register the currently active tab with the given path.
    pub fn handle_sync(&mut self, path: &str) -> Response {
        // Already tracked? No-op.
        if self.path_to_tab.contains_key(path) {
            return Response::Synced;
        }

        // Find the active tab
        let active_tab = self.tabs.iter().find(|t| t.active);

        if let Some(tab) = active_tab {
            self.path_to_tab.insert(
                path.to_string(),
                TabEntry {
                    index: tab.position, // Use tab.position for go_to_tab API
                    name: tab.name.clone(),
                    registered_at_pipe_call: self.pipe_call_count,
                },
            );
            Response::Synced
        } else {
            // No active tab found - include diagnostic info
            Response::Error(format!("error:no_active_tab:tabs_len={}", self.tabs.len()))
        }
    }

    /// Handle debug: return current state.
    pub fn handle_debug(&self) -> Response {
        Response::Debug {
            tabs_len: self.tabs.len(),
            tracked: self.path_to_tab.keys().cloned().collect(),
        }
    }

    /// Generate a unique tab name, adding a hash suffix if needed.
    pub fn generate_unique_name(&self, display_name: &str, path: &str) -> String {
        let name_in_use = self.tabs.iter().any(|t| t.name == display_name)
            || self.path_to_tab.values().any(|e| e.name == display_name);

        if name_in_use {
            let hash = short_hash(path);
            format!("{}·{}", display_name, hash)
        } else {
            display_name.to_string()
        }
    }

    /// Update tabs from a TabUpdate event.
    pub fn update_tabs(&mut self, new_tabs: Vec<TabInfo>) {
        self.reconcile_tabs(new_tabs);
    }

    /// Reconcile our tab mapping with updated tab info.
    ///
    /// Matching strategy:
    /// 1. First try to match by NAME (handles tab reordering)
    /// 2. If name not found, check if tab still exists at the stored POSITION
    ///    (handles rename by other plugins like zellij-tab-name)
    /// 3. If neither found, remove (after grace period)
    fn reconcile_tabs(&mut self, new_tabs: Vec<TabInfo>) {
        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        eprintln!(
            "wt-bridge[{}]: reconcile_tabs called with {} tabs, current tracked={:?}",
            self.instance_id % 10000,
            new_tabs.len(),
            self.path_to_tab.keys().collect::<Vec<_>>()
        );

        // Build maps for both matching strategies
        let mut name_to_positions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut position_to_name: BTreeMap<usize, String> = BTreeMap::new();
        for tab in new_tabs.iter() {
            name_to_positions
                .entry(tab.name.clone())
                .or_default()
                .push(tab.position);
            position_to_name.insert(tab.position, tab.name.clone());
        }

        // Collect paths to remove (can't modify during iteration)
        let mut paths_to_remove = Vec::new();
        let current_pipe_count = self.pipe_call_count;

        for (path, entry) in self.path_to_tab.iter_mut() {
            if let Some(positions) = name_to_positions.get(&entry.name) {
                // Tab with this name still exists - update index
                if positions.len() == 1 {
                    entry.index = positions[0];
                } else {
                    // Multiple tabs with same name - use closest position as heuristic
                    let closest = positions
                        .iter()
                        .min_by_key(|&&pos| (pos as i32 - entry.index as i32).abs())
                        .copied()
                        .unwrap_or(positions[0]);
                    entry.index = closest;
                }
            } else if let Some(new_name) = position_to_name.get(&entry.index) {
                // Tab name changed but position is still valid - update name
                #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
                eprintln!(
                    "wt-bridge[{}]: reconcile_tabs tab renamed {:?} -> {:?} at position {}",
                    self.instance_id % 10000,
                    entry.name,
                    new_name,
                    entry.index
                );
                entry.name = new_name.clone();
            } else {
                // Tab no longer exists - give newly registered entries a grace period
                if current_pipe_count.saturating_sub(entry.registered_at_pipe_call)
                    > GRACE_PERIOD_PIPE_CALLS
                {
                    paths_to_remove.push(path.clone());
                }
            }
        }

        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        if !paths_to_remove.is_empty() {
            eprintln!(
                "wt-bridge[{}]: reconcile_tabs removing paths: {:?}",
                self.instance_id % 10000,
                paths_to_remove
            );
        }

        for path in paths_to_remove {
            self.path_to_tab.remove(&path);
        }

        self.tabs = new_tabs;

        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        eprintln!(
            "wt-bridge[{}]: reconcile_tabs done, tracked now={:?}",
            self.instance_id % 10000,
            self.path_to_tab.keys().collect::<Vec<_>>()
        );
    }
}

/// Generate a short hash of a string (4 hex chars).
pub fn short_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:04x}", hash & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tab(name: &str, active: bool, position: usize) -> TabInfo {
        TabInfo {
            name: name.to_string(),
            active,
            position,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Sync tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn sync_registers_active_tab() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        let response = core.handle_sync("/path/to/worktree");

        assert_eq!(response, Response::Synced);
        assert!(core.path_to_tab.contains_key("/path/to/worktree"));

        let entry = &core.path_to_tab["/path/to/worktree"];
        assert_eq!(entry.index, 0);
        assert_eq!(entry.name, "main");
    }

    #[test]
    fn sync_uses_position_not_enumerate_index() {
        let mut core = WtBridgeCore::new();
        // Tab at position 5, but it's the first element in the array.
        // This catches the bug where we used enumerate index instead of tab.position.
        core.tabs = vec![make_tab("feature", true, 5)];

        core.handle_sync("/path/to/feature");

        let entry = &core.path_to_tab["/path/to/feature"];
        assert_eq!(
            entry.index, 5,
            "Should use tab.position (5), not enumerate index (0)"
        );
    }

    #[test]
    fn sync_is_idempotent() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        // First sync
        core.handle_sync("/path/to/worktree");

        // Modify the tab to verify second sync doesn't overwrite
        core.path_to_tab.get_mut("/path/to/worktree").unwrap().index = 99;

        // Second sync should be a no-op
        let response = core.handle_sync("/path/to/worktree");
        assert_eq!(response, Response::Synced);
        assert_eq!(core.path_to_tab["/path/to/worktree"].index, 99);
    }

    #[test]
    fn sync_fails_with_no_active_tab() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", false, 0)]; // Not active

        let response = core.handle_sync("/path/to/worktree");

        match response {
            Response::Error(msg) => assert!(msg.contains("no_active_tab")),
            _ => panic!("Expected error response"),
        }
    }

    #[test]
    fn sync_fails_with_empty_tabs() {
        let mut core = WtBridgeCore::new();
        // No tabs

        let response = core.handle_sync("/path/to/worktree");

        match response {
            Response::Error(msg) => {
                assert!(msg.contains("no_active_tab"));
                assert!(msg.contains("tabs_len=0"));
            }
            _ => panic!("Expected error response"),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Select tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn select_finds_tracked_path() {
        let mut core = WtBridgeCore::new();
        core.path_to_tab.insert(
            "/path/to/feature".to_string(),
            TabEntry {
                index: 2,
                name: "feature".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        let response = core.handle_select("feature", "/path/to/feature");

        assert_eq!(
            response,
            Response::Focused { tab_index: 3 } // 1-based
        );
    }

    #[test]
    fn select_returns_not_found_for_unknown_path() {
        let mut core = WtBridgeCore::new();

        let response = core
            .handle_message("select|feature|/path/to/feature")
            .unwrap();

        match response {
            Response::NotFound { unique_name } => assert_eq!(unique_name, "feature"),
            _ => panic!("Expected NotFound response, got {:?}", response),
        }
    }

    #[test]
    fn select_generates_unique_name_on_collision() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        let response = core.handle_select("main", "/path/to/other/main");

        match response {
            Response::NotFound { unique_name } => {
                assert!(unique_name.starts_with("main·"));
                assert!(unique_name.len() > 5); // main· + hash
            }
            _ => panic!("Expected NotFound response"),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Register tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn register_adds_new_entry() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        let response = core.handle_register("feature", "/path/to/feature");

        assert_eq!(response, Response::Registered);
        assert!(core.path_to_tab.contains_key("/path/to/feature"));

        let entry = &core.path_to_tab["/path/to/feature"];
        assert_eq!(entry.name, "feature");
        assert_eq!(entry.index, 1); // After existing tab
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tab reconciliation tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn reconcile_updates_tab_indices() {
        let mut core = WtBridgeCore::new();
        core.path_to_tab.insert(
            "/path/a".to_string(),
            TabEntry {
                index: 0,
                name: "a".to_string(),
                registered_at_pipe_call: 0,
            },
        );
        core.path_to_tab.insert(
            "/path/b".to_string(),
            TabEntry {
                index: 1,
                name: "b".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        // Tabs reordered
        core.update_tabs(vec![make_tab("b", false, 0), make_tab("a", true, 1)]);

        assert_eq!(core.path_to_tab["/path/a"].index, 1);
        assert_eq!(core.path_to_tab["/path/b"].index, 0);
    }

    #[test]
    fn reconcile_removes_stale_entries_after_grace_period() {
        let mut core = WtBridgeCore::new();
        core.path_to_tab.insert(
            "/path/old".to_string(),
            TabEntry {
                index: 5, // Position 5 doesn't exist in new_tabs
                name: "old".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        // Simulate many pipe calls passing
        core.pipe_call_count = GRACE_PERIOD_PIPE_CALLS + 2;

        // Tab "old" no longer exists (neither by name nor position)
        core.update_tabs(vec![make_tab("new", true, 0)]);

        assert!(!core.path_to_tab.contains_key("/path/old"));
    }

    #[test]
    fn reconcile_preserves_recent_entries_within_grace_period() {
        let mut core = WtBridgeCore::new();
        core.pipe_call_count = 10;
        core.path_to_tab.insert(
            "/path/new".to_string(),
            TabEntry {
                index: 0,
                name: "new".to_string(),
                registered_at_pipe_call: 8, // Recent
            },
        );

        // Tab "new" not in the TabUpdate yet (race condition)
        core.update_tabs(vec![make_tab("main", true, 0)]);

        // Should be preserved due to grace period
        assert!(core.path_to_tab.contains_key("/path/new"));
    }

    #[test]
    fn reconcile_handles_tab_rename_by_other_plugin() {
        // Simulates zellij-tab-name plugin renaming "worktrunk.release" to " release"
        let mut core = WtBridgeCore::new();
        core.path_to_tab.insert(
            "/path/to/release".to_string(),
            TabEntry {
                index: 1,
                name: "worktrunk.release".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        // Tab was renamed by another plugin but position is the same
        core.update_tabs(vec![
            make_tab("main", true, 0),
            make_tab(" release", false, 1), // Renamed, but still at position 1
        ]);

        // Entry should be preserved with updated name
        assert!(core.path_to_tab.contains_key("/path/to/release"));
        let entry = &core.path_to_tab["/path/to/release"];
        assert_eq!(entry.name, " release"); // Name updated
        assert_eq!(entry.index, 1); // Position unchanged
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Unique name generation tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn unique_name_no_collision() {
        let core = WtBridgeCore::new();

        let name = core.generate_unique_name("feature", "/path/to/feature");

        assert_eq!(name, "feature");
    }

    #[test]
    fn unique_name_collision_with_tab() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        let name = core.generate_unique_name("main", "/other/main");

        assert!(name.starts_with("main·"));
        assert_ne!(name, "main");
    }

    #[test]
    fn unique_name_collision_with_tracked() {
        let mut core = WtBridgeCore::new();
        core.path_to_tab.insert(
            "/path/main".to_string(),
            TabEntry {
                index: 0,
                name: "main".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        let name = core.generate_unique_name("main", "/other/main");

        assert!(name.starts_with("main·"));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Message parsing tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn handle_message_parses_sync() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        let response = core.handle_message("sync|/path/to/worktree");

        assert_eq!(response, Some(Response::Synced));
    }

    #[test]
    fn handle_message_parses_select() {
        let mut core = WtBridgeCore::new();

        let response = core.handle_message("select|feature|/path/to/feature");

        match response {
            Some(Response::NotFound { unique_name }) => assert_eq!(unique_name, "feature"),
            _ => panic!("Expected NotFound response"),
        }
    }

    #[test]
    fn handle_message_parses_register() {
        let mut core = WtBridgeCore::new();

        let response = core.handle_message("register|feature|/path/to/feature");

        assert_eq!(response, Some(Response::Registered));
    }

    #[test]
    fn handle_message_parses_debug() {
        let mut core = WtBridgeCore::new();

        let response = core.handle_message("debug");

        match response {
            Some(Response::Debug { tabs_len, tracked }) => {
                assert_eq!(tabs_len, 0);
                assert!(tracked.is_empty());
            }
            _ => panic!("Expected Debug response"),
        }
    }

    #[test]
    fn handle_message_rejects_unknown() {
        let mut core = WtBridgeCore::new();

        let response = core.handle_message("unknown|foo");

        match response {
            Some(Response::Error(msg)) => assert!(msg.contains("unknown_action")),
            _ => panic!("Expected Error response"),
        }
    }

    #[test]
    fn handle_message_returns_error_for_empty() {
        let mut core = WtBridgeCore::new();

        // Empty string produces [""] after split, which is an unknown action
        match core.handle_message("") {
            Some(Response::Error(msg)) => assert!(msg.contains("unknown_action")),
            _ => panic!("Expected Error response for empty message"),
        }
    }

    #[test]
    fn handle_message_strips_trailing_newline() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        // Sync with trailing newline (as sent by CLI via writeln!)
        let response = core.handle_message("sync|/path/to/worktree\n");
        assert_eq!(response, Some(Response::Synced));

        // Path should be stored WITHOUT the trailing newline
        assert!(core.path_to_tab.contains_key("/path/to/worktree"));
        assert!(!core.path_to_tab.contains_key("/path/to/worktree\n"));

        // Select without trailing newline should find the path
        let response = core.handle_message("select|main|/path/to/worktree");
        assert!(matches!(response, Some(Response::Focused { tab_index: 1 })));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Hash tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn short_hash_is_4_hex_chars() {
        let hash = short_hash("/some/path");

        assert_eq!(hash.len(), 4);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn short_hash_is_deterministic() {
        let hash1 = short_hash("/path/to/worktree");
        let hash2 = short_hash("/path/to/worktree");

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn short_hash_differs_for_different_paths() {
        let hash1 = short_hash("/path/a");
        let hash2 = short_hash("/path/b");

        assert_ne!(hash1, hash2);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Protocol string tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn response_to_protocol_synced() {
        assert_eq!(Response::Synced.to_protocol(), "synced");
    }

    #[test]
    fn response_to_protocol_focused() {
        assert_eq!(
            Response::Focused { tab_index: 3 }.to_protocol(),
            "focused:3"
        );
    }

    #[test]
    fn response_to_protocol_not_found() {
        assert_eq!(
            Response::NotFound {
                unique_name: "main·d5e3".to_string()
            }
            .to_protocol(),
            "not_found:main·d5e3"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Integration scenario tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn scenario_first_switch_from_layout_tab() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        // User runs wt switch @ from layout's initial "main" tab
        // 1. sync registers current tab
        let sync_resp = core.handle_message("sync|/path/to/main");
        assert_eq!(sync_resp, Some(Response::Synced));

        // 2. select finds it (same path)
        let select_resp = core.handle_message("select|main|/path/to/main");
        assert_eq!(select_resp, Some(Response::Focused { tab_index: 1 }));
    }

    #[test]
    fn scenario_switch_to_new_worktree() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", true, 0)];

        // 1. sync current tab
        core.handle_message("sync|/path/to/main");

        // 2. select different path - not found
        let resp = core.handle_message("select|feature|/path/to/feature");
        match resp {
            Some(Response::NotFound { unique_name }) => assert_eq!(unique_name, "feature"),
            _ => panic!("Expected NotFound"),
        }

        // 3. CLI creates tab and registers
        let reg_resp = core.handle_message("register|feature|/path/to/feature");
        assert_eq!(reg_resp, Some(Response::Registered));

        // 4. Next switch to feature should find it
        let select_resp = core.handle_message("select|feature|/path/to/feature");
        assert_eq!(select_resp, Some(Response::Focused { tab_index: 2 }));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Full plugin integration tests (simulates test-wt-bridge.sh flow)
    // ─────────────────────────────────────────────────────────────────────────

    /// Full integration test simulating the test-wt-bridge.sh script flow.
    /// This tests the complete plugin behavior through WtBridgePlugin wrapper.
    #[test]
    fn integration_test_wt_bridge_protocol() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        // Initial state: 2 tabs, Tab1 is active
        plugin.handle_event(PluginEvent::TabsUpdated(vec![
            make_tab("Tab1", true, 0),
            make_tab("Tab2", false, 1),
        ]));

        // Step 1: Sync pathA on Tab1 (active)
        plugin.handle_pipe(
            PipeSourceId::Cli("1".into()),
            "wt",
            Some("sync|/tmp/pathA".into()),
        );
        assert!(plugin.host.outputs[0].1.contains("synced"));
        assert_eq!(plugin.core.path_to_tab["/tmp/pathA"].index, 0);

        // Step 2: CLI switches to Tab2 - plugin receives TabUpdate
        plugin.handle_event(PluginEvent::TabsUpdated(vec![
            make_tab("Tab1", false, 0),
            make_tab("Tab2", true, 1),
        ]));

        // Step 3: Sync pathB on Tab2 (now active)
        plugin.handle_pipe(
            PipeSourceId::Cli("2".into()),
            "wt",
            Some("sync|/tmp/pathB".into()),
        );
        assert!(plugin.host.outputs[1].1.contains("synced"));
        assert_eq!(
            plugin.core.path_to_tab["/tmp/pathB"].index, 1,
            "pathB should be at position 1 (Tab2)"
        );

        // Step 4: Select pathA - should return focused:1
        plugin.handle_pipe(
            PipeSourceId::Cli("3".into()),
            "wt",
            Some("select|test|/tmp/pathA".into()),
        );
        assert!(
            plugin.host.outputs[2].1.contains("focused:1"),
            "pathA should return focused:1, got: {}",
            plugin.host.outputs[2].1
        );

        // Step 5: Select pathB - should return focused:2
        plugin.handle_pipe(
            PipeSourceId::Cli("4".into()),
            "wt",
            Some("select|test|/tmp/pathB".into()),
        );
        assert!(
            plugin.host.outputs[3].1.contains("focused:2"),
            "pathB should return focused:2, got: {}",
            plugin.host.outputs[3].1
        );
    }

    /// Documents the race condition: if TabUpdate isn't received, sync uses stale tab.
    #[test]
    fn integration_race_condition_without_tabupdate() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        // Initial state: 2 tabs, Tab1 is active
        plugin.handle_event(PluginEvent::TabsUpdated(vec![
            make_tab("Tab1", true, 0),
            make_tab("Tab2", false, 1),
        ]));

        // Step 1: Sync pathA on Tab1
        plugin.handle_pipe(
            PipeSourceId::Cli("1".into()),
            "wt",
            Some("sync|/tmp/pathA".into()),
        );
        assert_eq!(plugin.core.path_to_tab["/tmp/pathA"].index, 0);

        // Step 2: CLI switches to Tab2, but NO TabUpdate received (race condition)
        // (we skip the TabsUpdated event here to simulate the race)

        // Step 3: Sync pathB - plugin still thinks Tab1 is active
        plugin.handle_pipe(
            PipeSourceId::Cli("2".into()),
            "wt",
            Some("sync|/tmp/pathB".into()),
        );

        // BUG: pathB is incorrectly associated with Tab1 (index 0)
        assert_eq!(
            plugin.core.path_to_tab["/tmp/pathB"].index, 0,
            "Without TabUpdate, sync incorrectly uses Tab1"
        );

        // This is why test-wt-bridge.sh needs sleep after go-to-tab
    }

    #[test]
    fn scenario_switch_back_to_previous() {
        let mut core = WtBridgeCore::new();
        core.tabs = vec![make_tab("main", false, 0), make_tab("feature", true, 1)];

        // Register both
        core.path_to_tab.insert(
            "/path/main".to_string(),
            TabEntry {
                index: 0,
                name: "main".to_string(),
                registered_at_pipe_call: 0,
            },
        );
        core.path_to_tab.insert(
            "/path/feature".to_string(),
            TabEntry {
                index: 1,
                name: "feature".to_string(),
                registered_at_pipe_call: 0,
            },
        );

        // From feature, sync current
        core.handle_message("sync|/path/feature");

        // Switch back to main
        let resp = core.handle_message("select|main|/path/main");
        assert_eq!(resp, Some(Response::Focused { tab_index: 1 }));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Plugin wrapper tests (using FakeHost)
    // ─────────────────────────────────────────────────────────────────────────

    /// Fake host that records all calls for testing.
    #[derive(Default)]
    struct FakeHost {
        outputs: Vec<(String, String)>,
        unblocks: Vec<String>,
    }

    impl Host for FakeHost {
        fn cli_pipe_output(&mut self, pipe_id: &str, msg: &str) {
            self.outputs.push((pipe_id.to_string(), msg.to_string()));
        }

        fn unblock_cli_pipe_input(&mut self, pipe_id: &str) {
            self.unblocks.push(pipe_id.to_string());
        }
    }

    #[test]
    fn wrapper_ignores_non_wt_pipes() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        let handled = plugin.handle_pipe(
            PipeSourceId::Cli("1".into()),
            "other-plugin",
            Some("payload".into()),
        );

        assert!(!handled);
        assert!(plugin.host.outputs.is_empty());
    }

    #[test]
    fn wrapper_handles_wt_pipe() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        let handled = plugin.handle_pipe(PipeSourceId::Cli("1".into()), "wt", Some("debug".into()));

        assert!(handled);
        assert_eq!(plugin.host.outputs.len(), 1);
        assert!(plugin.host.outputs[0].1.starts_with("debug:"));
    }

    #[test]
    fn wrapper_queues_before_permission() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());

        // Send message before permission granted
        plugin.handle_pipe(
            PipeSourceId::Cli("pipe-7".into()),
            "wt",
            Some("debug".into()),
        );

        // No output yet
        assert!(plugin.host.outputs.is_empty());

        // Grant permission
        plugin.handle_event(PluginEvent::PermissionGranted);

        // Now queued message is processed
        assert_eq!(plugin.host.outputs.len(), 1);
        assert_eq!(plugin.host.outputs[0].0, "pipe-7");
    }

    #[test]
    fn wrapper_responds_with_tab_index_on_focus() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);
        plugin.handle_event(PluginEvent::TabsUpdated(vec![make_tab("main", true, 0)]));

        // Register a path
        plugin.handle_pipe(
            PipeSourceId::Cli("1".into()),
            "wt",
            Some("register|main|/path/main".into()),
        );

        // Select it - should respond with focused:{tab_index}
        plugin.handle_pipe(
            PipeSourceId::Cli("2".into()),
            "wt",
            Some("select|main|/path/main".into()),
        );

        // Tab at position 0 → focused:1 (1-indexed)
        assert_eq!(plugin.host.outputs.len(), 2);
        assert!(plugin.host.outputs[1].1.contains("focused:1"));
    }

    #[test]
    fn wrapper_responds_with_error_on_no_payload() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        plugin.handle_pipe(PipeSourceId::Cli("pipe-5".into()), "wt", None);

        assert_eq!(plugin.host.outputs.len(), 1);
        assert!(plugin.host.outputs[0].1.contains("error:no_payload"));
        assert_eq!(plugin.host.unblocks, vec!["pipe-5"]);
    }

    #[test]
    fn wrapper_ignores_plugin_source() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());
        plugin.handle_event(PluginEvent::PermissionGranted);

        // From plugin, not CLI - should not produce output
        plugin.handle_pipe(PipeSourceId::Plugin, "wt", Some("debug".into()));

        // Message handled but no output (no CLI to respond to)
        assert!(plugin.host.outputs.is_empty());
    }

    #[test]
    fn wrapper_updates_tabs_on_event() {
        let mut plugin = WtBridgePlugin::new(FakeHost::default());

        plugin.handle_event(PluginEvent::TabsUpdated(vec![
            make_tab("main", true, 0),
            make_tab("feature", false, 1),
        ]));

        assert_eq!(plugin.core.tabs.len(), 2);
        assert_eq!(plugin.core.tabs[0].name, "main");
        assert_eq!(plugin.core.tabs[1].name, "feature");
    }
}
