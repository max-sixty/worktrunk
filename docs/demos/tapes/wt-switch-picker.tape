Source docs/demos/tapes/shared-setup.tape

Output "{{OUTPUT_GIF}}"

# PICKER DEMO: the interactive picker as its own subject
#
# The picker's two selling points are hard to show in a still: rows that paint
# from local git before any network call, with per-row CI arriving behind them,
# and a preview pane that pages through a diff or a PR thread without leaving
# the list. The order below takes them in turn.
#
# 1. wt switch          — rows land immediately, CI still in flight
# 2. alt-p              — preview off: the same rows reveal their right-hand
#                         columns (CI, commit, age) while CI is still arriving
# 3. alt-p, ↓↓          — preview back on, following the cursor
# 4. "alpha"            — filter to the branch with the large diff
# 5. ctrl-d ×3          — page through a 397-line diff
# 6. alt-8, ctrl-d      — the PR's comment thread, paged
# 7. alt-7              — the PR itself: title, author, URL, description
# 8. Enter              — switch to it
#
# The environment is `prepare_picker` in docs/demos/build: eleven worktrees,
# varied CI (PR numbers, bare branch CI, none), and a mocked forge whose
# per-branch response delays are what make the CI column stream.

Source docs/demos/tapes/shared-commands.tape

Type "clear"
Enter
Sleep 1s
Set KeyStrokes Show

# Open the picker. Rows paint from local git right away.
Type "wt switch"
Show
Sleep 300ms
Enter
Sleep 500ms

# alt-p hides the preview, and the same rows widen to full width to reveal the
# columns the preview was covering. No reload, so no column moves — and the CI
# cells are still landing one at a time behind the frame.
Alt+p
Sleep 3s

# Preview back on. It follows the cursor.
Alt+p
Sleep 1.2s
Down
Sleep 1.2s
Down
Sleep 1.2s

# Filter to alpha, which carries the large diff. Spelled out rather than
# "alp": narrowing the list keeps the cursor's row *index*, so a query that
# still matches several rows would leave it on whichever one now sits at that
# index (after the two Downs above, "alp" selects beta).
Type "alpha"
Sleep 1.5s

# Page through it.
Ctrl+d
Sleep 700ms
Ctrl+d
Sleep 700ms
Ctrl+d
Sleep 2.5s

# alt-8 is the PR's comment thread, which pages the same way. VHS needs the
# digit quoted — `Alt+` takes a string, Enter or Tab, and a bare 8 lexes as a
# number.
Alt+"8"
Sleep 3s
Ctrl+d
Sleep 3s

# alt-7 is the PR itself.
Alt+"7"
Sleep 3s

# Enter switches to the selected worktree.
Enter
Sleep 2.5s

Hide
Type "exit"
Enter
