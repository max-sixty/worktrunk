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

<!-- Maintainers: rationale.md (same directory) covers the harness rules and
design choices behind this — read it before re-adding guards or routes. -->

1. **Pick the branch name** if none was given: short, from the task and
   consistent with existing worktree names, or, mid-session, from the work
   being moved; with nothing to derive from, ask.

2. **Create the worktree** with a `Bash` call (omit `-C <repo>` for this repo):

   ```
   wt -C <repo> switch --create <branch> --no-cd --format=json
   ```

   Stdout is JSON whose `path` field is the worktree's absolute path (status
   lines go to stderr). On `Branch <branch> already exists`: if the user named
   the branch, rerun without `--create` (it enters the branch, creating its
   worktree if missing); if step 1 picked the name, pick another and rerun. Any
   other failure (not a git repo, invalid name): report it and stop.

   Mid-session, carry uncommitted work across: `git stash push -u` before
   creating the worktree, then `git -C <path> stash pop` after (the stash is
   shared across worktrees).

3. **Enter the worktree, then do the task.** Call
   `EnterWorktree({path: "<path from the JSON>"})`.

   - **Accepted** → the session is re-rooted in the worktree. Do the task (or,
     with no task text, confirm it's ready and wait).
   - **Rejected** → the worktree is in another repo (`EnterWorktree` re-roots
     only within the repo your cwd is in; the rejection is graceful and creates
     nothing). You can still `cd` into the worktree and work there if it's
     inside an allowed directory — an entry in `permissions.additionalDirectories`,
     such as `~/workspace`. So `cd <path>` and read the result:
     - no `Shell cwd was reset` notice → it stuck; the worktree is reachable.
       Work there and do the task.
     - `Shell cwd was reset` → not reachable. Stop and ask the user to make it
       reachable: add the repo, or a parent like `~/workspace`, to
       `permissions.additionalDirectories` (durable, every session), or run
       `/add-dir <path>` (this session). Then continue. Don't grind through
       absolute paths with `cd` resetting on every command.

## Cleanup

The worktree is a normal worktrunk worktree: it persists after the session
ends, shows up in `wt list`, and is merged or removed with `wt merge` /
`wt remove <branch>` like any other. Don't remove it unprompted. If the user
asks to leave mid-session, `ExitWorktree({action: "keep"})` returns the
session to its original directory; `ExitWorktree` cannot remove a worktree
entered by `path`, so removal is always `wt remove <branch>`.

## Scope

This command authorizes creating/entering ONE worktree (in the named repo, if
one was given) and doing the requested task. Commits, pushes, and merges still
each require explicit user permission.
