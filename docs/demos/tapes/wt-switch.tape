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
Sleep 1s
Show
Sleep 500ms

# Switch to existing worktree
Type "wt switch alpha"
Enter
Sleep 1.5s

# Create new worktree (runs post-start hooks)
Type "wt switch --create api"
Enter
Sleep 1.5s

# Create with specific command execution - launch Claude with initial prompt
Type "wt switch --execute claude --create dashboard -- 'Add settings page'"
Sleep 500ms
Enter
Sleep 6s
