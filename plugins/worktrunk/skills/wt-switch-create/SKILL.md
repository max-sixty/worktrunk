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

Creating the worktree comes first on every invocation, before any other work.
The invocation is itself the explicit request to create it; a research or
read-only task gets one all the same.

<!-- Maintainers: rationale.md (same directory) covers the harness rules and
design choices behind this — read it before re-adding guards or routes. -->

1. **Pick the branch name** if none was given: short, from the task and
   consistent with existing worktree names, or, mid-session, from the work
   being moved; with nothing to derive from, ask.

2. **With no repo argument, create and enter in one call:**
   `EnterWorktree({name: "<branch>"})`. Worktrunk's `WorktreeCreate` hook runs
   `wt switch --create`, so the result is an ordinary `wt` worktree in the
   default layout, and the user sees no confirmation prompt. On success,
   do the task (or, with no task text, confirm it's ready and wait).

   Mid-session, carry uncommitted work across: `git stash push -u` before the
   `EnterWorktree` call, then `git stash pop` after — the call re-roots the
   session into the new worktree, and the stash is shared across worktrees.

3. **Otherwise create it with `wt` and enter by path.** Two cases reach here: a
   repo argument, which step 2 can't target, and a failed step 2, whose error
   says which — `✗ Branch <branch> already exists`, or `Already in a worktree
   session`. Create with a `Bash` call (omit `-C <repo>` for this repo):

   ```
   wt -C <repo> switch --create <branch> --no-cd --format=json
   ```

   Stdout is JSON whose `path` field is the worktree's absolute path (status
   lines go to stderr). On `Branch <branch> already exists`: if the user named
   the branch, rerun without `--create` (it enters the branch, creating its
   worktree if missing); if step 1 picked the name, pick another and rerun. Any
   other failure (not a git repo, invalid name): report it and stop.

   Then call `EnterWorktree({path: "<path from the JSON>"})`.

   - **Accepted** → the session is re-rooted in the worktree. Do the task (or,
     with no task text, confirm it's ready and wait).
   - **Tool error** — the tool ran and returned an error (`Cannot enter
     worktree: …`) → graceful; nothing moved, and one recovery covers them
     all. Common causes: the cwd resolves to no git repo (e.g. a non-git
     parent like `~/workspace` that only holds repos, as in a background job)
     or to a different repo than the target; or the session is already rooted
     in a worktree (or is a pinned agent), which limits entry to the current
     repo's `.claude/worktrees/` and excludes even a same-repo `wt` sibling.
     The recovery test is whether you can `cd` into the worktree, which works
     when it's inside an allowed directory. So `cd <path>` and read the
     result:
     - no `Shell cwd was reset` notice → it stuck; the worktree is reachable.
       Work there, but a bare `cd` is not a tracked re-root, so the cwd can
       revert to the session's launch worktree across turns (and in spawned
       subagents); pin commands with `git -C <path>` / `wt -C <path>` rather
       than trusting the `cd` to persist.
     - `Shell cwd was reset` → not reachable. Stop and ask the user to make it
       reachable: add the repo, or a parent like `~/workspace`, to
       `permissions.additionalDirectories` (durable, every session), or run
       `/add-dir <path>` (this session). Then continue. Don't grind through
       absolute paths with `cd` resetting on every command.
   - **Denied** — the call itself was refused, with no tool error → however
     the denial is worded, it is the user's answer to the confirmation Claude
     Code shows for entering a worktree outside `.claude/worktrees/`, unless
     there was no user to ask (the denial says the session couldn't prompt),
     which decides nothing — take the recovery above. On the user's answer:
     the worktree `wt` just created still exists; only the entry didn't
     happen. Report its path and ask how to proceed, since reaching it
     through `cd` would override that answer.

## Cleanup

The worktree is a normal worktrunk worktree: it shows up in `wt list` and is
merged or removed with `wt merge` / `wt remove <branch>` like any other. Don't
remove it unprompted.

A worktree from step 2 that the session never touched — no changed files, no
commits — is cleaned up when the session ends, branch included; anything
written into it keeps it. A worktree from step 3 always stays. If the user asks
to leave mid-session, `ExitWorktree({action: "keep"})` returns the session to
its original directory;
`ExitWorktree` cannot remove a worktree entered by `path`, so removing one of
those is always `wt remove <branch>`.

## Scope

The command's mandate is ONE worktree (in the named repo, if one was given)
and the requested task inside it. Commits, pushes, and merges still each
require explicit user permission.
