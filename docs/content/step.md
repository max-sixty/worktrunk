+++
title = "wt step"
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

Run individual operations. The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities.

## Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Manual merge workflow with review between steps:

```bash
wt step commit
wt step squash
wt step rebase
wt step push
```

## Operations

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- `copy-ignored` — Copy gitignored files between worktrees
- `for-each` — [experimental] Run a command in every worktree

## See also

- [`wt merge`](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [`wt hook`](@/hook.md) — Run configured hooks

## Command reference

{% terminal() %}
wt step - Run individual operations

The building blocks of <b>wt merge</b> — commit, squash, rebase, push — plus standalone
utilities.

Usage: <b><span class=c>wt step</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>commit</span></b>        Commit changes with LLM commit message
  <b><span class=c>squash</span></b>        Squash commits since branching
  <b><span class=c>push</span></b>          Fast-forward target to current branch
  <b><span class=c>rebase</span></b>        Rebase onto target
  <b><span class=c>copy-ignored</span></b>  Copy gitignored files to another worktree
  <b><span class=c>for-each</span></b>      [experimental] Run command in each worktree

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

## wt step commit

Stages all changes (including untracked files) and commits with an [LLM-generated message](@/llm-commits.md).

### Options

#### `--stage`

Controls what to stage before committing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```bash
wt step commit --stage=tracked
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
# Inspect the rendered prompt
wt step commit --show-prompt | less

# Pipe to a different LLM
wt step commit --show-prompt | llm -m gpt-5-nano
```

### Command reference

{% terminal() %}
wt step commit - Commit changes with LLM commit message

Stages working tree changes and commits with an LLM-generated message.

Usage: <b><span class=c>wt step commit</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

      <b><span class=c>--no-verify</span></b>
          Skip hooks

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked
            changes
          - <b><span class=c>tracked</span></b>: Stage tracked changes only (like <b>git add -u</b>)
          - <b><span class=c>none</span></b>:    Stage nothing, commit only what&#39;s already in the index

      <b><span class=c>--show-prompt</span></b>
          Show prompt without running LLM

          Outputs the rendered prompt to stdout for debugging or manual piping.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

## wt step squash

Stages all changes (including untracked files), then squashes all commits since diverging from the target branch into a single commit with an [LLM-generated message](@/llm-commits.md).

### Options

#### `--stage`

Controls what to stage before squashing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, squash only committed changes |

```bash
wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
wt step squash --show-prompt | less
```

### Command reference

{% terminal() %}
wt step squash - Squash commits since branching

Stages working tree changes, squashes all commits since diverging from target
into one, generates message with LLM.

Usage: <b><span class=c>wt step squash</span></b> <span class=c>[OPTIONS]</span> <span class=c>[TARGET]</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[TARGET]</span>
          Target branch

          Defaults to default branch.

<b><span class=g>Options:</span></b>
  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

      <b><span class=c>--no-verify</span></b>
          Skip hooks

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked
            changes
          - <b><span class=c>tracked</span></b>: Stage tracked changes only (like <b>git add -u</b>)
          - <b><span class=c>none</span></b>:    Stage nothing, commit only what&#39;s already in the index

      <b><span class=c>--show-prompt</span></b>
          Show prompt without running LLM

          Outputs the rendered prompt to stdout for debugging or manual piping.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

## wt step copy-ignored

Git worktrees share the repository but not untracked files. This command copies gitignored files to another worktree, eliminating cold starts.

### Setup

Add to the project config:

```toml
# .config/wt.toml
[post-create]
copy = "wt step copy-ignored"
```

All gitignored files are copied by default, as if `.worktreeinclude` contained `**`. To copy only specific patterns, create a `.worktreeinclude` file using gitignore syntax:

```gitignore
# .worktreeinclude — optional, limits what gets copied
.env
node_modules/
target/
.cache/
```

### What gets copied

Only gitignored files are copied — tracked files are never touched. If `.worktreeinclude` exists, files must match **both** `.worktreeinclude` **and** be gitignored.

### Common patterns

| Type | Patterns |
|------|----------|
| Dependencies | `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/` |
| Build caches | `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/` |
| Generated assets | Images, ML models, binaries too large for git |
| Environment files | `.env` (if not generated per-worktree) |

### Features

- Uses copy-on-write (reflink) when available for space-efficient copies
- Handles nested `.gitignore` files, global excludes, and `.git/info/exclude`
- Skips existing files (safe to re-run)
- Skips `.git` entries and other worktrees

### Performance

Reflink copies share disk blocks until modified — no data is actually copied. For a 14GB `target/` directory:

| Command | Time |
|---------|------|
| `cp -R` (full copy) | 2m |
| `cp -Rc` / `wt step copy-ignored` | 20s |

Uses per-file reflink (like `cp -Rc`) — copy time scales with file count.

If the files are needed before any commands run in the worktree, put `wt step copy-ignored` in the `post-create` hook. Otherwise use the `post-start` hook so the copy runs in the background.

### Language-specific notes

#### Rust

The `target/` directory is huge (often 1-10GB). Copying with reflink cuts first build from ~68s to ~3s by reusing compiled dependencies.

#### Node.js

`node_modules/` is large but mostly static. If the project has no native dependencies, symlinks are even faster:

```toml
[post-create]
deps = "ln -sf {{ primary_worktree_path }}/node_modules ."
```

#### Python

Virtual environments contain absolute paths and can't be copied. Use `uv sync` instead — it's fast enough that copying isn't worth it.

### Behavior vs Claude Code on desktop

The `.worktreeinclude` pattern is shared with [Claude Code on desktop](https://code.claude.com/docs/en/desktop), which copies matching files when creating worktrees. Differences:

- worktrunk copies all gitignored files by default; Claude Code requires `.worktreeinclude`
- worktrunk uses copy-on-write for large directories like `target/` — potentially 30x faster on macOS, 6x on Linux
- worktrunk runs as a configurable hook in the worktree lifecycle

### Command reference

{% terminal() %}
wt step copy-ignored - Copy gitignored files to another worktree

Copies gitignored files to another worktree. By default copies all gitignored
files; use <b>.worktreeinclude</b> to limit what gets copied. Useful in post-create
hooks to sync local config files (<b>.env</b>, IDE settings) to new worktrees. Skips
symlinks and existing files.

Usage: <b><span class=c>wt step copy-ignored</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--from</span></b><span class=c> &lt;FROM&gt;</span>
          Source worktree branch

          Defaults to main worktree.

      <b><span class=c>--to</span></b><span class=c> &lt;TO&gt;</span>
          Destination worktree branch

          Defaults to current worktree.

      <b><span class=c>--dry-run</span></b>
          Show what would be copied

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

## wt step for-each

Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

### Template variables

All variables are shell-escaped. See [`wt hook` template variables](@/hook.md#template-variables) for the complete list and filters.

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

<!-- END AUTO-GENERATED from `wt step --help-page` -->
