+++
title = "wt remove"
weight = 12

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-page` — edit cli.rs to update -->

Removes worktrees and their branches (if merged), returning to the main worktree. Defaults to removing the current worktree.

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

By default, branches are deleted when merging them would add nothing. This works with squash-merge and rebase workflows where commit history differs but file changes match.

Worktrunk checks five conditions (in order of cost):

1. **Same commit** — Branch HEAD equals the default branch. Shows `_` in `wt list`.
2. **Ancestor** — Branch is in target's history (fast-forward or rebase case). Shows `⊂`.
3. **No added changes** — Three-dot diff (`target...branch`) is empty. Shows `⊂`.
4. **Trees match** — Branch tree SHA equals target tree SHA. Shows `⊂`.
5. **Merge adds nothing** — Simulated merge produces the same tree as target. Handles squash-merged branches where target has advanced. Shows `⊂`.

The "same commit" check uses the local default branch; for other checks, **target** means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches showing `_` or `⊂` are dimmed as safe to delete.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

## Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--no-background` to run in the foreground.

## Shortcuts

`@` (current), `-` (previous), `^` (default branch). See [`wt switch`](@/switch.md#shortcuts).

## See also

- [`wt merge`](@/merge.md) — Remove worktree after merging
- [`wt list`](@/list.md) — View all worktrees

## Command reference

{% terminal() %}
wt remove - Remove worktree; delete branch if merged

Usage: <b><span class=c>wt remove</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCHES]...</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCHES]...</span>
          Branch name [default: current]

<b><span class=g>Options:</span></b>
      <b><span class=c>--no-delete-branch</span></b>
          Keep branch after removal

  <b><span class=c>-D</span></b>, <b><span class=c>--force-delete</span></b>
          Delete unmerged branches

      <b><span class=c>--no-background</span></b>
          Run removal in foreground

      <b><span class=c>--no-verify</span></b>
          Skip hooks

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

  <b><span class=c>-f</span></b>, <b><span class=c>--force</span></b>
          Force worktree removal

          Remove worktrees even if they contain untracked files (like build
          artifacts). Without this flag, removal fails if untracked files exist.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

<!-- END AUTO-GENERATED from `wt remove --help-page` -->
