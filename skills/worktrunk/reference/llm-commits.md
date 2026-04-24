# LLM Commit Messages

Worktrunk generates commit messages by building a templated prompt and piping it to an external command. This integrates with `wt merge`, `wt step commit`, and `wt step squash`.

## Setup

Any command that reads a prompt from stdin and outputs a commit message works. Add to `~/.config/worktrunk/config.toml`:

### Claude Code

```toml
[commit.generation]
command = "CLAUDECODE= MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''"
```

`CLAUDECODE=` unsets the nesting guard so `claude -p` works from within a Claude Code session. `--no-session-persistence` prevents the commit conversation from polluting `claude --continue`. The other flags disable tools, skills, settings, and system prompt for fast text-only output. See [Claude Code docs](https://docs.anthropic.com/en/docs/build-with-claude/claude-code) for installation.

### Codex

```toml
[commit.generation]
command = "codex exec -m gpt-5.1-codex-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
```

Uses the fast mini model with low reasoning effort and an empty system prompt for faster output. Requires `jq` for JSON parsing. See [Codex CLI docs](https://developers.openai.com/codex/cli/).

### Other tools

```toml
# opencode — use a fast model variant
command = "opencode run -m anthropic/claude-haiku-4.5 --variant fast"

# llm
command = "llm -m claude-haiku-4.5"

# aichat
command = "aichat -m claude:claude-haiku-4.5"
```

## How it works

When worktrunk needs a commit message, it builds a prompt from a template and pipes it to the configured command via shell (`sh -c`). Environment variables can be set inline in the command string.

## Usage

These examples assume a feature worktree with changes to commit.

### wt merge

Squashes all changes (uncommitted + existing commits) into one commit with an LLM-generated message, then merges to the default branch:

```bash
$ wt merge
<span class=c>◎</span> <span class=c>Squashing 3 commits into a single commit <span style='color:var(--bright-black,#555)'>(5 files, <span class=g>+16</span></span></span><span style='color:var(--bright-black,#555)'>)</span>...
<span class=c>◎</span> <span class=c>Generating squash commit message...</span>
<span style='background:var(--bright-white,#fff)'> </span> <b>feat(auth): Implement JWT authentication system</b>
<span style='background:var(--bright-white,#fff)'> </span>
<span style='background:var(--bright-white,#fff)'> </span> Add comprehensive JWT token handling including validation, refresh
<span style='background:var(--bright-white,#fff)'> </span> logic, and authentication tests.
<span class=g>✓</span> <span class=g>Squashed @ a1b2c3d</span>
<span class=c>◎</span> <span class=c>Merging 1 commit to <b>main</b> @ <span class=d>a1b2c3d</span> (no rebase needed)</span>
<span style='background:var(--bright-white,#fff)'> </span> * <span style='color:var(--yellow,#a60)'>a1b2c3d</span> feat(auth): Implement JWT authentication system
<span style='background:var(--bright-white,#fff)'> </span>  auth.rs             | 2 <span class=g>++</span>
<span style='background:var(--bright-white,#fff)'> </span>  auth_test.rs        | 2 <span class=g>++</span>
<span style='background:var(--bright-white,#fff)'> </span>  integration_test.rs | 6 <span class=g>++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>  jwt.rs              | 3 <span class=g>+++</span>
<span style='background:var(--bright-white,#fff)'> </span>  jwt_test.rs         | 3 <span class=g>+++</span>
<span style='background:var(--bright-white,#fff)'> </span>  5 files changed, 16 insertions(+)
<span class=g>✓</span> <span class=g>Merged to <b>main</b> <span style='color:var(--bright-black,#555)'>(1 commit, 5 files, +16</span></span><span style='color:var(--bright-black,#555)'>)</span>
<span class=c>◎</span> <span class=c>Removing <b>feature</b> worktree &amp; branch in background (same commit as <b>main</b>,</span> <span class=d>_</span><span class=c>)</span>
<span class=d>○</span> Switched to worktree for <b>main</b> @ <b>~/repo</b>
```

### wt step commit

Stages and commits with LLM-generated message:

```bash
$ wt step commit
<span class=c>◎</span> <span class=c>Generating commit message and committing changes... <span style='color:var(--bright-black,#555)'>(2 files, <span class=g>+26</span></span></span><span style='color:var(--bright-black,#555)'>)</span>
<span style='background:var(--bright-white,#fff)'> </span> <b>feat(validation): add input validation utilities</b>
<span class=g>✓</span> <span class=g>Committed changes @ <span class=d>a1b2c3d</span></span>
```

### wt step squash

Squashes branch commits into one with LLM-generated message:

```bash
$ wt step squash
<span class=c>◎</span> <span class=c>Squashing 3 commits into a single commit <span style='color:var(--bright-black,#555)'>(5 files, <span class=g>+16</span></span></span><span style='color:var(--bright-black,#555)'>)</span>...
<span class=c>◎</span> <span class=c>Generating squash commit message...</span>
<span style='background:var(--bright-white,#fff)'> </span> <b>feat(auth): Implement JWT authentication system</b>
<span style='background:var(--bright-white,#fff)'> </span>
<span style='background:var(--bright-white,#fff)'> </span> Add comprehensive JWT token handling including validation, refresh
<span style='background:var(--bright-white,#fff)'> </span> logic, and authentication tests.
<span class=g>✓</span> <span class=g>Squashed @ a1b2c3d</span>
```

See [`wt merge`](https://worktrunk.dev/merge/) and [`wt step`](https://worktrunk.dev/step/) for full documentation.

## Branch summaries

[experimental]

With `summary = true` and a `[commit.generation] command` configured, Worktrunk generates LLM branch summaries — one-line descriptions of each branch's changes since the default branch.

Summaries appear in:

- **`wt switch`** [interactive picker](https://worktrunk.dev/switch/#interactive-picker) — preview tab 5
- **`wt list --full`** — the Summary column (see [`wt list`](https://worktrunk.dev/list/#llm-summaries))

Enable in user config:

```toml
[list]
summary = true
```

Summaries are cached and regenerated only when the diff changes.

## Prompt templates

Worktrunk uses [minijinja](https://docs.rs/minijinja/) templates (Jinja2-like syntax) to build prompts. There are sensible defaults, but templates are fully customizable.

### Custom templates

Override the defaults with inline templates:

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

### Template variables

| Variable | Description |
|----------|-------------|
| `{{ git_diff }}` | The diff (staged changes or combined diff for squash) |
| `{{ git_diff_stat }}` | Diff statistics (files changed, insertions, deletions) |
| `{{ branch }}` | Current branch name |
| `{{ repo }}` | Repository name |
| `{{ recent_commits }}` | Recent commit subjects (for style reference) |
| `{{ commits }}` | Commits being squashed (squash template only) |
| `{{ target_branch }}` | Merge target branch (squash template only) |

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
