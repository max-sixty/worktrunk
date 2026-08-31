Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Env ANTHROPIC_API_KEY "{{ANTHROPIC_API_KEY}}"
Env CLAUDE_CODE_OAUTH_TOKEN "{{CLAUDE_CODE_OAUTH_TOKEN}}"

Type "clear"
Enter
Sleep 1s

# Switch to alpha worktree first (has uncommitted changes for diff stats)
Type "wt switch alpha"
Show
Sleep 300ms
Enter
Sleep 1.5s

# Launch Claude Code to show statusline integration
# Tweet 12 is about seeing branch, diff stats, CI status at a glance in Claude's UI
# Statusline will show: ~/w/acme.alpha  alpha  +N  #N  | Opus 4.5
# Pre-enter pause for big UI takeover
Sleep 1s
Type "claude"
Enter
Sleep 8s

Hide
