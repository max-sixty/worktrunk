# TODO

## Resolved: wt-bridge Plugin State Loss (2024-12-03)

### Problem (Fixed)
Plugin's `path_to_tab` mapping was being cleared when tabs were renamed by external plugins (like zellij-tab-name), causing duplicate tabs on subsequent switches.

### Root Cause
The `reconcile_tabs()` function only matched by tab name. When zellij-tab-name plugin renamed "worktrunk.release" to " release", the entry couldn't be found by name and was removed after the grace period.

### Solution
Updated `reconcile_tabs()` to use dual matching strategy:
1. First try to match by NAME (handles tab reordering)
2. If name not found, check if tab still exists at stored POSITION (handles rename by other plugins)
3. If neither found, remove (after grace period)

See test: `reconcile_handles_tab_rename_by_other_plugin` in `wt-bridge/src/lib.rs`

### Verification
```bash
# Test procedure (all should work now):
cargo run -- ui setup
wt ui
wt switch release    # Creates tab 2
wt switch main       # Should focus tab 1
wt switch release    # Should focus tab 2 (not create tab 3!)

# Verify: should show only 2 tabs
zellij -s wt:<hash> action query-tab-names
# Expected: main, release (2 tabs) ✓
```
