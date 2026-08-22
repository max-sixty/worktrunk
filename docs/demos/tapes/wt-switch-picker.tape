Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 1s
Set KeyStrokes Show

# Launch interactive picker (wt switch without args)
Type "wt switch"
Show
Sleep 300ms
Enter
Sleep 1.5s

# Quick browse - second down comes fast
Down
Sleep 0.1s
Down
Sleep 0.8s

# Scroll the diff panel with Ctrl+D
Ctrl+d
Sleep 0.5s
Ctrl+d
Sleep 0.5s
Ctrl+d
Sleep 3s

# Filter to find alpha branch
Type "alp"
Sleep 1s

# Select it
Enter
Sleep 1.5s

Hide
Type "exit"
Enter
