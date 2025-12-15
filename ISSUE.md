# Issue Report: Zellij Plugin `go_to_tab()` API Not Switching Tabs

## Executive Summary

We have a Zellij plugin (`wt-bridge`) that should switch tabs when receiving a "select" command via pipe. Debug logging confirms `go_to_tab(N)` is being called with the correct index, but the tab does not actually switch. The plugin returns "focused" to the CLI indicating success, but the tab remains unchanged.

## Goals

1. When `wt switch <worktree>` is run, the wt-bridge plugin should:
   - Receive a `select|<name>|<path>` message via zellij pipe
   - Look up the path in its internal mapping to find the associated tab
   - Call `go_to_tab(index)` to switch to that tab
   - Return "focused" to indicate success

2. The user should see the zellij tab actually switch to the target worktree's tab.

## Architecture Overview

### Components

1. **worktrunk CLI** (`wt switch`) - Rust CLI that sends pipe messages to zellij
2. **wt-bridge plugin** - Zellij WASM plugin that:
   - Receives pipe messages
   - Tracks which paths map to which tabs
   - Calls zellij APIs to switch tabs
   - Responds back to CLI

### Communication Flow

```
wt switch foo
    |
    v
zellij pipe --name wt -- 'select|foo|/path/to/foo'
    |
    v
wt-bridge plugin receives pipe message
    |
    v
plugin calls go_to_tab(N)
    |
    v
plugin responds "focused" via cli_pipe_output
```

## Relevant Code

### Plugin Entry Point (main.rs)

The plugin uses a `Host` trait to abstract zellij API calls for testability:

```rust
// wt-bridge/src/main.rs (lines 60-76)

/// Zellij host implementation - calls actual zellij APIs.
struct ZellijHost;

impl Host for ZellijHost {
    fn go_to_tab(&mut self, index: u32) {
        eprintln!("wt-bridge: go_to_tab({})", index);  // DEBUG LOGGING
        go_to_tab(index);  // This is zellij_tile::prelude::go_to_tab
    }

    fn cli_pipe_output(&mut self, pipe_id: &str, msg: &str) {
        cli_pipe_output(pipe_id, msg);
    }

    fn unblock_cli_pipe_input(&mut self, pipe_id: &str) {
        unblock_cli_pipe_input(pipe_id);
    }
}
```

### Plugin Load and Permissions (main.rs)

```rust
// wt-bridge/src/main.rs (lines 93-103)

impl ZellijPlugin for WtBridge {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,  // Required for go_to_tab
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::ReadCliPipes,
        ]);

        subscribe(&[EventType::TabUpdate, EventType::PermissionRequestResult]);
    }
    // ...
}
```

### Core Plugin Logic (lib.rs)

#### Permission Handling and Message Processing

```rust
// wt-bridge/src/lib.rs (lines 140-161)

/// Handle incoming pipe message.
pub fn pipe(&mut self, source: PipeSourceId, _name: &str, payload: Option<&str>) -> bool {
    #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
    eprintln!(
        "wt-bridge: pipe() called - name={:?}, payload={:?}, source={:?}",
        _name, payload, source
    );

    let payload = match payload {
        Some(p) => p.to_string(),
        None => {
            // ...
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
```

#### Message Handler That Calls go_to_tab

```rust
// wt-bridge/src/lib.rs (lines 179-187)

/// Handle a message using the core logic.
fn handle_message(&mut self, source: PipeSourceId, payload: String) {
    if let Some(response) = self.core.handle_message(&payload) {
        if let Response::Focused { tab_index } = &response {
            self.host.go_to_tab(*tab_index);  // <-- THIS IS THE KEY CALL
        }
        self.respond(&source, &response.to_protocol());
    }
}
```

#### Handle Select Logic

