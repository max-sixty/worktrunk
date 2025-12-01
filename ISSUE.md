# Zellij Integration: Plugin Architecture Question

## Executive Summary

We have implemented a Zellij workspace integration for worktrunk (a git worktree management CLI). The current architecture uses:

1. A **WASM plugin** (`wt-bridge`) loaded at session start via `load_plugins` in zellij config
2. **Direct `zellij action` commands** for tab management (query, focus, create)

**Our open question**: Is the plugin actually necessary? Or can we remove it entirely and rely solely on `zellij action` commands?

## Goals

We want `wt switch <branch>` to:
1. If running inside a zellij "workspace" (a session managed by worktrunk): Focus or create a tab for the target worktree
2. If running outside zellij: Change directory to the worktree (existing behavior)

The user experience we're targeting:
```bash
# User starts a workspace session
wt ui              # Creates/attaches to zellij session "wt:a1b2c3d"

# Inside the session, switching worktrees switches tabs instead of cd
wt switch feature  # Focuses or creates "feature" tab
wt switch bugfix   # Focuses or creates "bugfix" tab
```

## Current Architecture

### Component 1: The Plugin (`wt-bridge`)

Location: `wt-bridge/src/main.rs`

The plugin is compiled to WASM and loaded via `load_plugins` in `~/.config/zellij/config.kdl`:

```kdl
load_plugins {
    "file:/Users/maximilian/.config/zellij/plugins/wt-bridge.wasm"
}
```

**Current plugin code** (simplified to just permission caching):

```rust
//! wt-bridge: Zellij plugin for worktrunk permission caching.
//!
//! This plugin's primary purpose is to cache zellij permissions. When loaded via
//! `load_plugins` in config.kdl, zellij prompts the user to grant permissions once
//! per session. This avoids repeated permission dialogs.
//!
//! Tab management is handled by the CLI using direct `zellij action` commands,
//! which don't require plugin involvement.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

#[derive(Default)]
struct WtBridge {
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
```

**Permissions requested**:
- `ReadApplicationState` - Read tab names, session info
- `ChangeApplicationState` - Create/focus tabs
- `OpenTerminalsOrPlugins` - Open new terminals/plugins

### Component 2: The CLI Library Layer

Location: `src/zellij/mod.rs`

The CLI uses direct `zellij action` commands for all tab operations:

```rust
/// Focus an existing tab by name.
///
/// Note: zellij stores tab names with a leading space in their internal
/// representation. The `go-to-tab-name` command requires this leading space.
fn go_to_tab(name: &str) -> anyhow::Result<()> {
    // Prepend leading space to match zellij's internal tab name format
    let tab_name = format!(" {}", name.trim());

    let output = Command::new("zellij")
        .args(["action", "go-to-tab-name", &tab_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to go to tab '{}': {}", name, stderr.trim());
    }

    Ok(())
}

/// Create a new tab with the given name and working directory.
fn create_tab(name: &str, cwd: &Path) -> anyhow::Result<()> {
    let cwd_str = cwd
        .to_str()
        .expect("worktree path from git should be valid UTF-8");

    let output = Command::new("zellij")
        .args(["action", "new-tab", "--name", name, "--cwd", cwd_str])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create tab '{}': {}", name, stderr.trim());
    }

    Ok(())
}

/// Check if a tab with the given name exists.
fn tab_exists(name: &str) -> bool {
    let output = match Command::new("zellij")
        .args(["action", "query-tab-names"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let name_trimmed = name.trim();
    stdout.lines().any(|line| line.trim() == name_trimmed)
}

/// Focus or create a tab for a worktree.
pub fn focus_or_create_tab(worktree_path: &Path) -> anyhow::Result<()> {
    let tab_name = worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("worktree");

    if tab_exists(tab_name) {
        go_to_tab(tab_name)
    } else {
        create_tab(tab_name, worktree_path)
    }
}
```

**Commands used**:
- `zellij action query-tab-names` - Lists all tab names (returns names with leading spaces)
- `zellij action go-to-tab-name " {name}"` - Focuses a tab (requires leading space)
- `zellij action new-tab --name {name} --cwd {path}` - Creates a new tab

### How They Interact (Currently)

```
Session Start:
┌─────────────────────────────────────────────────────────────────┐
│ zellij --session wt:a1b2c3d                                      │
│                                                                  │
│   load_plugins {                                                 │
│     "file:~/.config/zellij/plugins/wt-bridge.wasm"               │
│   }                                                              │
│                                                                  │
│   → Plugin loads, requests permissions                           │
│   → Zellij shows permission dialog                               │
│   → User grants permissions                                      │
│   → Permissions "cached" for this session                        │
└─────────────────────────────────────────────────────────────────┘

Tab Management (later in same session):
┌─────────────────────────────────────────────────────────────────┐
│ User runs: wt switch feature                                     │
│                                                                  │
│   CLI detects we're inside workspace (via ZELLIJ_SESSION_NAME)   │
│   CLI runs: zellij action query-tab-names                        │
│   CLI runs: zellij action go-to-tab-name " feature"              │
│       OR: zellij action new-tab --name feature --cwd /path       │
│                                                                  │
│   → Tab is focused or created                                    │
│   → Plugin is NOT involved in this step                          │
└─────────────────────────────────────────────────────────────────┘
```

