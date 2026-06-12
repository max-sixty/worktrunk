---
name: wt-switch-create
description: Create a new worktrunk worktree and switch this session's working directory into it; a repo argument creates the worktree in another repo instead (without re-rooting the session). Use when launching a session that should work in its own worktree (e.g. `/wt-switch-create -- <task>`, or `/wt-switch-create my-branch ~/workspace/other-repo -- <task>`), or mid-session to move work into a fresh branch.
argument-hint: "[<branch>] [<repo>] [-- task...]"
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev) and this plugin's WorktreeCreate hook
---

Arguments: `$ARGUMENTS`. Grammar: `[<branch>] [<repo>] [-- <task>]`.

- **branch** — name for the new worktree's branch; when omitted, derive a
  name from the task consistent with existing worktree names.
- **repo** — path; create the worktree in this repo instead of the session's
  current one.
- **task** — what to do inside the new worktree. No task means enter the
  worktree and wait.

A path-shaped token (absolute, `~`-relative, `./`- or `../`-relative) among
the first two tokens is the repo. Everything after `--` is the task, and a
bare token before it is the branch. Without a `--`: one remaining bare token
is the branch; more than one means it's all task (derive the branch from it).

```
/wt-switch-create my-feature -- fix the parser bug
/wt-switch-create -- fix the parser bug
/wt-switch-create fix the parser bug
/wt-switch-create my-feature ~/workspace/other-repo -- fix the parser bug
/wt-switch-create my-feature
```

## What to do

Before starting the task:

1. **Resolve the target repo**: the repo argument if given. Otherwise the
   session's repo — unless the task's work plainly lives in another repo
   (named as the thing to work on, not merely cited as context); state that
   inference when you use it, and when in doubt stay with the session's repo.
2. **Enter the worktree**: call `EnterWorktree({name: "<branch>"})`. It routes
   through this plugin's `WorktreeCreate` hook, which tries `wt switch
   --create <branch> --no-cd` and falls back to `wt switch <branch> --no-cd`
   if the create fails (e.g. the branch already exists) — new branch,
   existing branch, and existing worktree all land in worktrunk's layout. It
   works in background sessions too. Two special cases:
   - Already inside an `EnterWorktree`-created worktree? It refuses to nest —
     call `ExitWorktree({action: "keep"})` first. If the exit reports there
     is nothing to exit (the isolation came from the spawn, not this
     session), reuse the current worktree and say the requested branch was
     not created.
   - Target repo isn't the session's? Don't call `EnterWorktree`: it has no
     repo parameter, and the harness keeps the shell inside the session's
     working directories, so the session can't be re-rooted into another
     repo. Create the worktree without entering it — `wt -C <repo> switch
     --create <branch> --no-cd`, dropping `--create` if the branch already
     exists — and do the task there, prefixing Bash commands with
     `cd <worktree> && ` (cwd resets between Bash calls, not within one) and
     writing only inside the worktree; report that the session wasn't
     re-rooted and the worktree outlives the session.

   If `EnterWorktree` or the `wt -C` create fails, report the error and stop —
   don't work in the original directory.
3. **Do the task** in the new worktree. If there was no task text, confirm
   the worktree is ready and wait.

## Cleanup

Don't remove the worktree yourself. Removal runs through
`ExitWorktree({action: "remove"})` — which refuses unless re-invoked with
`discard_changes: true` — or the session-exit prompt, and then this plugin's
`WorktreeRemove` hook → `wt remove --foreground`, which fails on dirty
worktrees and retains unmerged branches (removing only their worktree, with a
`wt remove -D <branch>` hint) instead of force-deleting.

## Scope

This command authorizes creating/entering ONE worktree — in the target repo
resolved above — and doing the requested task. Commits, pushes, and merges
still each require explicit user permission.
