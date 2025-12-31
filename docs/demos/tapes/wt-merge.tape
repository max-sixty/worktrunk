Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Env NEXTEST_STATUS_LEVEL "none"
Env NEXTEST_FINAL_STATUS_LEVEL "flaky"
Env NEXTEST_NO_FAIL_FAST "1"

Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

Type "wt list"
Enter
Sleep 1.6s

Type "wt switch --create notes"
Enter
Sleep 1.6s

Type 'echo "- Q4 report ready" >> notes.md'
Enter
Sleep 0.8s

# Pre-enter pause: merge output scrolls past visible area
Type "wt merge"
Sleep 1s
Enter
Sleep 8s

Type "wt list --branches --full"
Enter
Sleep 1.5s

Sleep 4.5s
Type "exit"
Enter
Sleep 700ms