```rust
// wt-bridge/src/lib.rs (lines 242-264)

/// Handle select: focus existing tab or respond with name for creation.
pub fn handle_select(&self, display_name: &str, path: &str) -> Response {
    #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
    eprintln!(
        "wt-bridge: select display_name={} path={} tracked={:?}",
        display_name, path, self.path_to_tab.keys().collect::<Vec<_>>()
    );
    if let Some(entry) = self.path_to_tab.get(path) {
        #[cfg(all(feature = "plugin", target_arch = "wasm32"))]
        eprintln!(
            "wt-bridge: found entry index={} name={} -> go_to_tab({})",
            entry.index, entry.name, entry.index as u32 + 1
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
```

### Response Type

```rust
// wt-bridge/src/lib.rs

pub enum Response {
    Synced,
    Registered,
    Focused { tab_index: u32 },
    NotFound { unique_name: String },
    Error(String),
}

impl Response {
    pub fn to_protocol(&self) -> String {
        match self {
            Response::Synced => "synced".to_string(),
            Response::Registered => "registered".to_string(),
            Response::Focused { .. } => "focused".to_string(),
            Response::NotFound { unique_name } => format!("not_found:{unique_name}"),
            Response::Error(msg) => msg.clone(),
        }
    }
}
```

## What We've Tried

### 1. Adding Debug Logging

Added `eprintln!` statements to trace the execution flow:

```rust
// In ZellijHost::go_to_tab
eprintln!("wt-bridge: go_to_tab({})", index);

// In handle_select
eprintln!("wt-bridge: select display_name={} path={} tracked={:?}", ...);
eprintln!("wt-bridge: found entry index={} name={} -> go_to_tab({})", ...);
```

### 2. Rebuilt and Installed Plugin

```bash
cargo build --target wasm32-wasip1 --features plugin --release
cp target/wasm32-wasip1/release/wt-bridge.wasm ~/.config/zellij/plugins/
```

Verified debug strings are in the installed plugin:
```bash
$ strings ~/.config/zellij/plugins/wt-bridge.wasm | grep "wt-bridge: go_to_tab"
Keymodifiermain_keyadditional_modifierswt-bridge: go_to_tab()
```

### 3. Created Test Sessions

Set up zellij test sessions with multiple tabs:
```bash
zellij --session wt-test3
zellij -s wt-test3 action new-tab --name "Tab2"
zellij -s wt-test3 action new-tab --name "Tab3"
```

### 4. Loaded Plugin and Synced Paths

```bash
zellij -s wt-test3 action launch-or-focus-plugin "file:/Users/maximilian/.config/zellij/plugins/wt-bridge.wasm" --floating

# Sync paths to tabs
zellij -s wt-test3 action go-to-tab 1
zellij -s wt-test3 pipe --name wt -- 'sync|/pathA'   # Returns: synced

zellij -s wt-test3 action go-to-tab 2
zellij -s wt-test3 pipe --name wt -- 'sync|/pathB'   # Returns: synced
```

### 5. Tested Tab Switching

From tab 3, tried to switch to tab 1 (where /pathA is mapped):
```bash
zellij -s wt-test3 action go-to-tab 3
zellij -s wt-test3 pipe --name wt -- 'select|test|/pathA'
# Returns: focused
```

**Result: Tab does NOT switch.** The plugin returns "focused" but the active tab remains tab 3.

## Debug Log Output

From `/var/folders/wf/s6ycxvvs4ln8qsdbfx40hnc40000gn/T/zellij-501/zellij-log/zellij.log`:

```
DEBUG  |/Users/maximilian/.config| 2025-12-03 13:50:54.704 [id: 1] wt-bridge: select display_name=test path=/pathA tracked=["/pathA", "/pathB"]
DEBUG  |/Users/maximilian/.config| 2025-12-03 13:50:54.704 [id: 1] wt-bridge: found entry index=0 name= zellij -> go_to_tab(1)
DEBUG  |/Users/maximilian/.config| 2025-12-03 13:50:54.704 [id: 1] wt-bridge: go_to_tab(1)
```

This confirms:
1. The select message is received correctly
2. The path lookup succeeds (finds index=0)
3. `go_to_tab(1)` is called (1-indexed, so first tab)

But the tab doesn't actually switch.

