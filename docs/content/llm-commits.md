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

Any command that reads a prompt from stdin and outputs a commit message works. Add to `~/.config/worktrunk/config.toml`:

### Claude Code

```toml
[commit.generation]
command = "MAX_THINKING_TOKENS=0 claude -p --model haiku --tools '' --disable-slash-commands --setting-sources '' --system-prompt ''"
```

The flags disable tools, skills, settings, and system prompt for fast text-only output. See [Claude Code docs](https://docs.anthropic.com/en/docs/build-with-claude/claude-code) for installation.

### llm

```toml
[commit.generation]
command = "llm -m claude-haiku-4.5"
```

Install with `uv tool install llm llm-anthropic && llm keys set anthropic`. See [llm docs](https://llm.datasette.io/).

### aichat

```toml
[commit.generation]
command = "aichat -m claude:claude-haiku-4.5"
```

See [aichat docs](https://github.com/sigoden/aichat).

### Codex

```toml
[commit.generation]
command = "codex exec -m gpt-5.1-codex-mini -c model_reasoning_effort='low' --sandbox read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
```

Uses the fast mini model with low reasoning effort. Requires `jq` for JSON parsing. See [Codex CLI docs](https://developers.openai.com/codex/cli/).

## How it works

When worktrunk needs a commit message, it builds a prompt from a template and pipes it to the configured command via shell (`sh -c`). Environment variables can be set inline in the command string.

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

See [`wt merge`](@/merge.md) and [`wt step`](@/step.md) for full documentation.

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
[commit.generation]
command = "llm -m claude-haiku-4.5"

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

### Template syntax

Templates use [minijinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html), which supports:

- **Variables**: `{{ branch }}`, `{{ repo | upper }}`
- **Filters**: `{{ commits | length }}`, `{{ repo | upper }}`
- **Conditionals**: `{% if recent_commits %}...{% endif %}`
- **Loops**: `{% for c in commits %}{{ c }}{% endfor %}`
- **Loop variables**: `{{ loop.index }}`, `{{ loop.length }}`
- **Whitespace control**: `{%- ... -%}` strips surrounding whitespace

See `wt config create --help` for the full default templates.

## Fallback behavior

When no LLM is configured, worktrunk generates deterministic messages based on changed filenames (e.g., "Changes to auth.rs & config.rs").
