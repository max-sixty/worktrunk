Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter

# Start with a complete command in the first recorded frame.
Type "wt list"
Show
Sleep 300ms
Enter
Sleep 2.5s

# Move into existing work, create fresh work, then confirm the result.
Type "wt switch alpha"
Enter
Sleep 1.5s

Type "wt switch --create api"
Enter
Sleep 2.5s

Type "wt list"
Enter
Sleep 2.5s

# Finish on Worktrunk's safe cleanup workflow.
Type "wt remove"
Enter
Sleep 3.5s

Hide
