# wt remove

Remove worktree; delete branch if merged. Defaults to the current worktree.

## Examples

Remove current worktree:

```
$ wt remove
◎ Running pre-remove project:cleanup
  flyctl scale count 0
Scaling app to 0 machines
◎ Removing api worktree & branch in background (same commit as main, _)
○ Switched to worktree for main @ ~/repo
```

Remove specific worktrees / branches:

```bash
$ wt remove feature-branch
$ wt remove old-feature another-branch
```

Keep the branch:

```bash
$ wt remove --no-delete-branch feature-branch
```

Force-delete an unmerged branch:

```bash
$ wt remove -D experimental
```

## Branch cleanup

By default, branches are deleted when they would add no changes to the default branch if merged. This works with both unchanged git histories, and squash-merge or rebase workflows where commit history differs but file changes match.

Worktrunk checks six conditions (in order of cost):

1. **Same commit** — Branch HEAD equals the default branch. Shows `_` in `wt list`.
2. **Ancestor** — Branch is in target's history (fast-forward or rebase case). Shows `⊂`.
3. **No added changes** — Three-dot diff (`target...branch`) is empty. Shows `⊂`.
4. **Trees match** — Branch tree SHA equals target tree SHA. Shows `⊂`.
5. **Merge adds nothing** — Simulated merge produces the same tree as target. Handles squash-merged branches where target has advanced with changes to different files. Shows `⊂`.
6. **Patch-id match** — Branch's entire diff matches a single squash-merge commit on target. Fallback for when the simulated merge conflicts because target later modified the same files the branch touched. Shows `⊂`.

The default-branch walk is capped so a single check stays fast; a squash merge with hundreds of commits landed since the merge point falls outside the cap and needs `-D` to remove.

The 'same commit' check uses the local default branch; for other checks, 'target' means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches matching these conditions and with empty working trees are dimmed in `wt list` as safe to delete.

## Force flags

Worktrunk has two force flags for different situations:

| Flag | Scope | When to use |
|------|-------|-------------|
| `--force` (`-f`) | Worktree | Worktree has uncommitted changes |
| `--force-delete` (`-D`) | Branch | Branch has unmerged commits |

```bash
$ wt remove feature --force       # Remove dirty worktree
$ wt remove feature -D            # Delete unmerged branch
$ wt remove feature --force -D    # Both
```

Use `--no-delete-branch` to keep the branch regardless of merge status.

## Background removal

Removal runs in the background by default — the command returns immediately. The worktree is renamed into `.git/wt/trash/` (instant same-filesystem rename), git metadata is pruned, the branch is deleted, and a detached `rm -rf` finishes cleanup. Cross-filesystem worktrees fall back to `git worktree remove`. Logs: `.git/wt/logs/{branch}/internal/remove.log`. Use `--foreground` to run in the foreground.

After each `wt remove`, entries in `.git/wt/trash/` older than 24 hours are swept by a detached `rm -rf` — eventual cleanup for directories orphaned when a previous background removal was interrupted (SIGKILL, reboot, disk full).

## Reaping processes [experimental]

`--reap` terminates processes left running in the worktree before it is removed — a `post-start` dev server, a file watcher, a language server — freeing the ports and file handles they hold. Processes are discovered by working directory: any process whose current directory is at or under the worktree path (`SIGTERM`, then `SIGKILL` for survivors).

```bash
$ wt remove --reap feature
◎ Reaping 2 processes under feature worktree
   ┃ 51234 node
   ┃ 51240 esbuild
✓ Reaped 2 processes
◎ Removing feature worktree & branch in background (same commit as main, _)
```

To avoid killing work the user did not mean to kill, two guards keep `--reap` conservative:

- **Interactive processes are spared.** A process holding a controlling terminal — an interactive shell, or a terminal editor such as `vim` with unsaved buffers — is never reaped. Only detached processes remain candidates.
- **Discovery is by working directory only.** A process that started in the worktree and later changed directory, or a daemon that reparented to `init`, no longer reports a directory under the worktree and is not found. To reliably reap those, launch them with [`wt step tether`](https://worktrunk.dev/step/#wt-step-tether), which kills the whole process group when the worktree is removed.

Reaping runs before the worktree directory is touched, so it is independent of foreground/background removal and the `--force` flag. Unix only; on Windows `--reap` is rejected.

## Hooks

`pre-remove` hooks run before the worktree is deleted (with access to worktree files). `post-remove` hooks run after removal. See [`wt hook`](https://worktrunk.dev/hook/) for configuration.

## Detached HEAD worktrees

Detached worktrees have no branch name. Pass the worktree path instead: `wt remove /path/to/worktree`.

## Command reference

```
wt remove - Remove worktree; delete branch if merged

Defaults to the current worktree.

Usage: wt remove [OPTIONS] [BRANCHES]...

Arguments:
  [BRANCHES]...
          Branch name or worktree path [default: current]

Options:
      --no-delete-branch
          Keep branch after removal

  -D, --force-delete
          Delete unmerged branches

      --foreground
          Run removal in foreground (block until complete)

      --reap
          Kill processes started in the worktree [experimental]

          Before removal, terminate processes whose working directory is under the worktree — dev
          servers, watchers, language servers. Processes holding a controlling terminal (interactive
          shells, terminal editors) are left alone. Unix only.

  -f, --force
          Force worktree removal

          Remove a dirty worktree, including staged, modified, and untracked files. Without this
          flag, removal fails if the worktree has any uncommitted changes.

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --no-hooks
          Skip hooks

      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after removal completes.

          [default: text]
          [possible values: text, json]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```
