+++
title = "Claude Code Integration"
description = "Worktrunk plugin for Claude Code: configuration skill, worktree isolation for agents, and activity tracking for wt list."
weight = 23

[extra]
group = "Reference"
+++

The worktrunk Claude Code plugin provides three features:

1. **Configuration skill** — Documentation Claude Code can read, so it can help set up LLM commits, hooks, and troubleshoot issues
2. **Worktree isolation** — When Claude Code agents create isolated worktrees, the plugin routes creation and removal through `wt` instead of raw `git`
3. **Activity tracking** — Status markers in `wt list` showing which worktrees have active Claude sessions (🤖 working, 💬 waiting)

## Installation

Recommended:

{{ terminal(cmd="wt config plugins claude install") }}

Manual equivalent:

{{ terminal(cmd="claude plugin marketplace add max-sixty/worktrunk|||claude plugin install worktrunk@worktrunk") }}

## Configuration skill

The plugin includes a skill — documentation that Claude Code can read — covering worktrunk's configuration system. After installation, Claude Code can help with:

- Setting up LLM-generated commit messages
- Adding project hooks (pre-start, pre-merge, pre-commit)
- Configuring worktree path templates
- Fixing shell integration issues

Claude Code is designed to load the skill automatically when it detects worktrunk-related questions.

## Activity tracking

The plugin tracks Claude sessions with status markers in `wt list`:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__list_with_user_marker.snap — edit source to update -->

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

- 🤖 — Claude is working
- 💬 — Claude is waiting for input

### Manual status markers

Set status markers manually for any workflow:

{% terminal() %}
<span class="cmd">wt config state marker set "🚧"                   # Current branch</span>
<span class="cmd">wt config state marker set "✅" --branch feature  # Specific branch</span>
<span class="cmd">git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct</span>
{% end %}

## Worktree isolation

Claude Code agents can run in isolated worktrees (`isolation: "worktree"`). By default, Claude Code creates these with `git worktree add`. The plugin's `WorktreeCreate` and `WorktreeRemove` hooks route this through `wt switch --create` and `wt remove` instead, so worktrees created by agents get worktrunk's naming conventions, hooks, and lifecycle management.

## Statusline

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. This may fetch CI status from the network when the cache is stale (often ~1–2 seconds), making it suitable for async statuslines but too slow for synchronous shell prompts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>●</span>  | Opus 🌔 65%</code>

When Claude Code provides context window usage via stdin JSON, a moon phase gauge appears (🌕→🌑 as context fills).

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-statusline.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-statusline.gif" alt="Claude Code statusline demo" width="1600" height="900">
</picture>
</figure>

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}
```
