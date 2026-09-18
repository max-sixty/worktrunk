Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 1s

# Basic list - progressive rendering
Type "wt list"
Show
Sleep 300ms
Enter
Sleep 2.5s

# Full view with CI and diff stats. The CI cells arrive from the forge behind
# the table, so this waits out the slowest mocked response (DEMO_PRS) and
# still leaves the settled table on screen.
Type "wt list --full"
Enter
Sleep 3.5s

# Show all branches including remote-only
Type "wt list --branches"
Enter
Sleep 2.5s

Hide
Type "exit"
Enter
