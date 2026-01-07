+++
title = "wt switch"
weight = 10

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-page` — edit cli.rs to update -->

Change directory to a worktree, creating one if needed.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-switch.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-switch.gif" alt="wt switch demo" width="1600" height="900">
</picture>
</figure>

Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

## Examples

```bash
wt switch feature-auth           # Switch to worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create new branch and worktree
wt switch --create hotfix --base production
```

## Creating a branch

The `--create` flag creates a new branch from the `--base` branch (defaults to default branch). Without `--create`, the branch must already exist.

## Creating worktrees

If the branch already has a worktree, `wt switch` changes directories to it. Otherwise, it creates one, running [hooks](@/hook.md).

When creating a worktree, worktrunk:

1. Creates worktree at configured path
2. Switches to new directory
3. Runs [post-create hooks](@/hook.md#post-create) (blocking)
4. Spawns [post-start hooks](@/hook.md#post-start) (background)

```bash
wt switch feature                        # Existing branch → creates worktree
wt switch --create feature               # New branch and worktree
wt switch --create fix --base release    # New branch from release
wt switch --create temp --no-verify      # Skip hooks
```

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (main/master) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |

```bash
wt switch -                      # Back to previous
wt switch ^                      # Default branch worktree
wt switch --create fix --base=@  # Branch from current HEAD
```

## When wt switch fails

- **Branch doesn't exist** — Use `--create`, or check `wt list --branches`
- **Path occupied** — Another worktree is at the target path; switch to it or remove it
- **Stale directory** — Use `--clobber` to remove a non-worktree directory at the target path

To change which branch a worktree is on, use `git switch` inside that worktree.

## See also

- [`wt select`](@/select.md) — Interactive worktree selection
- [`wt list`](@/list.md) — View all worktrees
- [`wt remove`](@/remove.md) — Delete worktrees when done
- [`wt merge`](@/merge.md) — Integrate changes back to the default branch

## Command reference

{% terminal() %}
wt switch - Switch to a worktree

Usage: <b><span class=c>wt switch</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;BRANCH&gt;</span> <b><span class=c>[--</span></b> <span class=c>&lt;EXECUTE_ARGS&gt;...</span><b><span class=c>]</span></b>

<b><span class=g>Arguments:</span></b>
  <span class=c>&lt;BRANCH&gt;</span>
          Branch name

          Shortcuts: &#39;^&#39; (default branch), &#39;-&#39; (previous), &#39;@&#39; (current)

  <span class=c>[EXECUTE_ARGS]...</span>
          Additional arguments for --execute command (after --)

          Arguments after <b>--</b> are appended to the execute command. Each argument
          is POSIX shell-escaped before appending.

<b><span class=g>Options:</span></b>
  <b><span class=c>-c</span></b>, <b><span class=c>--create</span></b>
          Create a new branch

  <b><span class=c>-b</span></b>, <b><span class=c>--base</span></b><span class=c> &lt;BASE&gt;</span>
          Base branch

          Defaults to default branch.

  <b><span class=c>-x</span></b>, <b><span class=c>--execute</span></b><span class=c> &lt;EXECUTE&gt;</span>
          Command to run after switch

          Replaces the wt process with the command after switching, giving it
          full terminal control. Useful for launching editors, AI agents, or
          other interactive tools.

          Especially useful with shell aliases:

            <b>alias wsc=&#39;wt switch --create -x claude&#39;</b>
            <b>wsc feature-branch -- &#39;Fix GH #322&#39;</b>

          Then <b>wsc feature-branch</b> creates the worktree and launches Claude Code.
          Arguments after <b>--</b> are passed to the command, so <b>wsc feature -- &#39;Fix</b>
          GH #322&#39; runs <b>claude &#39;Fix GH #322&#39;</b>, starting Claude with a prompt.

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

      <b><span class=c>--clobber</span></b>
          Remove stale paths at target

      <b><span class=c>--no-verify</span></b>
          Skip hooks

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

<!-- END AUTO-GENERATED from `wt switch --help-page` -->
