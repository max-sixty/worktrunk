# wt-bridge Plugin Development

## Building

```bash
# Build the plugin (from wt-bridge directory)
cd wt-bridge
cargo build --target wasm32-wasip1 --features plugin --release

# Install to zellij plugins directory
cp target/wasm32-wasip1/release/wt-bridge.wasm ~/.config/zellij/plugins/

# Or use the setup command from worktrunk.zellij root (builds + installs + configures)
cargo run -- ui setup
```

## Version Compatibility

The plugin uses `zellij-tile` which must match the installed zellij version:

- **zellij 0.43.1** → `zellij-tile = "=0.43.1"` (pinned in Cargo.toml)

If you see "wasm trap: out of bounds memory access" errors in logs, check for version mismatch between zellij and zellij-tile.

## Plugin Caching

**Important:** Zellij has TWO levels of caching:

1. **Compiled wasm cache** at `~/Library/Caches/org.Zellij-Contributors.Zellij` (macOS)
2. **In-memory plugin cache** per session

After rebuilding, you must:

1. **Restart your zellij session** (clears both caches):
   ```bash
   # List sessions
   zellij list-sessions

   # Kill the session you want to restart
   zellij kill-session <session-name>

   # Start fresh
   zellij
   ```

2. **Or use `--skip-plugin-cache`** (bypasses compiled cache):
   ```bash
   zellij action launch-or-focus-plugin "file:~/.config/zellij/plugins/wt-bridge.wasm" --floating --skip-plugin-cache
   ```

## Debugging

### Log File Location

Zellij logs are at:
```
$TMPDIR/zellij-<UID>/zellij-log/zellij.log
```

To find the exact path:
```bash
# macOS (typically /var/folders/.../T/zellij-501/zellij-log/zellij.log)
ls "$TMPDIR"/zellij-*/zellij-log/

# Or use zellij's built-in check
zellij setup --check  # Shows all paths
```

Plugin `eprintln!` output appears in this log file.

### Plugin Logging

Add debug output to the plugin:
```rust
eprintln!("wt-bridge: debug message here: {:?}", some_value);
```

View recent plugin-related logs:
```bash
grep -i "wt-bridge\|wt_bridge" "$TMPDIR"/zellij-*/zellij-log/zellij.log | tail -20
```

### Viewing Plugin in a Pane

To see the plugin's render output and interact with it:
```bash
zellij action launch-or-focus-plugin "file:~/.config/zellij/plugins/wt-bridge.wasm" --floating
```

### Verifying Plugin Contents

Check if your changes are in the compiled wasm:
```bash
strings ~/.config/zellij/plugins/wt-bridge.wasm | grep "your search string"
```

### Common Issues

#### Panic on Plugin Load

If the plugin panics, zellij shows:
```
Loading Panic!
ERROR: <NO PAYLOAD>
PanicHookInfo { ... }
ERROR IN PLUGIN - check logs for more info
```

**Cause:** Often happens when the plugin receives unexpected input (e.g., non-numeric pipe ID from `launch-or-focus-plugin`).

**Fix:** Handle edge cases gracefully instead of using `.expect()` or `.unwrap()`.

#### Wasm Trap / Out of Bounds Memory

If logs show:
```
wasm trap: out of bounds memory access
```

This is a memory safety issue in the plugin code. Check:
1. Tab deserialization in `update()` - ensure events are handled safely
2. Vector/slice access bounds
3. String parsing edge cases

#### Plugin Not Responding

If `wt switch` times out waiting for the plugin:
1. Plugin may not have permissions - launch it in a pane to grant permissions
2. Plugin may have crashed - check logs for errors
3. Plugin may be using old cached version - rebuild and restart session

### Permissions

Plugins need to request permissions in their `load()` method:
```rust
request_permission(&[
    PermissionType::ReadApplicationState,
    PermissionType::ChangeApplicationState,
    PermissionType::OpenTerminalsOrPlugins,
    PermissionType::ReadCliPipes,
]);
```

Permissions are cached by plugin URL after the first grant. To trigger the permission dialog for a background plugin, launch it in a visible pane.

## Testing

The plugin core logic is in `src/lib.rs` and can be tested without zellij:
```bash
cargo test -p wt-bridge --lib
```

The zellij-specific wrapper is in `src/main.rs` and requires manual testing inside zellij.
