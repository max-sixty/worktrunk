+++
title = "wt remove"
weight = 12

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-page` — edit src/cli.rs to update -->

Removes worktrees and their branches. Without arguments, removes the current worktree and returns to the main worktree.

## Examples

Remove current worktree:

```bash
wt remove
```

Remove specific worktrees:

```bash
wt remove feature-branch
wt remove old-feature another-branch
```

Keep the branch:

```bash
wt remove --no-delete-branch feature-branch
```

Force-delete an unmerged branch:

```bash
wt remove -D experimental
```

## Branch cleanup

Branches delete automatically when their content is already in the target branch (typically main). This works with squash-merge and rebase workflows where commit history differs but file changes match.

Use `-D` to force-delete unmerged branches. Use `--no-delete-branch` to keep the branch.

## Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--no-background` to run in the foreground.

Arguments resolve by path first, then branch name—see [wt switch](@/switch.md#path-first-lookup). Shortcuts: `@` (current), `-` (previous), `^` (main worktree).

## See also

- [wt merge](@/merge.md) — Remove worktree after merging
- [wt list](@/list.md) — View all worktrees

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-page` — edit cli.rs to update -->

```
wt remove - Remove worktree and branch
Usage: wt remove [OPTIONS] [WORKTREES]...

Arguments:
  [WORKTREES]...
          Worktree or branch (@ for current)

Options:
      --no-delete-branch
          Keep branch after removal

  -D, --force-delete
          Delete unmerged branches

      --no-background
          Run removal in foreground

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```
