+++
title = "Codex Integration"
description = "Worktrunk plugin for Codex: configuration skill, bundled activity hooks for wt list, and worktree workflow guidance."
weight = 24

[extra]
group = "Reference"
+++

The Worktrunk Codex plugin packages two features:

1. **Configuration skill** — Documentation Codex can read, so it can help set up LLM commits, hooks, and troubleshoot issues
2. **Bundled activity hooks** — Codex lifecycle hooks that can set `wt list` markers for active Codex sessions (🤖 working, 💬 waiting or idle)

Codex does not currently expose the Claude Code `WorktreeCreate` and `WorktreeRemove` hook events. Use `wt switch --create` and `wt remove` directly for worktree lifecycle management.

## Installation

Recommended:

{{ terminal(cmd="wt config plugins codex install") }}

This configures the Worktrunk marketplace. It does not install the plugin by itself. Then open `/plugins` in Codex and install Worktrunk from the Worktrunk marketplace.

Manual equivalent:

{{ terminal(cmd="codex plugin marketplace add max-sixty/worktrunk") }}

To remove the marketplace entry later:

{{ terminal(cmd="wt config plugins codex uninstall") }}

Uninstall removes the Worktrunk marketplace from Codex. It intentionally leaves any already-installed Worktrunk plugin and global Codex hook feature flags unchanged, because other Codex hooks may depend on those settings.

## Configuration skill

The plugin includes a skill — documentation that Codex can read — covering Worktrunk's configuration system. After installation, Codex can help with:

- Setting up LLM-generated commit messages
- Adding project hooks (pre-start, pre-merge, pre-commit)
- Configuring worktree path templates
- Fixing shell integration issues
- Spawning parallel worktrees when explicitly requested

## Activity tracking

When Codex loads the plugin's bundled hooks, the plugin tracks Codex sessions with status markers in `wt list`:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__list_with_user_marker.snap — edit source to update -->

{% terminal(cmd="wt list") %}
<span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>Path</b>                 <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ main             <span class=d>^</span><span class=d>⇡</span>                         <span class=g>⇡1</span>      .                    <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span> 🤖              <span class=g>↑1</span>               ../repo.feature-api  <span class=d>70343f03</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span> 💬              <span class=g>↑1</span>               ../repo.review-ui    <span class=d>a585d6ed</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ wip-docs       <span class=c>?</span> <span class=d>–</span>                                  ../repo.wip-docs     <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 with changes, 2 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

- 🤖 — Codex is working
- 💬 — Codex is waiting or idle

If a Codex process exits before the next `Stop` hook, the marker can remain. Clear it manually with:

{{ terminal(cmd="wt config state marker clear") }}

Codex CLI releases may gate plugin-bundled hooks behind the `plugin_hooks` feature. If markers do not appear after installing the plugin, run:

{{ terminal(cmd="codex features list") }}

If `plugin_hooks` is `false`, enable it:

{{ terminal(cmd="codex features enable plugin_hooks") }}

As a stable fallback, copy this repository's `hooks/hooks.json` to a normal Codex hook location such as `~/.codex/hooks.json`.

## Worktree workflow

Create a worktree and launch Codex in one step:

{{ terminal(cmd="wt switch --create feature-auth --execute=codex -- 'Add authentication tests'") }}

For multiple parallel Codex sessions:

{{ terminal(cmd="wt switch -x codex -c feature-a -- 'Add user authentication'|||wt switch -x codex -c feature-b -- 'Fix the pagination bug'|||wt switch -x codex -c feature-c -- 'Write tests for the API'") }}

The `-x` flag runs a command after switching; arguments after `--` are passed to Codex. Configure [post-start hooks](@/hook.md#hook-types) to automate setup (install deps, start dev servers).

## LLM commit messages

Worktrunk can also use Codex to generate commit messages:

```toml
[commit.generation]
command = "codex exec -m gpt-5.1-codex-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
```

See [LLM Commit Messages](@/llm-commits.md#codex) for details.

## Claude Code comparison

The [Claude Code plugin](@/claude-code.md) has one extra integration: Claude Code worktree lifecycle hooks can route agent-created worktrees through `wt switch --create` and `wt remove`. Codex users should invoke Worktrunk directly for lifecycle operations.
