Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# Switch to branch with uncommitted changes
Type "wt switch hooks"
Enter
Sleep 1s

# Show staged changes
Type "git diff --staged --stat"
Enter
Sleep 2s

# Use wt step commit to generate message via LLM
Type "wt step commit"
Enter
Sleep 4s

# Show the result
Type "git log -1"
Enter
Sleep 3s

Type "exit"
Enter
Sleep 500ms
