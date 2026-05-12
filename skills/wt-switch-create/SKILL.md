---
name: wt-switch-create
description: Create a new worktrunk worktree and switch this session's working directory into it. Use when launching a session that should work in its own worktree (e.g. `/wt-switch-create my-branch <task>`), or mid-session to move work into a fresh branch.
argument-hint: "<branch-name> [task...]"
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev) and this plugin's WorktreeCreate hook
---

Arguments: `$ARGUMENTS`. The **first whitespace-delimited token** is the branch
name for the new worktree; everything after it (if any) is the task to perform
once inside the worktree.

## What to do

1. **First action — before reading any files or running any commands** — call
   `EnterWorktree({name: "<branch-name>"})` with the first token of the
   arguments as the name. This re-roots the session into the new worktree.

   - It works because this plugin maps `WorktreeCreate` →
     `wt switch --create <name> --no-cd --format=json`, so the new worktree
     lands in worktrunk's normal sibling layout (`<repo>.<branch>/`), not under
     `.claude/worktrees/`.
   - `wt switch --create` is idempotent: if the branch already exists, this
     just re-enters its worktree.
   - If you are *already* inside an `EnterWorktree`-created worktree (e.g. the
     background harness already isolated this session), **skip this step** —
     `EnterWorktree` refuses to nest. Note that you're reusing the existing
     worktree and continue.
   - If `EnterWorktree` fails (not a git repo, invalid branch name, etc.),
     report the error and stop — do not fall back to working in the original
     directory, since that defeats the purpose.

2. After the cwd switch succeeds, proceed with the task portion of the
   arguments (the text after the branch name) in the new worktree. If there was
   no task text, just confirm the worktree is ready and wait for the next
   instruction.

## Cleanup

Don't remove the worktree yourself. `ExitWorktree({action: "remove"})` (if the
user asks to leave) or the session-exit prompt routes through this plugin's
`WorktreeRemove` hook → `wt remove -D --foreground`. A worktree with uncommitted
changes won't be auto-removed without confirmation — that's intended.

## Scope

This command authorizes creating/entering ONE worktree and doing the requested
task. Commits, pushes, and merges still each require explicit user permission.
