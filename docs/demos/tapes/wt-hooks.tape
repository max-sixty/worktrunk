Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 1s
Show
Sleep 500ms

# Show the hooks config (3 hooks: post-start deps+dev, pre-remove cleanup)
Type "cat .config/wt.toml"
Enter
Sleep 3s

# Create a new worktree - shows post-start hook running
Type "wt switch --create feature"
Enter
Sleep 3s

# Remove a merged worktree
Type "wt remove billing"
Enter
Sleep 2s

Type "exit"
Enter
Sleep 500ms