## What We Tried (And Why We Ended Up Here)

### Original Approach: Plugin-Based Tab Management via Pipe

Initially, we tried to use the zellij pipe mechanism to communicate with the plugin:

```bash
# The CLI would run this to tell the plugin to create/focus a tab:
zellij pipe --name wt -- 'select|/path/to/worktree'
```

The plugin would receive this pipe message and create/focus the tab using plugin API calls.

**Problems encountered**:

1. **Race condition**: Pipe messages arriving before permissions were granted
   - Fixed by adding message queuing in the plugin

2. **Critical issue - Tabs disappearing**: When using `zellij pipe --plugin file:path/to/plugin.wasm`, the plugin instance created is tied to the pipe connection lifecycle
   - When the pipe connection terminates, the plugin instance is destroyed
   - Any tabs created by that plugin instance are destroyed with it
   - This was a fundamental architectural mismatch

**Evidence from testing** (log output):
```
[wt-bridge] Received pipe message: select|/tmp
[wt-bridge] Creating tab: tmp
# Tab appears briefly, then disappears
# Log shows: "Broken pipe", "Received empty message from client"
```

### Current Approach: Direct `zellij action` Commands

We switched to using `zellij action` commands directly from the CLI, bypassing the plugin entirely for tab management.

**This works correctly**:
- `zellij action query-tab-names` returns all tabs
- `zellij action new-tab --name X --cwd Y` creates persistent tabs
- `zellij action go-to-tab-name " X"` focuses tabs (with leading space quirk)

The plugin was simplified to only request permissions at session start.

## Our Assumptions (Unverified)

### Assumption 1: Plugin Permissions Are Shared with CLI Actions

**What we assume**: When the plugin requests and receives permissions at session start, those permissions somehow benefit subsequent `zellij action` commands run from the CLI.

**Why we're uncertain**: `zellij action` commands are external CLI commands, not plugin API calls. They might operate through a completely different permission model.

**What we'd need to verify**:
- Do `zellij action` commands require any permissions at all?
- If they do, are those permissions tied to the plugin's permission grant?
- Or do `zellij action` commands always work regardless of plugin permissions?

### Assumption 2: Plugins Load at Session Start Need Explicit Permission Grants

**What we assume**: Loading a plugin via `load_plugins` in config.kdl causes a one-time permission dialog that the user must approve.

**Why we're uncertain**: We haven't tested what happens if we remove the plugin entirely. Does zellij still prompt for permissions? Or do `zellij action` commands work without any permission grants?

### Assumption 3: Plugin's `has_permission` Flag Is Meaningful

**What we assume**: The plugin's `has_permission` state affects something.

**Why this might be wrong**: Currently the plugin does nothing with this flag. It's just stored. The plugin doesn't perform any actions after receiving permission.

## Open Questions

### Question 1: Do `zellij action` commands require plugin permissions?

The zellij documentation and help output don't clearly state whether `zellij action` commands:
- Require any permissions
- Inherit permissions from loaded plugins
- Always work regardless of permission state

**Research needed**:
- Zellij source code or documentation on the permission model
- Testing: Start a fresh zellij session without the plugin and try `zellij action new-tab`

### Question 2: What is the purpose of `load_plugins` permission grants?

The `load_plugins` directive loads plugins at session start. These plugins can request permissions like `ReadApplicationState`, `ChangeApplicationState`, `OpenTerminalsOrPlugins`.

**What we don't know**:
- Do these permissions only apply to the plugin's own API calls?
- Or do they affect the entire session?
- Can CLI commands (`zellij action`) benefit from these grants?

**Research needed**:
- Zellij documentation on the permission model
- Understanding of how zellij's permission system works architecturally

### Question 3: Can we remove the plugin entirely?

If `zellij action` commands work without any plugin:

1. We could remove `wt-bridge` entirely
2. We could remove `load_plugins` from the config
3. The CLI would use `zellij action` commands directly

**What we'd lose**:
- If permissions ARE needed and ARE provided by the plugin, we'd lose the ability to create/manage tabs

**Research needed**:
- Test `zellij action` commands in a session without the wt-bridge plugin loaded

### Question 4: What permissions do `zellij action` subcommands require?

