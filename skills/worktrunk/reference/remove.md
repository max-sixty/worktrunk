# wt remove

Remove worktree; delete branch if merged. Defaults to the current worktree.

## Examples

Remove current worktree:

```bash
$ wt remove
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

The 'same commit' check uses the local default branch; for other checks, 'target' means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches matching these conditions and with empty working trees are dimmed in `wt list` as safe to delete.

## Force flags

Worktrunk has two force flags for different situations:

| Flag | Scope | When to use |
|------|-------|-------------|
| `--force` (`-f`) | Worktree | Worktree has untracked files |
| `--force-delete` (`-D`) | Branch | Branch has unmerged commits |

```bash
$ wt remove feature --force       # Remove worktree with untracked files
$ wt remove feature -D            # Delete unmerged branch
$ wt remove feature --force -D    # Both
```

Without `--force`, removal fails if the worktree contains untracked files. Without `--force-delete`, removal keeps branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of merge status.

## Background removal

Removal runs in the background by default — the command returns immediately. The worktree is renamed into `.git/wt/trash/` (instant same-filesystem rename), git metadata is pruned, the branch is deleted, and a detached `rm -rf` finishes cleanup. Cross-filesystem worktrees fall back to `git worktree remove`. Logs: `.git/wt/logs/{branch}-remove.log`. Use `--foreground` to run in the foreground.

After each `wt remove`, entries in `.git/wt/trash/` older than 24 hours are swept by a detached `rm -rf` — eventual cleanup for directories orphaned when a previous background removal was interrupted (SIGKILL, reboot, disk full).

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
          Branch name [default: current]

Options:
      --no-delete-branch
          Keep branch after removal

  -D, --force-delete
          Delete unmerged branches

      --foreground
          Run removal in foreground (block until complete)

  -f, --force
          Force worktree removal

          Remove worktrees even if they contain untracked files (like build artifacts). Without this
          flag, removal fails if untracked files exist.

  -h, --help
          Print help (see a summary with '-h')

Automation:
  -y, --yes
          Skip approval prompts

      --no-hooks
          Skip hooks

      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after removal completes.

          Possible values:
          - text: Human-readable text output
          - json: JSON output

          [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```
