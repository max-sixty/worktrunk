Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

# OMNIBUS DEMO: Comprehensive worktrunk showcase
#
# Design principles:
# 1. Simple features before complex ones
#    - list, switch, select (navigation) → create, commit, merge (workflows)
# 2. Logical story arc
#    - Overview → work on branches → create feature → complete it → cleanup
# 3. Merge comes AFTER making changes (not on pre-existing uncommitted changes)
# 4. Each Zellij tab shows a distinct workflow
#
# Tab structure:
#   TAB 1: wt list → wt switch api → claude (simple task)
#   TAB 2: [NEW TAB] wsl abbreviation (wt switch -x claude --create) → claude
#   TAB 3: [NEW TAB] wt switch --create → wt step commit → git push
#   TAB 4: [NEW TAB] wt switch auth → wt list → wt remove → wt list --full
#   Then back to TAB 1: [NEW PANE] git diff → wt merge (on Claude's completed work)
#
# Features demonstrated:
# - wt list / wt list --full (URLs, CI status, diff stats)
# - wt switch (simple navigation)
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
Sleep 5s
# Start Zellij before showing
Type "zellij"
Enter
Sleep 4s
# Dismiss plugin permission dialog (zellij-tab-name asks for permissions on first run)
Type "y"
Sleep 1s
Show
Sleep 500ms
# Clear any leftover characters from the permission dialog
Type "clear"
Enter
Sleep 500ms

# ==== TAB 1: Overview and simple navigation ====
# Start with wt list to see all worktrees
Type "wt list"
Enter
Sleep 3s

# Simple switch with Tab completion - navigate to existing worktree
# NOTE: Maintainer marked Tab cycling as excellent in wt-core demo
# - "wt switch a" + Tab → shows api/auth/alpha menu (400ms to review)
# - Tab → selects api (300ms to see selection)
# - Enter → accepts selection (dismisses pager)
# - 50ms then Enter again → executes command
Type "wt switch a"
Tab
Sleep 400ms
Tab
Sleep 300ms
Enter
Sleep 50ms
Enter
Sleep 1.5s

# Launch Claude agent on api (simple task that completes during demo)
# Note: lib.rs has an add() function, so this will definitely produce changes
Type "claude 'Add a test for the add function'"
Enter
Sleep 5.5s

# ==== TAB 2: wsl abbreviation (creates billing, launches Claude) ====
# [NEW TAB] - Ctrl+t creates new Zellij tab (starts in demo repo via default_cwd)
Ctrl+t
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

# ==== TAB 3: Create, commit, push workflow ====
# This tab shows wt step commit as a standalone feature
# [NEW TAB]
Ctrl+t
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

# ==== TAB 4: Navigation, list, and cleanup ====
# [NEW TAB]
Ctrl+t
Sleep 2s

# wt switch - navigate to auth worktree
Type "wt switch auth"
Enter
Sleep 1.5s

# wt list - see current state (before removal)
Type "wt list"
Enter
Sleep 2.5s

# wt remove - delete current worktree (we're in auth)
Type "wt remove"
Enter
Sleep 3s

# wt list --full - shows what changed
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
Ctrl+n
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

# TAB 3: feature branch (already pushed)
Ctrl+Space
Sleep 200ms
Type "t3"
Sleep 1.5s

# TAB 4: show final state with wt list --full
Ctrl+Space
Sleep 200ms
Type "t4"
Sleep 1s

# Show comprehensive list of all worktrees
Type "wt list --full"
Enter
Sleep 3s

# Done!
Type "done!"
Sleep 1.5s
