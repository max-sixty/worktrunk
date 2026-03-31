+++
title = "wt remove"
description = "Remove worktree; delete branch if merged. Defaults to the current worktree."
weight = 12

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-page` — edit cli.rs to update -->

Remove worktree; delete branch if merged. Defaults to the current worktree.

## Examples

Remove current worktree:

{{ terminal(cmd="wt remove") }}

Remove specific worktrees / branches:

{{ terminal(cmd="wt remove feature-branch|||wt remove old-feature another-branch") }}

Keep the branch:

{{ terminal(cmd="wt remove --no-delete-branch feature-branch") }}

Force-delete an unmerged branch:

{{ terminal(cmd="wt remove -D experimental") }}

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

{{ terminal(cmd="wt remove feature --force       # Remove worktree with untracked files|||wt remove feature -D            # Delete unmerged branch|||wt remove feature --force -D    # Both") }}

Without `--force`, removal fails if the worktree contains untracked files. Without `--force-delete`, removal keeps branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of merge status.

## Background removal

Removal runs in the background by default — the command returns immediately. Logs are written to `.git/wt/logs/{branch}-remove.log`. Use `--foreground` to run in the foreground.

## Hooks

`pre-remove` hooks run before the worktree is deleted (with access to worktree files). `post-remove` hooks run after removal. See [`wt hook`](@/hook.md) for configuration.

## Detached HEAD worktrees

Detached worktrees have no branch name. Pass the worktree path instead: `wt remove /path/to/worktree`.

## See also

- [`wt merge`](@/merge.md) — Remove worktree after merging
- [`wt list`](@/list.md) — View all worktrees

## Command reference

{% terminal() %}
wt remove - Remove worktree; delete branch if merged

Defaults to the current worktree.

Usage: <b><span class=c>wt remove</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCHES]...</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCHES]...</span>
          Branch name [default: current]

<b><span class=g>Options:</span></b>
      <b><span class=c>--no-delete-branch</span></b>
          Keep branch after removal

  <b><span class=c>-D</span></b>, <b><span class=c>--force-delete</span></b>
          Delete unmerged branches

      <b><span class=c>--foreground</span></b>
          Run removal in foreground (block until complete)

  <b><span class=c>-f</span></b>, <b><span class=c>--force</span></b>
          Force worktree removal

          Remove worktrees even if they contain untracked files (like build artifacts). Without this
          flag, removal fails if untracked files exist.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Automation:</span></b>
  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

      <b><span class=c>--no-verify</span></b>
          Skip hooks

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

<!-- END AUTO-GENERATED from `wt remove --help-page` -->
