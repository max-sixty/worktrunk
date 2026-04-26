+++
title = "wt step"
description = "Run individual operations. The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities."
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

Run individual operations. The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities.

## Examples

Commit with LLM-generated message:

{% terminal(cmd="wt step commit") %}
<span class=c>◎</span> <span class=c>Generating commit message and committing changes... <span style='color:var(--bright-black,#555)'>(2 files, <span class=g>+26</span></span></span><span style='color:var(--bright-black,#555)'>)</span>
<span style='background:var(--bright-white,#fff)'> </span> <b>feat(validation): add input validation utilities</b>
<span class=g>✓</span> <span class=g>Committed changes @ <span class=d>a1b2c3d</span></span>
{% end %}

Manual merge workflow with review between steps:

{{ terminal(cmd="wt step commit|||wt step squash|||wt step rebase|||wt step push") }}

## Operations

- [`commit`](#wt-step-commit) — Stage and commit with [LLM-generated message](@/llm-commits.md)
- [`squash`](#wt-step-squash) — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- [`diff`](#wt-step-diff) — Show all changes since branching (committed, staged, unstaged, untracked)
- [`copy-ignored`](#wt-step-copy-ignored) — Copy gitignored files between worktrees
- [`eval`](#wt-step-eval) — <span class="badge-experimental"></span> Evaluate a template expression
- [`for-each`](#wt-step-for-each) — <span class="badge-experimental"></span> Run a command in every worktree
- [`promote`](#wt-step-promote) — <span class="badge-experimental"></span> Swap a branch into the main worktree
- [`prune`](#wt-step-prune) — Remove worktrees and branches merged into the default branch
- [`relocate`](#wt-step-relocate) — <span class="badge-experimental"></span> Move worktrees to expected paths
- [`<alias>`](@/extending.md#aliases) — Run a configured command alias

## See also

- [`wt merge`](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [`wt hook`](@/hook.md) — Run configured hooks
- [Aliases](@/extending.md#aliases) — Custom command templates run as `wt <name>`

## Command reference

{% terminal() %}
wt step - Run individual operations

The building blocks of <b>wt merge</b> — commit, squash, rebase, push — plus standalone utilities.

Usage: <b><span class=c>wt step</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>commit</span></b>        Stage and commit with LLM-generated message
  <b><span class=c>squash</span></b>        Squash commits since branching
  <b><span class=c>rebase</span></b>        Rebase onto target
  <b><span class=c>push</span></b>          Fast-forward target to current branch
  <b><span class=c>diff</span></b>          Show all changes since branching
  <b><span class=c>copy-ignored</span></b>  Copy gitignored files to another worktree
  <b><span class=c>eval</span></b>          [experimental] Evaluate a template expression
  <b><span class=c>for-each</span></b>      [experimental] Run command in each worktree
  <b><span class=c>promote</span></b>       [experimental] Swap a branch into the main worktree
  <b><span class=c>prune</span></b>         [experimental] Remove worktrees merged into the default branch
  <b><span class=c>relocate</span></b>      [experimental] Move worktrees to expected paths

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

# Subcommands

## wt step commit

Stage and commit with LLM-generated message.

See [LLM-generated commit messages](@/llm-commits.md) for configuration and prompt customization.

### Options

#### `--stage`

Controls what to stage before committing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

{{ terminal(cmd="wt step commit --stage=tracked") }}

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

{{ terminal(cmd="# Inspect the rendered prompt|||wt step commit --show-prompt | less||||||# Pipe to a different LLM|||wt step commit --show-prompt | llm -m gpt-5-nano") }}

### Command reference

{% terminal() %}
wt step commit - Stage and commit with LLM-generated message

Usage: <b><span class=c>wt step commit</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
  <b><span class=c>-b</span></b>, <b><span class=c>--branch</span></b><span class=c> &lt;BRANCH&gt;</span>
          Branch to operate on (defaults to current worktree)

      <b><span class=c>--no-hooks</span></b>
          Skip hooks

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked changes
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
          Verbose output (-v: info logs + hook/alias template variable &amp; output; -vv: debug logs +
          diagnostic report + trace.log/output.log under .git/wt/logs/)

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts
{% end %}

## wt step squash

Squash commits since branching. Stages changes and generates message with LLM.

See [LLM-generated commit messages](@/llm-commits.md) for configuration and prompt customization.

### Options

#### `--stage`

Controls what to stage before squashing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, squash only committed changes |

{{ terminal(cmd="wt step squash --stage=none") }}

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

{{ terminal(cmd="wt step squash --show-prompt | less") }}

### Command reference

{% terminal() %}
wt step squash - Squash commits since branching

Stages changes and generates message with LLM.

Usage: <b><span class=c>wt step squash</span></b> <span class=c>[OPTIONS]</span> <span class=c>[TARGET]</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[TARGET]</span>
          Target branch

          Defaults to default branch.

<b><span class=g>Options:</span></b>
      <b><span class=c>--no-hooks</span></b>
          Skip hooks

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked changes
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
          Verbose output (-v: info logs + hook/alias template variable &amp; output; -vv: debug logs +
          diagnostic report + trace.log/output.log under .git/wt/logs/)

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts
{% end %}

## wt step diff

Show all changes since branching. Includes committed, staged, unstaged, and untracked files.

This is what `wt merge` would include — a single diff against the merge base.

### Extra git diff arguments

Arguments after `--` are forwarded to `git diff`:

{{ terminal(cmd="wt step diff -- --stat|||wt step diff -- --name-only|||wt step diff -- -- '*.rs'") }}

The diff is pipeable to tools like `delta`:

{{ terminal(cmd="wt step diff | delta") }}

### How it works

Equivalent to:

{{ terminal(cmd="cp __WT_QUOT__$(git rev-parse --git-dir)/index__WT_QUOT__ /tmp/idx|||GIT_INDEX_FILE=/tmp/idx git add --intent-to-add .|||GIT_INDEX_FILE=/tmp/idx git diff $(git merge-base HEAD $(wt config state default-branch))") }}

`git diff` ignores untracked files. `git add --intent-to-add .` registers them in the index without staging their content, making them visible to `git diff`. This runs against a copy of the real index so the original is never modified.

### Command reference

{% terminal() %}
wt step diff - Show all changes since branching

Includes committed, staged, unstaged, and untracked files.

Usage: <b><span class=c>wt step diff</span></b> <span class=c>[OPTIONS]</span> <span class=c>[TARGET]</span> <b><span class=c>[--</span></b> <span class=c>&lt;EXTRA_ARGS&gt;...</span><b><span class=c>]</span></b>

<b><span class=g>Arguments:</span></b>
  <span class=c>[TARGET]</span>
          Target branch

          Defaults to default branch.

  <span class=c>[EXTRA_ARGS]...</span>
          Extra arguments forwarded to <b>git diff</b>

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step copy-ignored

Copy gitignored files to another worktree. Eliminates cold starts by copying build caches and dependencies.

### Setup

Add to the project config:

```toml
# .config/wt.toml
[post-start]
copy = "wt step copy-ignored"
```

### What gets copied

All gitignored files are copied by default, except for built-in excluded directories: VCS metadata (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`). Tracked files are never touched.

To limit what gets copied further, create `.worktreeinclude` with gitignore-style patterns. Files must be **both** gitignored **and** in `.worktreeinclude`:

```text
# .worktreeinclude
.env
node_modules/
target/
```

After `.worktreeinclude` selects entries, you can add more gitignore-style excludes in user config, per-project user overrides, or project config:

```toml
[step.copy-ignored]
exclude = [".cache/", ".turbo/"]
```

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
- Skips existing files by default (safe to re-run)
- `--force` overwrites existing files in the destination
- Always skips built-in excluded directories — VCS metadata (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`) — and nested worktrees

### Performance

Reflink copies share disk blocks until modified — no data is actually copied. For a 14GB `target/` directory:

| Command | Time |
|---------|------|
| `cp -R` (full copy) | 2m |
| `cp -Rc` / `wt step copy-ignored` | 20s |

Uses per-file reflink (like `cp -Rc`) — copy time scales with file count.

Use the `post-start` hook so the copy runs in the background. Use `pre-start` instead if subsequent hooks or `--execute` command need the copied files immediately.

### Background-hook priority (experimental)

When invoked from a background hook pipeline (`post-*` hooks), `wt step copy-ignored` self-lowers its CPU and I/O priority — `taskpolicy -b` on macOS, `nice -n 19` plus `ionice -c 3` on Linux — so it yields to interactive work. Foreground callers (`pre-*` hooks, direct interactive use) run at normal priority so the user isn't waiting on a throttled copy.

wt signals background-hook context by exporting `WORKTRUNK_FOREGROUND=-1` into every detached hook pipeline; `copy-ignored` inspects that variable on entry. The variable name is experimental and may change.

### Language-specific notes

#### Rust

The `target/` directory is huge (often 1-10GB). Copying with reflink cuts first build from ~68s to ~3s by reusing compiled dependencies.

#### Node.js

`node_modules/` is large but mostly static. If the project has no native dependencies, symlinks are even faster:

```toml
[pre-start]
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

Eliminates cold starts by copying build caches and dependencies.

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

      <b><span class=c>--force</span></b>
          Overwrite existing files in destination

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step eval

<span class="badge-experimental"></span>

Evaluate a template expression. Prints the result to stdout for use in scripts and shell substitutions.

All [hook template variables and filters](@/hook.md#template-variables) are available.

### Examples

Get the port for the current branch:

{% terminal(cmd="wt step eval '__WT_OPEN__ branch | hash_port __WT_CLOSE__'") %}
16066
{% end %}

Use in shell substitution:

{{ terminal(cmd="curl http://localhost:$(wt step eval '__WT_OPEN__ branch | hash_port __WT_CLOSE__')/health") }}

Combine multiple values:

{% terminal(cmd="wt step eval '__WT_OPEN__ branch | hash_port __WT_CLOSE__,__WT_OPEN__ (__WT_QUOT__supabase-api-__WT_QUOT__ ~ branch) | hash_port __WT_CLOSE__'") %}
16066,16739
{% end %}

Use conditionals and filters:

{% terminal(cmd="wt step eval '__WT_OPEN__ branch | sanitize_db __WT_CLOSE__'") %}
feature_auth_oauth2_a1b
{% end %}

Show available template variables:

{% terminal(cmd="wt step eval --dry-run '__WT_OPEN__ branch __WT_CLOSE__'") %}
branch=feature/auth-oauth2
worktree_path=/home/user/projects/myapp-feature-auth-oauth2
...
Result: feature/auth-oauth2
{% end %}

Note: This command is experimental and may change in future versions.

### Command reference

{% terminal() %}
wt step eval - [experimental] Evaluate a template expression

Prints the result to stdout for use in scripts and shell substitutions.

Usage: <b><span class=c>wt step eval</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;TEMPLATE&gt;</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>&lt;TEMPLATE&gt;</span>
          Template expression to evaluate

<b><span class=g>Options:</span></b>
      <b><span class=c>--dry-run</span></b>
          Show template variables and expanded result

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step for-each

<span class="badge-experimental"></span>

Run command in each worktree. Executes sequentially with real-time output; continues on failure.

A summary of successes and failures is shown at the end. Context JSON is piped to stdin for scripts that need structured data.

### Template variables

All variables are shell-escaped. See [`wt hook` template variables](@/hook.md#template-variables) for the complete list and filters.

### Examples

Check status across all worktrees:

{{ terminal(cmd="wt step for-each -- git status --short") }}

Run npm install in all worktrees:

{{ terminal(cmd="wt step for-each -- npm install") }}

Use branch name in command:

{{ terminal(cmd="wt step for-each -- __WT_QUOT__echo Branch: __WT_OPEN__ branch __WT_CLOSE____WT_QUOT__") }}

Pull updates in worktrees with upstreams (skips others):

{{ terminal(cmd="git fetch --prune && wt step for-each -- '[ __WT_QUOT__$(git rev-parse @{u} 2>/dev/null)__WT_QUOT__ ] || exit 0; git pull --autostash'") }}

Note: This command is experimental and may change in future versions.

### Command reference

{% terminal() %}
wt step for-each - [experimental] Run command in each worktree

Executes sequentially with real-time output; continues on failure.

Usage: <b><span class=c>wt step for-each</span></b> <span class=c>[OPTIONS]</span> <b><span class=c>--</span></b> <span class=c>&lt;ARGS&gt;...</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>&lt;ARGS&gt;...</span>
          Command template (see --help for all variables)

<b><span class=g>Options:</span></b>
      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format (text, json)

          Possible values:
          - <b><span class=c>text</span></b>: Human-readable text output
          - <b><span class=c>json</span></b>: JSON output

          [default: text]

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step promote

<span class="badge-experimental"></span>

Swap a branch into the main worktree. Exchanges branches and gitignored files between two worktrees.

**Experimental.** Use promote for temporary testing when the main worktree has special significance (Docker Compose, IDE configs, heavy build artifacts anchored to project root), and hooks & tools aren't yet set up to run on arbitrary worktrees. The idiomatic Worktrunk workflow does not use `promote`; instead each worktree has a full environment. `promote` is the only Worktrunk command which changes a branch in an existing worktree.

### Example

{{ terminal(cmd="# from ~/project (main worktree)|||wt step promote feature") }}

Before:

```
  Branch   Path
@ main     ~/project
+ feature  ~/project.feature
```

After:

```
  Branch   Path
@ feature  ~/project
+ main     ~/project.feature
```

To restore: `wt step promote main` from anywhere, or just `wt step promote` from the main worktree.

Without an argument, promotes the current branch — or restores the default branch if run from the main worktree.

### Requirements

- Both worktrees must be clean
- The branch must have an existing worktree

### Gitignored files

Gitignored files (build artifacts, `node_modules/`, `.env`) are swapped along with the branches so each worktree keeps the artifacts that belong to its branch. Files are discovered using the same mechanism as [`copy-ignored`](#wt-step-copy-ignored) and can be filtered with `.worktreeinclude`.

The swap uses `rename()` for each entry — fast regardless of entry size, since only filesystem metadata changes. If the worktree is on a different filesystem from `.git/`, it falls back to reflink copy.

### Command reference

{% terminal() %}
wt step promote - [experimental] Swap a branch into the main worktree

Exchanges branches and gitignored files between two worktrees.

Usage: <b><span class=c>wt step promote</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCH]</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCH]</span>
          Branch to promote to main worktree

          Defaults to current branch, or default branch from main worktree.

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step prune

<span class="badge-experimental"></span>

Remove worktrees merged into the default branch.

Bulk-removes worktrees and branches that are integrated into the default branch, using the same criteria as `wt remove`'s branch cleanup. Stale worktree entries are cleaned up too.

In `wt list`, candidates show `_` (same commit) or `⊂` (content integrated). Run `--dry-run` to preview. See `wt remove --help` for the full integration criteria.

Locked worktrees and the main worktree are always skipped. The current worktree is removed last, triggering cd to the primary worktree. Pre-remove and post-remove hooks run for each removal.

### Min-age guard

Worktrees younger than `--min-age` (default: 1 hour) are skipped. This prevents removing a worktree just created from the default branch — it looks "merged" because its branch points at the same commit.

{{ terminal(cmd="wt step prune --min-age=0s     # no age guard|||wt step prune --min-age=2d     # skip worktrees younger than 2 days") }}

### Examples

Preview what would be removed:

{{ terminal(cmd="wt step prune --dry-run") }}

Remove all merged worktrees:

{{ terminal(cmd="wt step prune") }}

### Command reference

{% terminal() %}
wt step prune - [experimental] Remove worktrees merged into the default branch

Usage: <b><span class=c>wt step prune</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--dry-run</span></b>
          Show what would be removed

      <b><span class=c>--min-age</span></b><span class=c> &lt;MIN_AGE&gt;</span>
          Skip worktrees younger than this

          [default: 1h]

      <b><span class=c>--foreground</span></b>
          Run removal in foreground (block until complete)

      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format (text, json)

          Possible values:
          - <b><span class=c>text</span></b>: Human-readable text output
          - <b><span class=c>json</span></b>: JSON output

          [default: text]

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

## wt step relocate

<span class="badge-experimental"></span>

Move worktrees to expected paths. Relocates worktrees whose path doesn't match the worktree-path template.

### Examples

Preview what would be moved:

{{ terminal(cmd="wt step relocate --dry-run") }}

Move all mismatched worktrees:

{{ terminal(cmd="wt step relocate") }}

Auto-commit and clobber blockers (never fails):

{{ terminal(cmd="wt step relocate --commit --clobber") }}

Move specific worktrees:

{{ terminal(cmd="wt step relocate feature bugfix") }}

### Swap handling

When worktrees are at each other's expected locations (e.g., `alpha` at
`repo.beta` and `beta` at `repo.alpha`), relocate automatically resolves
this by using a temporary location.

### Clobbering

With `--clobber`, non-worktree paths at target locations are moved to
`<path>.bak-<timestamp>` before relocating.

### Main worktree behavior

The main worktree can't be moved with `git worktree move`. Instead, relocate
switches it to the default branch and creates a new linked worktree at the
expected path. Untracked and gitignored files remain at the original location.

### Skipped worktrees

- **Dirty** (without `--commit`) — use `--commit` to auto-commit first
- **Locked** — unlock with `git worktree unlock`
- **Target blocked** (without `--clobber`) — use `--clobber` to backup blocker
- **Detached HEAD** — no branch to compute expected path

Note: This command is experimental and may change in future versions.

### Command reference

{% terminal() %}
wt step relocate - [experimental] Move worktrees to expected paths

Relocates worktrees whose path doesn&#39;t match the <b>worktree-path</b> template.

Usage: <b><span class=c>wt step relocate</span></b> <span class=c>[OPTIONS]</span> <span class=c>[BRANCHES]...</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[BRANCHES]...</span>
          Worktrees to relocate (defaults to all mismatched)

<b><span class=g>Options:</span></b>
      <b><span class=c>--dry-run</span></b>
          Show what would be moved

      <b><span class=c>--commit</span></b>
          Commit uncommitted changes before relocating

      <b><span class=c>--clobber</span></b>
          Backup non-worktree paths at target locations

          Moves blocking paths to <b>&lt;path&gt;.bak-&lt;timestamp&gt;</b>.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

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

<!-- END AUTO-GENERATED from `wt step --help-page` -->