### Multiple Attempts

Multiple `go_to_tab(1)` calls logged at different times, all with same result:
```
13:50:54.704 - go_to_tab(1)
13:52:57.863 - go_to_tab(1)
13:54:52.335 - go_to_tab(1)
```

None of these resulted in an actual tab switch.

## Plugin UI State Verification

Using the plugin's floating UI, we confirmed the state:

```
Tabs:
  [0]  zellij (pos=0)
  [1]  zellij (pos=1) *     <- Currently active
  [2]  zellij (pos=2)

Tracked:
  /pathA -> tab 0 ( zellij)
  /pathB -> tab 1 ( zellij)
```

After calling `select|test|/pathA`:
- Expected: Tab [0] becomes active
- Actual: Tab [1] or [2] remains active (didn't switch)

## Zellij go_to_tab API Documentation

From https://zellij.dev/documentation/plugin-api-commands.html:

> **go_to_tab**
> Change the focused tab to the specified index (corresponding with the default tab names, starting at `1`, `0` will be considered as `1`).
>
> **Permission Required:** `ChangeApplicationState`

Key points:
- **1-indexed**: Tab 1 = first tab (position 0)
- Requires `ChangeApplicationState` permission

## Assumptions and Hypotheses

### Assumption 1: Permissions Are Granted
We request `ChangeApplicationState` in `load()`:
```rust
request_permission(&[
    PermissionType::ReadApplicationState,
    PermissionType::ChangeApplicationState,  // Needed for go_to_tab
    // ...
]);
```

The plugin IS responding to pipe messages, which suggests at least some permissions are granted. However, we haven't explicitly verified that `ChangeApplicationState` was granted.

**Counter-evidence**: The plugin receives `TabUpdate` events and pipe messages, indicating `ReadApplicationState` and `ReadCliPipes` work. But we haven't confirmed `ChangeApplicationState` specifically.

### Assumption 2: go_to_tab Works From Floating Plugins
We're calling `go_to_tab` from a floating plugin pane. It's possible floating plugins have restrictions on changing application state.

**Unknown**: No documentation found about floating plugin restrictions.

### Assumption 3: go_to_tab Works From Pipe Event Handler
We call `go_to_tab` synchronously within the pipe message handler. It's possible the API requires the call to happen outside the event handler, or needs to be deferred.

**Unknown**: No documentation about this constraint.

### Assumption 4: The API Actually Works
It's possible there's a bug in zellij's `go_to_tab` implementation for plugins.

**Unknown**: Need to check zellij issues or try other plugins that use go_to_tab.

## What We Haven't Tried

1. **Running plugin as non-floating (tiled) pane** - to rule out floating pane restrictions
2. **Calling go_to_tab from render() or other event** - to rule out pipe handler issues
3. **Checking if permissions are actually granted** - no easy way to verify this
4. **Using a different zellij version** - currently on zellij 0.43.1
5. **Looking at other plugins that successfully use go_to_tab** - like zjstatus or tab-bar

## Open Questions

1. **Does `go_to_tab` work at all from WASM plugins?** Are there any known working examples?

2. **Does calling `go_to_tab` from within a pipe handler work?** Or does it need to be deferred?

3. **Do floating plugins have restrictions on `ChangeApplicationState`?**

4. **Is there a way to verify which permissions were actually granted?**

5. **Is there a zellij bug related to `go_to_tab` not working?** GitHub issues to check:
   - https://github.com/zellij-org/zellij/issues/3535 (tab index issues after creating/deleting tabs)
   - https://github.com/zellij-org/zellij/issues/3878 (pipe communication issues)

6. **Is `go_to_tab` the right API?** Should we use `focus_terminal_pane` or another API instead?

7. **What happens when `go_to_tab` fails?** Is there any error logged? Does it fail silently?

## Environment

- **Zellij version**: 0.43.1
- **zellij-tile crate**: 0.43.1 (pinned in Cargo.toml)
- **OS**: macOS Darwin 25.0.0
- **Plugin target**: wasm32-wasip1

## Potential Research Directions

1. **Search zellij GitHub issues** for "go_to_tab" "not working" or "plugin" "tab switch"

2. **Look at zjstatus plugin source** (https://github.com/dj95/zjstatus) - it's a popular plugin that might use go_to_tab

3. **Check zellij Discord or discussions** for plugin developers who've used go_to_tab

4. **Read zellij source code** for `go_to_tab` implementation to understand when it might fail silently

5. **Check if there are alternative APIs** like `focus_tab_index` or similar that might work better

## Workaround Considered

Instead of having the plugin call `go_to_tab`, we could:
1. Have the plugin return the tab index in the response
2. Have the CLI call `zellij action go-to-tab N` directly

This bypasses the plugin permission system entirely, but adds latency and complexity.

## Key Files

- Plugin source: `/Users/maximilian/workspace/worktrunk.zellij/wt-bridge/src/lib.rs`
- Plugin main: `/Users/maximilian/workspace/worktrunk.zellij/wt-bridge/src/main.rs`
- Installed plugin: `~/.config/zellij/plugins/wt-bridge.wasm`
- Zellij logs: `/var/folders/wf/s6ycxvvs4ln8qsdbfx40hnc40000gn/T/zellij-501/zellij-log/zellij.log`

---

## Current Issue: Plugin State Not Persisting Between Operations

### Problem Description

After implementing the workaround (CLI calls `go-to-tab` instead of plugin), we're still seeing duplicate tabs being created. The plugin's `path_to_tab` mapping is being reset between operations.

### Manual Test Procedure

```bash
# 1. Build and install plugin with debug logging
cargo run -- ui setup

# 2. Kill any existing test session
zellij kill-session wt:e9fcb6e 2>/dev/null || true

# 3. Start the workspace using wt ui (creates correct session name)
# From inside tmux or another terminal:
wt ui  # This starts zellij session wt:e9fcb6e

# 4. Inside the zellij session, run test sequence:
wt switch release    # Should create tab 2 (main=1, release=2)
wt switch main       # Should focus tab 1 (no new tab)
wt switch release    # CRITICAL: Should focus tab 2, NOT create tab 3!

# 5. Check tabs after sequence:
zellij -s wt:e9fcb6e action query-tab-names
# Expected: main, release (2 tabs)
# Actual: main, release, release (3 tabs - FAILURE)

# 6. Check logs for debug output:
grep -E "wt-bridge:" /var/folders/wf/s6ycxvvs4ln8qsdbfx40hnc40000gn/T/zellij-501/zellij-log/zellij.log | tail -30
```

### Critical Observations

1. **Session name matters**: Must use `wt ui` to start session (creates `wt:e9fcb6e`).
   Using `zellij -s wt:test` bypasses plugin communication entirely because
   `detect_context()` returns `InsideOtherWorkspace` instead of `InsideWorkspace`.

2. **Register works but state disappears**: Log shows:
   ```
   19:00:49.704 register done, tracked=[worktrunk, worktrunk.release]
   19:01:11.969 select main, tracked=[worktrunk]  # <-- release GONE!
   ```

3. **Multiple plugin instances**: There may be multiple wt-bridge instances across
   different sessions, and pipe messages may be going to the wrong instance.

### Debug Logging Added

Added debug logging to `handle_register` in `wt-bridge/src/lib.rs`:
```rust
eprintln!("wt-bridge: register tab_name={} path={} tabs={:?}", ...);
eprintln!("wt-bridge: register found tab, using position={}", ...);
eprintln!("wt-bridge: register done, tracked={:?}", ...);
```

### Current Hypothesis

The plugin is loaded per-session, but the `zellij pipe --plugin PLUGIN_PATH` command
may be targeting the wrong plugin instance when multiple wt-bridge instances exist
across different sessions.

Need to investigate:
1. How zellij routes pipe messages to specific plugin instances
2. Whether the pipe command inherits session context from environment
3. Whether there's a way to target a specific session's plugin instance
