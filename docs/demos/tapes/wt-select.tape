Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 500ms
Show
Set KeyStrokes Show
Sleep 500ms

# Launch wt select
Type "wt select"
Enter
Sleep 2s

# Navigate to alpha branch (has large main…± diff and working tree changes)
Down
Sleep 1s
Down
Sleep 1.5s

# Switch to main…± tab (panel 3) to see committed diff from main
Type "3"
Sleep 1.5s

# Browse the diff with Ctrl+D (shows scrolling through large diff)
Ctrl+d
Sleep 1s
Ctrl+d
Sleep 1s
Ctrl+d
Sleep 1.5s

# Filter to narrow down and select
Type "alp"
Sleep 1.5s
Enter
Sleep 2s

# Show we're now in the worktree
Sleep 500ms

Type "exit"
Enter
Sleep 500ms
