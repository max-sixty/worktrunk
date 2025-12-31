Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# Basic list - progressive rendering
Type "wt list"
Enter
Sleep 2.5s

# Full view with CI and diff stats
Type "wt list --full"
Enter
Sleep 2.5s

# Show all branches including remote-only
Type "wt list --branches"
Enter
Sleep 2.5s

Type "exit"
Enter
Sleep 500ms
