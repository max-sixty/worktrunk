+++
title = "wt step"
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

Run individual git workflow operations: commits, squashes, rebases, and pushes.

## Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Manual merge workflow with review between steps:

```bash
wt step commit
wt step squash
# Review the squashed commit
wt step rebase
wt step push
```

## Operations

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- `for-each` — [experimental] Run a command in every worktree

## Options

### `--stage`

Controls what to stage before committing. Available for `commit` and `squash`:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```bash
wt step commit --stage=tracked
wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
# Inspect the rendered prompt
wt step commit --show-prompt | less

# Pipe to a different LLM
wt step commit --show-prompt | llm -m gpt-5-nano
```

## See also

- [`wt merge`](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [`wt hook`](@/hook.md) — Run configured hooks

## Command reference

{% terminal() %}
wt step - Run individual operations

Usage: <b><span class=c>wt step</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>commit</span></b>    Commit changes with LLM commit message
  <b><span class=c>squash</span></b>    Squash commits since branching
  <b><span class=c>push</span></b>      Fast-forward target to current branch
  <b><span class=c>rebase</span></b>    Rebase onto target
  <b><span class=c>for-each</span></b>  [experimental] Run command in each worktree

<b><span class=g>Options:</span></b>
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

## wt step for-each

Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

### Template variables

All variables are shell-escaped:

| Variable | Description |
|----------|-------------|
| `{{ branch }}` | Branch name (raw, e.g., `feature/auth`) |
| `{{ branch \| sanitize }}` | Branch name with `/` and `\` replaced by `-` |
| `{{ repo }}` | Repository directory name (e.g., `myproject`) |
| `{{ repo_path }}` | Absolute path to repository root |
| `{{ worktree_name }}` | Worktree directory name |
| `{{ worktree_path }}` | Absolute path to current worktree |
| `{{ main_worktree_path }}` | Default branch worktree path |
| `{{ commit }}` | Current HEAD commit SHA (full) |
| `{{ short_commit }}` | Current HEAD commit SHA (7 chars) |
| `{{ default_branch }}` | Default branch name (e.g., "main") |
| `{{ remote }}` | Primary remote name (e.g., "origin") |
| `{{ remote_url }}` | Primary remote URL |
| `{{ upstream }}` | Upstream tracking branch, if configured |

**Deprecated:** `repo_root` (use `repo_path`), `worktree` (use `worktree_path`), `main_worktree` (use `repo`).

### Examples

Check status across all worktrees:

```bash
wt step for-each -- git status --short
```

Run npm install in all worktrees:

```bash
wt step for-each -- npm install
```

Use branch name in command:

```bash
wt step for-each -- "echo Branch: {{ branch }}"
```

Pull updates in worktrees with upstreams (skips others):

```bash
git fetch --prune && wt step for-each -- '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

Note: This command is experimental and may change in future versions.

### Command reference

{% terminal() %}
wt step for-each - [experimental] Run command in each worktree

Usage: <b><span class=c>wt step for-each</span></b> <span class=c>[OPTIONS]</span> <b><span class=c>--</span></b> <span class=c>&lt;ARGS&gt;...</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>&lt;ARGS&gt;...</span>
          Command template (see --help for all variables)

<b><span class=g>Options:</span></b>
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

<!-- END AUTO-GENERATED from `wt step --help-page` -->
