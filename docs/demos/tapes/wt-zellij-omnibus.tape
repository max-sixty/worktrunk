Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

# OMNIBUS DEMO: Comprehensive worktrunk showcase
#
# Design principles:
# 1. Simple features before complex ones
#    - list, switch (navigation) → create, commit, merge (workflows)
# 2. Logical story arc
#    - Overview → work on branches → create feature → complete it → cleanup
# 3. Merge comes AFTER making changes (not on pre-existing uncommitted changes)
# 4. Each Zellij tab shows a distinct workflow
#
# Tab structure:
#   TAB 1: wt switch (picker → api) → claude (simple task)
#   TAB 2: [NEW TAB] wsl abbreviation (wt switch -x claude --create) → claude
#   TAB 3: [NEW TAB] wt switch --create → wt step commit → git push → wt remove → wt list --full
#   Then back to TAB 1: [NEW PANE] git diff → wt merge (on Claude's completed work)
#
# Features demonstrated:
# - wt list / wt list --full (URLs, CI status, diff stats)
# - wt switch (interactive picker)
# - wsl abbreviation (wt switch --execute=claude --create)
# - wt switch --create (with post-start hooks)
# - wt step commit (LLM commit message, standalone)
# - wt merge (on uncommitted work - commits, rebases, merges, cleans up)
# - wt remove (cleanup)
# - Multi-agent Claude Code in parallel tabs

Require zellij

Source docs/demos/tapes/shared-commands.tape

Env ANTHROPIC_API_KEY "{{ANTHROPIC_API_KEY}}"

# Pre-install marketplace plugin before recording
Type "claude plugin marketplace add anthropics/claude-plugins-official"
Enter
Sleep 10s
# Start Zellij before showing
Type "zellij"
Enter
Sleep 3s
# Dismiss plugin permission dialog (zellij-tab-name asks for permissions on first run)
# The permission is cached per-plugin-path, but on macOS Zellij uses native cache APIs
# that ignore $HOME, so we can't pre-populate the cache in the demo's temp directory.
# If no dialog appears (cache hit), "y" goes to shell, so we follow with Enter + clear.
Type "y"
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# ==== TAB 1: Interactive picker and Claude agent ====
# wt switch (no args) - opens interactive picker
Type "wt switch"
Enter
Sleep 1.5s

# Filter to api branch
Type "api"
Sleep 1s

# Select it
Enter
Sleep 1.5s

# Launch Claude agent on api (simple task that completes during demo)
# Note: lib.rs has an add() function, so this will definitely produce changes
Type "claude 'Add a test for the add function'"
Enter
Sleep 5.5s

# ==== TAB 2: wsl abbreviation (creates billing, launches Claude) ====
# [NEW TAB] via session manager (Ctrl+Space → tab mode → new)
# Direct Ctrl+t is intercepted by Claude Code's TUI, so use session manager.
Ctrl+Space
Sleep 200ms
Type "tn"
Sleep 2s

# wsl abbreviation expands to: wt switch --execute=claude --create
# Type "ws" then "l" to trigger expansion, then add branch name and instructions
Type "ws"
Type "l"
Sleep 200ms
Type " billing -- 'Format billing amounts with currency symbols'"
Sleep 500ms
Enter
Sleep 6s

# ==== TAB 3: Create, commit, push, remove workflow ====
# This tab shows wt step commit as a standalone feature, then cleanup
# [NEW TAB] via session manager
Ctrl+Space
Sleep 200ms
Type "tn"
Sleep 2s

# wt switch --create - shows post-start hooks running
Type "wt switch --create feature"
Enter
Sleep 4.5s

# Make a change
Type "echo '// User settings module' >> src/lib.rs"
Enter
Sleep 800ms

# wt step commit - LLM generates commit message (standalone feature)
Type "wt step commit"
Enter
Sleep 4s

# Push to origin (feature branch is now ready)
Type "git push -u origin feature"
Enter
Sleep 2s

# wt remove - done with feature, clean up
Type "wt remove"
Enter
Sleep 3s

# wt list --full - show remaining worktrees
Type "wt list --full"
Enter
Sleep 3.5s

# ==== Back to TAB 1: Merge Claude's completed work ====
# Agent finished - open new pane to show changes and merge
Ctrl+Space
Sleep 200ms
Type "t1"
Sleep 2s

# Open new pane for git diff and merge (like original zellij demo)
# Use session manager (Ctrl+Space → pane mode → new) since Ctrl+n is
# intercepted by Claude Code's TUI running in the active pane.
Ctrl+Space
Sleep 200ms
Type "on"
Sleep 1.2s

# Show what Claude changed
Type "git diff"
Enter
Sleep 3s

# wt merge - handles uncommitted changes, rebases, merges, cleans up
# Pre-merge hook runs tests, then commit, rebase, merge, cleanup
# (heavy output command - pre-enter pause)
Sleep 1s
Type "wt merge"
Enter
Sleep 11s

# ==== Cycle through all tabs to show parallel work ====
# TAB 2: billing agent working
Ctrl+Space
Sleep 200ms
Type "t2"
Sleep 1.5s

# TAB 3: show final state with wt list --full
Ctrl+Space
Sleep 200ms
Type "t3"
Sleep 1s

# Show comprehensive list of all worktrees (post-merge state)
Type "wt list --full"
Enter
Sleep 3s

# Done!
Type "done!"
Sleep 1.5s

Hide
# Clean up zellij after recording stops
Ctrl+Space
Sleep 200ms
Type "q"
Sleep 500ms
