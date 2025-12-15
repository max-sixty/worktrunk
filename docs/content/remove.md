+++
title = "wt remove"
weight = 12

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-page` — edit cli.rs to update -->

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

A branch is safe to delete when its content is already reflected in the target. Worktrunk checks four conditions (in order of cost):

1. **Same commit** — Branch HEAD is literally the same commit as target.
2. **No added changes** — Three-dot diff (`main...branch`) shows no files. The branch has no file changes beyond the merge-base (includes "branch is ancestor" case).
3. **Tree contents match** — Branch tree SHA equals main tree SHA. Commit history differs but file contents are identical (e.g., after a revert or merge commit pulling in main).
4. **Merge adds nothing** — Simulated merge (`git merge-tree`) produces the same tree as main. Handles squash-merged branches where main has since advanced.

In `wt list`, `_` indicates same commit, and `⊂` indicates content is integrated. Branches showing either are dimmed as safe to delete.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

## Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--no-background` to run in the foreground.

## Shortcuts

`@` (current), `-` (previous), `^` (main worktree). See [wt switch](@/switch.md#argument-resolution) for argument resolution.

## See also

- [wt merge](@/merge.md) — Remove worktree after merging
- [wt list](@/list.md) — View all worktrees

## Command reference

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

      --no-verify
          Skip hooks

      --force
          Skip approval prompts

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

<!-- END AUTO-GENERATED from `wt remove --help-page` -->
