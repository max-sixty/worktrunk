+++
title = "Claude Code Integration"
weight = 23

[extra]
group = "Reference"
+++

Worktrunk includes a Claude Code plugin that automatically tracks which worktrees have active Claude sessions. When Claude starts working in a worktree, the plugin sets a status marker; when Claude waits for input, the marker updates. This makes it easy to monitor multiple parallel agents from `wt list`.

## Status tracking

The plugin adds status indicators to `wt list`:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__with_user_marker.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Path</b>                 <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ main             <span class=d>^</span>                         .                             <span class=d>a058e792</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span> 🤖              <span class=g>↑1</span>      ../repo.feature-api           <span class=d>95e48b49</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span> 💬              <span class=g>↑1</span>      ../repo.review-ui             <span class=d>46b6a187</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ wip-docs       <span class=c>?</span> <span class=d>–</span>                         ../repo.wip-docs              <span class=d>a058e792</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 with changes, 2 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

- 🤖 — Claude is working
- 💬 — Claude is waiting for input

### Installation

```bash
$ claude plugin marketplace add max-sixty/worktrunk
$ claude plugin install worktrunk@worktrunk
```

### Manual status markers

Set status markers manually for any workflow:

```bash
$ wt config state marker set "🚧"                   # Current branch
$ wt config state marker set "✅" --branch feature  # Specific branch
$ git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct
```

## Statusline

`wt list statusline --claude-code` outputs a single-line status for the Claude Code statusline. This may fetch CI status from the network when the cache is stale (often ~1–2 seconds), making it suitable for async statuslines but too slow for synchronous shell prompts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>●</span>  | Opus</code>

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
    "command": "wt list statusline --claude-code"
  }
}
```
