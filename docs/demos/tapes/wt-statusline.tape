Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Env ANTHROPIC_API_KEY "{{ANTHROPIC_API_KEY}}"

# Pre-install marketplace plugin before recording
Type "claude plugin marketplace add anthropics/claude-plugins-official"
Enter
Sleep 5s
Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# Switch to alpha worktree first (has uncommitted changes for diff stats)
Type "wt switch alpha"
Enter
Sleep 1.5s

# Launch Claude Code to show statusline integration
# Tweet 12 is about seeing branch, diff stats, CI status at a glance in Claude's UI
# Statusline will show: ~/w/acme.alpha  alpha  +N  ●  | Opus 4.5
# Pre-enter pause for big UI takeover
Sleep 1s
Type "claude"
Enter
Sleep 8s
