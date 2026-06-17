---
name: wt-switch-create
description: Create a new worktrunk worktree (optionally in another repo) and switch this session's working directory into it. Use when launching a session that should work in its own worktree.
argument-hint: "[<branch>] [<repo>] [-- <task>]"
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev)
---

Arguments: `$ARGUMENTS`. Grammar: `[<branch>] [<repo>] [-- <task>]`.

- **branch** — optional; the branch name for the new worktree. When omitted,
  pick one (step 1 below).
- **repo** — optional path; create the worktree in this repo instead of the
  session's current one.
- **task** — optional; what to do inside the new worktree. No task means enter
  the worktree and wait.

Tokens before the `--` are the branch and/or repo: a path-shaped token
(starting with `/`, `~`, `./`, or `../`) is the repo; any other token is the
branch (`docs` is a branch name, never the `docs/` directory). More than one
branch-shaped token before a `--` doesn't fit the grammar — ask. Without a
`--`, judge where the task starts: leading tokens that read as a branch name
(`fix-auth`) or a repo path are consumed as such, and the rest is the task;
otherwise the whole input is the task (`fix the parser bug` has no
branch-shaped lead — all task).

```
/wt-switch-create my-feature -- fix the parser bug
/wt-switch-create -- fix the parser bug
/wt-switch-create my-feature ~/workspace/other-repo -- fix the parser bug
/wt-switch-create my-feature
```

## What to do

Steps 1–3 come before any other work.

<!-- Maintainers: the design choices here are backed by tested evidence in
rationale.md (same directory) — read it before re-adding guards or routes. -->

1. **Pick the branch name** if none was given: short, from the task ("fix the
   parser bug" → `fix-parser-bug`) and consistent with existing worktree
   names, or, mid-session, from the work being moved; with nothing to derive
   from, ask.

2. **Create the worktree** with a `Bash` call (omit `-C <repo>` when no repo
   was given):

   ```
   wt -C <repo> switch --create <branch> --no-cd --format=json
   ```

   Stdout is JSON whose `path` field is the worktree's absolute path (status
   lines go to stderr). On `Branch <branch> already exists`:

   - the user named the branch → rerun the same command without `--create`;
     it enters the existing branch, creating its worktree if missing.
   - the name was picked in step 1 → the user never chose that branch; pick
     another name and rerun.

   Any other failure (not a git repo, invalid name): report it and stop — do
   not do the task in the original directory.

   Mid-session, when the work to move is uncommitted in the current worktree,
   carry it across: `git stash push -u` before creating the worktree, then
   `git -C <path> stash pop` after (the stash is shared across worktrees).

3. **Re-root the session** with `EnterWorktree({path: "<path from the JSON>"})`.

   If it is rejected (worktree in a different repo, session already in a
   worktree, pinned cwd — the rejections are graceful and create nothing),
   leave the session rooted where it is and work in the worktree through
   absolute paths instead; name the worktree path when reporting back. Don't
   try to `cd` there: the harness resets `cd` that leaves the session's
   working directories, and `EnterWorktree` is the supported way to move a
   session.

4. **Do the task** in the worktree. If there was no task text, confirm the
   worktree is ready and wait for the next instruction.

## Cleanup

The worktree is a normal worktrunk worktree: it persists after the session
ends, shows up in `wt list`, and is merged or removed with `wt merge` /
`wt remove <branch>` like any other. Don't remove it unprompted. If the user
asks to leave mid-session, `ExitWorktree({action: "keep"})` returns the
session to its original directory; `ExitWorktree` cannot remove a worktree
entered by `path`, so removal is always `wt remove <branch>`.

## Scope

This command authorizes creating/entering ONE worktree — in the named repo, if
one was given — and doing the requested task. Commits, pushes, and merges still
each require explicit user permission.
