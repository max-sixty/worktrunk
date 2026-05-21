Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 1s
Show
Sleep 500ms

# 1. Show current worktrees
Type "wt list"
Enter
Sleep 2s

# 2. Switch to alpha worktree
Type "wt switch alpha"
Enter
Sleep 1s

# 3. Create new worktree with post-start
Type "wt switch --create api"
Enter
Sleep 2.5s

# 4. Show full list with CI status
Type "wt list --full"
Enter
Sleep 3s

# 5. Switch to billing using Tab cycling through branch options
# NOTE: Maintainer marked this as excellent - showcases completion well
# - "wt switch b" + Tab → shows billing/beta menu (400ms to review options)
# - Tab → cycles through options, selecting billing (300ms to see selection)
# - Enter → accepts selection (dismisses pager)
# - 50ms then Enter again → executes command
Type "wt switch b"
Tab
Sleep 400ms
Tab
Sleep 300ms
Enter
Sleep 50ms
Enter
Sleep 1.5s

# 6. Remove the merged worktree (pre-remove hook runs)
Type "wt remove"
Enter
Sleep 5s

Type "exit"
Enter
Sleep 500ms
