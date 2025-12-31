+++
title = "LLM Commit Messages"
weight = 22

[extra]
group = "Reference"
+++

Worktrunk generates commit messages by building a templated prompt and piping it to an external command. This integrates with `wt merge`, `wt step commit`, and `wt step squash`.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-commit.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-commit.gif" alt="LLM commit message generation demo" width="1600" height="900">
</picture>
</figure>

## Setup

### Install llm

[llm](https://llm.datasette.io/) from Simon Willison is recommended:

```bash
$ uv tool install -U llm
```

### Configure an API key

For Claude (recommended):

```bash
$ llm install llm-anthropic
$ llm keys set anthropic
```

For OpenAI:

```bash
$ llm keys set openai
```

### Add to user config

Create the config file if it doesn't exist:

```bash
$ wt config create
```

Then add the commit generation settings to `~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
```

Or for OpenAI:

```toml
[commit-generation]
command = "llm"
args = ["-m", "gpt-5-nano"]
```

## How it works

When worktrunk needs a commit message, it builds a prompt from a template and pipes it to the configured LLM command. The default templates include the git diff and style guidance.

## Usage

These examples assume a feature worktree with changes to commit.

### wt merge

Squashes all changes (uncommitted + existing commits) into one commit with an LLM-generated message, then merges to the default branch:

```bash
$ wt merge
◎ Squashing 3 commits into a single commit (5 files, +48)...
◎ Generating squash commit message...
   feat(auth): Implement JWT authentication system
   ...
```

### wt step commit

Stages and commits with LLM-generated message:

```bash
$ wt step commit
```

### wt step squash

Squashes branch commits into one with LLM-generated message:

```bash
$ wt step squash
```

See [wt merge](@/merge.md) and [wt step](@/step.md) for full documentation.

## Prompt templates

Worktrunk uses [minijinja](https://docs.rs/minijinja/) templates (Jinja2-like syntax) to build prompts. There are sensible defaults, but templates are fully customizable.

### Template variables

All variables are available in both templates:

| Variable | Description |
|----------|-------------|
| `{{ git_diff }}` | The diff (staged changes or combined diff for squash) |
| `{{ branch }}` | Current branch name |
| `{{ recent_commits }}` | Recent commit subjects (for style reference) |
| `{{ repo }}` | Repository name |
| `{{ commits }}` | Commit messages being squashed (chronological order) |
| `{{ target_branch }}` | Branch being merged into |

### Custom templates

Override the defaults with inline templates or external files:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]

template = """
Write a commit message for this diff. One line, under 50 chars.

Branch: {{ branch }}
Diff:
{{ git_diff }}
"""

squash-template = """
Combine these {{ commits | length }} commits into one message:
{% for c in commits %}
- {{ c }}
{% endfor %}

Diff:
{{ git_diff }}
"""
```

Or load templates from files (supports `~` expansion):

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
template-file = "~/.config/worktrunk/commit-template.txt"
squash-template-file = "~/.config/worktrunk/squash-template.txt"
```

### Template syntax

Templates use [minijinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html), which supports:

- **Variables**: `{{ branch }}`, `{{ repo | upper }}`
- **Filters**: `{{ commits | length }}`, `{{ repo | upper }}`
- **Conditionals**: `{% if recent_commits %}...{% endif %}`
- **Loops**: `{% for c in commits %}{{ c }}{% endfor %}`
- **Loop variables**: `{{ loop.index }}`, `{{ loop.length }}`
- **Whitespace control**: `{%- ... -%}` strips surrounding whitespace

See `wt config create --help` for the full default templates.

## Alternative tools

Any command that reads a prompt from stdin and outputs a commit message works:

```toml
# aichat
[commit-generation]
command = "aichat"
args = ["-m", "claude:claude-haiku-4.5"]

# Custom script
[commit-generation]
command = "./scripts/generate-commit.sh"
```

See [llm documentation](https://llm.datasette.io/) and [aichat](https://github.com/sigoden/aichat).

## Fallback behavior

When no LLM is configured, worktrunk generates deterministic messages based on changed filenames (e.g., "Changes to auth.rs & config.rs").
