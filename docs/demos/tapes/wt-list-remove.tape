Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# Show worktrees
Type "wt list"
Enter
Sleep 2s

# Remove a merged worktree using Tab completion
# NOTE: Maintainer marked Tab cycling as excellent in wt-core demo
# - "wt remove b" + Tab → shows billing/beta menu (400ms to review)
# - Tab → selects billing (300ms to see selection)
# - Enter → accepts selection (dismisses pager)
# - 50ms then Enter again → executes command
Type "wt remove b"
Tab
Sleep 400ms
Tab
Sleep 300ms
Enter
Sleep 50ms
Enter
Sleep 2s

Type "exit"
Enter
Sleep 500ms
