+++
title = "wt switch"
weight = 10

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-page` — edit cli.rs to update -->

Switch to a worktree. Creates one if needed.

Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-switch.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-switch.gif" alt="wt switch demo" width="1600" height="900">
</picture>
</figure>

## Examples

```bash
wt switch feature-auth           # Switch to worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create new branch and worktree
wt switch --create hotfix --base production
wt switch pr:123                 # Switch to PR #123's branch
```

## Creating a branch

The `--create` flag creates a new branch from the `--base` branch (defaults to default branch). Without `--create`, the branch must already exist.

**Upstream tracking:** Branches created with `--create` have no upstream tracking configured. This prevents accidental pushes to the wrong branch — for example, `--base origin/main` would otherwise make `git push` target `main`. Use `git push -u origin <branch>` to set up tracking when you're ready.

Without `--create`, switching to a remote branch (e.g., `wt switch feature` when only `origin/feature` exists) creates a local branch tracking the remote — this is the standard git behavior and is preserved.

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
| `^` | Default branch (`main`/`master`) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |
| `pr:{N}` | GitHub PR #N's branch |
| `mr:{N}` | GitLab MR !N's branch |

```bash
wt switch -                      # Back to previous
wt switch ^                      # Default branch worktree
wt switch --create fix --base=@  # Branch from current HEAD
wt switch pr:123                 # PR #123's branch
wt switch mr:101                 # MR !101's branch
```

## Interactive picker

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview. The picker requires a TTY.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-select.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-select.gif" alt="wt select demo" width="1600" height="800">
</picture>
</figure>

**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree from query |
| `Esc` | Cancel |
| `1`/`2`/`3`/`4` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

**Preview tabs** (toggle with number keys):

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Diff vs upstream tracking branch (ahead/behind)

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[select]
pager = "delta --paging=never --width=$COLUMNS"
```

Available on Unix only (macOS, Linux). On Windows, use `wt list` or `wt switch <branch>` directly.

## GitHub pull requests (experimental)

The `pr:<number>` syntax resolves the branch for a GitHub pull request. For same-repo PRs, it switches to the branch directly. For fork PRs, it fetches `refs/pull/N/head` and configures `pushRemote` to the fork URL.

```bash
wt switch pr:101                 # Checkout PR #101
```

Requires `gh` CLI to be installed and authenticated. The `--create` flag cannot be used with `pr:` syntax since the branch already exists.

**Fork PRs:** The local branch uses the PR's branch name directly (e.g., `feature-fix`), so `git push` works normally. If a local branch with that name already exists tracking something else, rename it first.

## GitLab merge requests (experimental)

The `mr:<number>` syntax resolves the branch for a GitLab merge request. For same-project MRs, it switches to the branch directly. For fork MRs, it fetches `refs/merge-requests/N/head` and configures `pushRemote` to the fork URL.

```bash
wt switch mr:101                 # Checkout MR !101
```

Requires `glab` CLI to be installed and authenticated. The `--create` flag cannot be used with `mr:` syntax since the branch already exists.

**Fork MRs:** The local branch uses the MR's branch name directly, so `git push` works normally. If a local branch with that name already exists tracking something else, rename it first.

## When wt switch fails

- **Branch doesn't exist** — Use `--create`, or check `wt list --branches`
- **Path occupied** — Another worktree is at the target path; switch to it or remove it
- **Stale directory** — Use `--clobber` to remove a non-worktree directory at the target path

To change which branch a worktree is on, use `git switch` inside that worktree.

## See also

- [`wt list`](@/list.md) — View all worktrees
- [`wt remove`](@/remove.md) — Delete worktrees when done
- [`wt merge`](@/merge.md) — Integrate changes back to the default branch

## Command reference

{% terminal() %}
wt switch - Switch to a worktree

Creates one if needed.

Usage: <b><span class=c>wt switch</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCH]</span> <b><span class=c>[--</span></b> <span class=c>&lt;EXECUTE_ARGS&gt;...</span><b><span class=c>]</span></b>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCH]</span>
          Branch name or shortcut

          Opens interactive picker if omitted. Shortcuts: &#39;^&#39; (default branch),
          &#39;-&#39; (previous), &#39;@&#39; (current), &#39;pr:{N}&#39; (GitHub PR), &#39;mr:{N}&#39; (GitLab
          MR)

  <span class=c>[EXECUTE_ARGS]...</span>
          Additional arguments for --execute command (after --)

          Arguments after <b>--</b> are appended to the execute command. Each argument
          is expanded for templates, then POSIX shell-escaped.

<b><span class=g>Options:</span></b>
      <b><span class=c>--branches</span></b>
          Include branches without worktrees (interactive picker)

      <b><span class=c>--remotes</span></b>
          Include remote branches (interactive picker)

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

          Supports <u>hook template variables</u> (<b>{{ branch }}</b>, <b>{{ worktree_path }}</b>,
          etc.) and filters. <b>{{ base }}</b> and <b>{{ base_worktree_path }}</b> require
          --create.

          Especially useful with shell aliases:

            <b>alias wsc=&#39;wt switch --create -x claude&#39;</b>
            <b>wsc feature-branch -- &#39;Fix GH #322&#39;</b>

          Then <b>wsc feature-branch</b> creates the worktree and launches Claude Code.
          Arguments after <b>--</b> are passed to the command, so <b>wsc feature -- &#39;Fix</b>
          GH #322&#39; runs <b>claude &#39;Fix GH #322&#39;</b>, starting Claude with a prompt.

          Template example: <b>-x &#39;code {{ worktree_path }}&#39;</b> opens VS Code at the
          worktree, <b>-x &#39;tmux new -s {{ branch | sanitize }}&#39;</b> starts a tmux
          session named after the branch.

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

<!-- END AUTO-GENERATED from `wt switch --help-page` -->
