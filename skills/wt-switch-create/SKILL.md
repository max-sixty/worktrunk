---
name: wt-switch-create
description: Create a new worktrunk worktree (optionally in another repo) and switch this session's working directory into it. Use when launching a session that should work in its own worktree (e.g. `/wt-switch-create my-branch -- <task>`, or `/wt-switch-create my-branch ~/workspace/other-repo -- <task>`), or mid-session to move work into a fresh branch.
argument-hint: "<branch-name> [<repo>] [-- task...]"
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev) and this plugin's WorktreeCreate hook
---

Arguments: `$ARGUMENTS`. Grammar: `<branch> [<repo>] [-- <task>]`.

- **branch** — required first token; the branch name for the new worktree.
- **repo** — optional path; create the worktree in this repo instead of the
  session's current one.
- **task** — optional; what to do inside the new worktree. No task means enter
  the worktree and wait.

Without a `--`: a path-shaped second token (absolute, `~`-relative, `./`- or
`../`-relative, or an existing directory) is the repo, and the task starts
after it. Otherwise the task starts at the second token.

```
/wt-switch-create my-feature -- fix the parser bug
/wt-switch-create my-feature ~/workspace/other-repo -- fix the parser bug
/wt-switch-create my-feature
```

## What to do

1. **First action — before reading any files or running any commands:**

   - If a repo was given, `cd` into it first with a `Bash` call (the working
     directory persists for the rest of the session). `EnterWorktree` has no
     repo parameter — it creates the worktree wherever the session is rooted.
   - Then call `EnterWorktree({name: "<branch-name>"})`. This re-roots the
     session into the new worktree. If a repo was given, confirm the new
     worktree landed under it; if not, the `cd` didn't take — report it and
     stop.
   - It works because this plugin maps `WorktreeCreate` →
     `wt switch --create <name> --no-cd --format=json`, so the new worktree
     lands in worktrunk's normal sibling layout (`<repo>.<branch>/`), not under
     `.claude/worktrees/`.
   - `wt switch --create` is idempotent: if the branch already exists, this
     just re-enters its worktree.
   - If you are *already* inside an `EnterWorktree`-created worktree (e.g. the
     background harness isolated this session), **skip `EnterWorktree`** — it
     refuses to nest. Reuse the existing worktree and continue. But if a repo
     was given and that worktree belongs to a different repo, you can't honor
     it — say so and stop rather than running the task in the wrong repo.
   - If `EnterWorktree` fails (not a git repo, invalid branch name, etc.),
     report the error and stop — do not fall back to working in the original
     directory, since that defeats the purpose.

2. After the cwd switch succeeds, do the task in the new worktree. If there was
   no task text, confirm the worktree is ready and wait for the next
   instruction.

## Cleanup

Don't remove the worktree yourself. `ExitWorktree({action: "remove"})` (if the
user asks to leave) or the session-exit prompt routes through this plugin's
`WorktreeRemove` hook → `wt remove --foreground`. A worktree with uncommitted
changes won't be auto-removed without confirmation — that's intended. A branch
with committed-but-unmerged work is retained (with a `wt remove -D <branch>`
hint) instead of being silently force-deleted.

## Scope

This command authorizes creating/entering ONE worktree — in the named repo, if
one was given — and doing the requested task. Commits, pushes, and merges still
each require explicit user permission.
