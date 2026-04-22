+++
title = "wt switch"
description = "Switch to a worktree; create if needed."
weight = 10

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-page` — edit cli.rs to update -->

Switch to a worktree; create if needed.

Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-switch.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-switch.gif" alt="wt switch demo" width="1600" height="900">
</picture>
</figure>

## Examples

{{ terminal(cmd="wt switch feature-auth           # Switch to worktree|||wt switch -                      # Previous worktree (like cd -)|||wt switch --create new-feature   # Create new branch and worktree|||wt switch --create hotfix --base production|||wt switch pr:123                 # Switch to PR #123's branch") }}

## Creating a branch

The `--create` flag creates a new branch from `--base` — the default branch unless specified. Without `--create`, the branch must already exist. Switching to a remote branch (e.g., `wt switch feature` when only `origin/feature` exists) creates a local tracking branch.

## Creating worktrees

If the branch already has a worktree, `wt switch` changes directories to it. Otherwise, it creates one:

1. Runs [pre-switch hooks](@/hook.md#hook-types), blocking until complete
2. Creates worktree at configured path
3. Switches to new directory
4. Runs [pre-start hooks](@/hook.md#hook-types), blocking until complete
5. Spawns [post-start](@/hook.md#hook-types) and [post-switch hooks](@/hook.md#hook-types) in the background

{{ terminal(cmd="wt switch feature                        # Existing branch → creates worktree|||wt switch --create feature               # New branch and worktree|||wt switch --create fix --base release    # New branch from release|||wt switch --create temp --no-hooks       # Skip hooks") }}

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (`main`/`master`) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |
| `pr:{N}` | GitHub PR #N's branch |
| `mr:{N}` | GitLab MR !N's branch |

{{ terminal(cmd="wt switch -                           # Back to previous|||wt switch ^                           # Default branch worktree|||wt switch --create fix --base=@       # Branch from current HEAD|||wt switch --create fix --base=pr:123  # Branch from PR #123's head|||wt switch pr:123                      # PR #123's branch|||wt switch mr:101                      # MR !101's branch") }}

Shortcuts also apply to `--base`. For a fork PR/MR, the head commit is fetched and used as the base SHA without creating a tracking branch.

## Interactive picker

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-switch-picker.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-switch-picker.gif" alt="wt switch picker demo" width="1600" height="800">
</picture>
</figure>

**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree named as entered text |
| `Esc` | Cancel |
| `1`–`5` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |
<!-- Alt-r (remove worktree) works but is omitted: cursor resets after skim reload (#1695). Add once fixed. See #1881. -->

**Preview tabs** — toggle with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Ahead/behind diff vs upstream tracking branch
5. **summary** — LLM-generated branch summary; requires `[list] summary = true` and `[commit.generation]`

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[switch.picker]
pager = "delta --paging=never --width=$COLUMNS"
```

Available on Unix only (macOS, Linux). On Windows, use `wt list` or `wt switch <branch>` directly.

## Pull requests and merge requests

The `pr:<number>` and `mr:<number>` shortcuts resolve a GitHub PR or GitLab MR to its branch. For same-repo PRs/MRs, worktrunk switches to the branch directly. For fork PRs/MRs, it fetches the ref (`refs/pull/N/head` or `refs/merge-requests/N/head`) and configures `pushRemote` to the fork URL.

{{ terminal(cmd="wt switch pr:101                 # GitHub PR #101|||wt switch mr:101                 # GitLab MR !101") }}

Requires `gh` (GitHub) or `glab` (GitLab) CLI to be installed and authenticated. The `--create` flag cannot be used with `pr:`/`mr:` syntax since the branch already exists.

**Forks:** The local branch uses the PR/MR's branch name directly (e.g., `feature-fix`), so `git push` works normally. If a local branch with that name already exists tracking something else, rename it first.

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
wt switch - Switch to a worktree; create if needed

Usage: <b><span class=c>wt switch</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCH]</span> <b><span class=c>[--</span></b> <span class=c>&lt;EXECUTE_ARGS&gt;...</span><b><span class=c>]</span></b>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCH]</span>
          Branch name or shortcut

          Opens interactive picker if omitted. Shortcuts: &#39;^&#39; (default branch), &#39;-&#39; (previous), &#39;@&#39;
          (current), &#39;pr:{N}&#39; (GitHub PR), &#39;mr:{N}&#39; (GitLab MR)

  <span class=c>[EXECUTE_ARGS]...</span>
          Additional arguments for --execute command (after --)

          Arguments after <b>--</b> are appended to the execute command. Each argument is expanded for
          templates, then POSIX shell-escaped.

<b><span class=g>Options:</span></b>
  <b><span class=c>-c</span></b>, <b><span class=c>--create</span></b>
          Create a new branch

  <b><span class=c>-b</span></b>, <b><span class=c>--base</span></b><span class=c> &lt;BASE&gt;</span>
          Base branch

          Defaults to default branch. Supports the same shortcuts as the branch argument: <b>^</b>, <b>@</b>, <b>-</b>,
<b>          pr:{N}</b>, <b>mr:{N}</b>.

  <b><span class=c>-x</span></b>, <b><span class=c>--execute</span></b><span class=c> &lt;EXECUTE&gt;</span>
          Command to run after switch

          Replaces the wt process with the command after switching, giving it full terminal control.
          Useful for launching editors, AI agents, or other interactive tools.

          Supports <u>hook template variables</u> (<b>{{ branch }}</b>, <b>{{ worktree_path }}</b>, etc.) and filters. <b>{{</b>
<b>          base }}</b> and <b>{{ base_worktree_path }}</b> require <b>--create</b>.

          Especially useful with shell aliases:

            <b>alias wsc=&#39;wt switch --create -x claude&#39;</b>
            <b>wsc feature-branch -- &#39;Fix GH #322&#39;</b>

          Then <b>wsc feature-branch</b> creates the worktree and launches Claude Code. Arguments after <b>--</b>
          are passed to the command, so <b>wsc feature -- &#39;Fix GH #322&#39;</b> runs <b>claude &#39;Fix GH #322&#39;</b>,
          starting Claude with a prompt.

          Template example: <b>-x &#39;code {{ worktree_path }}&#39;</b> opens VS Code at the worktree, <b>-x &#39;tmux</b>
<b>          new -s {{ branch | sanitize }}&#39;</b> starts a tmux session named after the branch.

      <b><span class=c>--clobber</span></b>
          Remove stale paths at target

      <b><span class=c>--no-cd</span></b>
          Skip directory change after switching

          Hooks still run normally. Useful when hooks handle navigation (e.g., tmux workflows) or
          for CI/automation. Use --cd to override.

          In picker mode (no branch argument), prints the selected branch name and exits without
          switching. Useful for scripting.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Picker Options:</span></b>
      <b><span class=c>--branches</span></b>
          Include branches without worktrees

      <b><span class=c>--remotes</span></b>
          Include remote branches

<b><span class=g>Automation:</span></b>
      <b><span class=c>--no-hooks</span></b>
          Skip hooks

      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format

          JSON prints structured result to stdout. Designed for tool integration (e.g., Claude Code
          WorktreeCreate hooks).

          Possible values:
          - <b><span class=c>text</span></b>: Human-readable text output
          - <b><span class=c>json</span></b>: JSON output

          [default: text]

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: info logs + hook/alias template variable &amp; output; -vv: debug logs +
          diagnostic report + trace.log/output.log under .git/wt/logs/)

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts
{% end %}

<!-- END AUTO-GENERATED from `wt switch --help-page` -->