Looking at the commands we use:
- `zellij action query-tab-names` - Reads tab state
- `zellij action go-to-tab-name` - Changes focus
- `zellij action new-tab` - Creates terminals

Do these map to:
- `ReadApplicationState` (for query)
- `ChangeApplicationState` (for focus change)
- `OpenTerminalsOrPlugins` (for new-tab)

Or do they bypass the permission system entirely since they're CLI commands, not plugin API calls?

## Zellij Command Reference

### Commands Used in Our Implementation

```bash
# Query all tab names (returns names with leading space, e.g., " main", " feature")
zellij action query-tab-names

# Focus a tab by name (requires leading space to match internal format)
zellij action go-to-tab-name " feature"

# Create a new tab
zellij action new-tab --name feature --cwd /path/to/worktree

# List sessions
zellij list-sessions

# Attach to session
zellij attach wt:a1b2c3d

# Create new session
zellij --session wt:a1b2c3d
```

### zellij action go-to-tab-name Help Output

```
zellij-action-go-to-tab-name
Go to tab with name [name]

USAGE:
    zellij action go-to-tab-name [OPTIONS] <NAME>

ARGS:
    <NAME>

OPTIONS:
    -c, --create    Create a tab if one does not exist
    -h, --help      Print help information
```

**Notable**: The `--create` flag could potentially replace our `tab_exists()` + `create_tab()` logic, but we don't use it because we need to set `--cwd` which is only available on `new-tab`.

### zellij action new-tab Help Output

```
zellij-action-new-tab
Create a new tab, optionally with a specified tab layout and name

USAGE:
    zellij action new-tab [OPTIONS]

OPTIONS:
    -c, --cwd <CWD>                  Change the working directory of the new tab
    -h, --help                       Print help information
    -l, --layout <LAYOUT>            Layout to use for the new tab
        --layout-dir <LAYOUT_DIR>    Default folder to look for layouts
    -n, --name <NAME>                Name of the new tab
```

## Zellij Plugin Permission Types

From `zellij_tile::prelude`:

```rust
pub enum PermissionType {
    ReadApplicationState,      // Read tabs, panes, session info
    ChangeApplicationState,    // Modify tabs, panes, focus
    OpenTerminalsOrPlugins,    // Create terminals, launch plugins
    WriteToStdin,              // Send keystrokes to terminals
    RunCommands,               // Execute shell commands
    OpenFiles,                 // Open files
    AccessFileSystem,          // Read/write filesystem
    // ... others
}
```

## Config Files

### ~/.config/zellij/config.kdl (Relevant Section)

```kdl
// Plugins to load in the background when a new session starts
load_plugins {
    "file:/Users/maximilian/.config/zellij/plugins/wt-bridge.wasm"
    "file:~/.config/zellij/plugins/zellij-tab-name.wasm"
}
```

## Summary of What Works vs. What's Uncertain

### What Works

1. **Context detection**: CLI correctly detects when running inside a worktrunk workspace via `ZELLIJ_SESSION_NAME` environment variable

2. **Tab operations via zellij action**:
   - `query-tab-names` successfully returns tab names
   - `go-to-tab-name` successfully focuses tabs (with leading space quirk handled)
   - `new-tab` successfully creates tabs with correct working directory

3. **Plugin loads and receives permissions**: The permission dialog appears and permissions are granted

### What's Uncertain

1. **Whether the plugin is necessary at all**: The CLI doesn't communicate with the plugin. It only uses `zellij action` commands.

2. **Whether `zellij action` commands require the plugin's permissions**: We don't know if these commands would work in a session without the plugin.

3. **Whether we're doing unnecessary work**: Loading a plugin, requesting permissions, tracking permission state - all for a plugin that might serve no purpose.

## Recommended Research Actions

1. **Test without the plugin**: Start a fresh zellij session without `wt-bridge` loaded, then run:
   ```bash
   zellij action query-tab-names
   zellij action new-tab --name test --cwd /tmp
   zellij action go-to-tab-name " test"
   ```
   If these work, the plugin is unnecessary.

2. **Review zellij documentation**: Look for documentation on:
   - The permission model for plugins vs. CLI commands
   - Whether `load_plugins` permissions affect CLI actions
   - Whether `zellij action` commands have their own permission requirements

3. **Review zellij source code**: The permission system implementation would definitively answer whether CLI commands bypass or use the permission system.

## Conclusion

The current architecture works, but may be overcomplicated. The plugin exists solely to "cache permissions," but we haven't verified that:
1. `zellij action` commands actually need those permissions
2. Plugin permissions transfer to CLI command execution
3. There's any benefit to having the plugin at all

If `zellij action` commands work independently of plugin permissions, we can:
1. Delete the `wt-bridge` plugin entirely
2. Remove it from `load_plugins` in config
3. Simplify the setup process (no plugin installation needed)
4. Reduce complexity and maintenance burden
