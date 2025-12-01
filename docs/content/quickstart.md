+++
title = "Quick Start"
weight = 1
+++

## Install

```bash
$ cargo install worktrunk
$ wt config shell install  # allows commands to change directories
```

## Create a worktree

```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```

This creates `../repo.fix-auth` on branch `fix-auth`.

## Switch between worktrees

```bash
$ wt switch feature-api
✅ Switched to worktree for feature-api at ../repo.feature-api
```

## List worktrees

```bash
$ wt list
  Branch       Status         HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ feature-api  +   ↑⇡      +36  -11   ↑4      ./repo.feature-api   ⇡3      b1554967  30m   Add API tests
^ main             ^⇣                         ./repo                   ⇣1  b834638e  1d    Initial commit
+ fix-auth        _                           ./repo.fix-auth              b834638e  1d    Initial commit

⚪ Showing 3 worktrees, 1 with changes, 1 ahead
```

Add `--full` for CI status and conflicts. Add `--branches` to include all branches.

## Clean up

When you're done with a worktree (e.g., after merging via CI):

```bash
$ wt remove
🔄 Removing feature-api worktree & branch in background (no marginal contribution to main)
```

Worktrunk checks if your changes are already on main before deleting the branch.

## Shortcuts

Use these shortcuts for common targets:

- `@` — current branch/worktree
- `-` — previous worktree (like `cd -`)
- `^` — main/default branch

```bash
$ wt switch -                              # Switch to previous worktree
$ wt switch --create hotfix --base=@       # Branch from current HEAD
$ wt remove @                              # Remove current worktree
```

## Next steps

- Learn [why worktrees](/concepts/) and how Worktrunk improves on plain git
- Set up [project hooks](/configuration/) for automated setup
- Use [LLM commit messages](/configuration/#llm-commit-messages) for auto-generated commits
