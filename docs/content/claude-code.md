+++
title = "Agent Integration"
description = "Worktrunk plugins for Claude Code, Codex, OpenCode, and Gemini CLI: a configuration skill, wt list activity tracking, and Claude-only worktree isolation."
weight = 23

[extra]
group = "Reference"
+++

Worktrunk ships a plugin for each supported agent CLI. What a plugin provides depends on the hooks that CLI exposes:

| Capability | Claude Code | Codex | OpenCode | Gemini CLI |
|---|:-:|:-:|:-:|:-:|
| Configuration skill | ✓ | ✓ |  | ✓ |
| Activity tracking (🤖/💬 in `wt list`) | ✓ |  | ✓ | ✓ |
| Worktree isolation | ✓ |  |  |  |
| `/wt-switch-create` command | ✓ |  |  |  |

The configuration skill is documentation the agent reads to help set up LLM commits, hooks, and troubleshooting. Activity tracking shows which worktrees have running sessions. Worktree isolation needs worktree-lifecycle hooks and `/wt-switch-create` needs session working-directory switching — both Claude Code-only, so Codex, OpenCode, and Gemini users invoke `wt switch --create` and `wt remove` directly. Codex omits activity tracking because its hooks have no turn-end event, so a 🤖 marker could never clear back to 💬.

## Installation

### Claude Code

{{ terminal(cmd="wt config plugins claude install") }}

Manual equivalent:

{{ terminal(cmd="claude plugin marketplace add max-sixty/worktrunk|||claude plugin install worktrunk@worktrunk") }}

### Codex

{{ terminal(cmd="wt config plugins codex install") }}

This configures the Worktrunk marketplace in Codex. Then run `/plugins` in Codex and install Worktrunk from the marketplace. Manual equivalent:

{{ terminal(cmd="codex plugin marketplace add max-sixty/worktrunk") }}

To remove the marketplace entry, run `wt config plugins codex uninstall`. Already-installed plugins are left unchanged.

### OpenCode

{{ terminal(cmd="wt config plugins opencode install") }}

This writes the activity-tracking plugin to OpenCode's global plugins directory, `~/.config/opencode/plugins/worktrunk.ts` (honoring `$OPENCODE_CONFIG_DIR` and `$XDG_CONFIG_HOME`). `wt config plugins opencode uninstall` removes it.

### Gemini CLI

{{ terminal(cmd="gemini extensions install https://github.com/max-sixty/worktrunk") }}

Gemini loads the extension natively from the repository, so there is no `wt` wrapper. `gemini extensions uninstall worktrunk` removes it.

## Configuration skill

With the `/worktrunk` skill, the agent can help with:

- Setting up LLM-generated commit messages
- Adding project hooks (pre-start, pre-merge, pre-commit)
- Configuring worktree path templates
- Fixing shell integration issues

Claude Code is designed to load the skill automatically when it detects worktrunk-related questions.

## Activity tracking

The Claude Code, OpenCode, and Gemini plugins track agent sessions with status markers in `wt list`:

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

- 🤖 — agent is working
- 💬 — agent is waiting or idle

The plugin clears the marker when a session ends. A stale marker can remain if the agent process is killed before its session-end hook runs; `wt config state marker clear` removes a marker manually.

### Manual status markers

Set status markers manually for any workflow:

{% terminal() %}
<span class="cmd">wt config state marker set "🚧"                   # Current branch</span>
<span class="cmd">wt config state marker set "✅" --branch feature  # Specific branch</span>
<span class="cmd">git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct</span>
{% end %}

## Worktree isolation (Claude Code only)

Claude Code agents can run in isolated worktrees (`isolation: "worktree"`). By default, Claude Code creates these with `git worktree add`. The plugin's `WorktreeCreate` and `WorktreeRemove` hooks route this through `wt switch --create` and `wt remove` instead, so worktrees created by agents get worktrunk's naming conventions, hooks, and lifecycle management.

## `/wt-switch-create` command (Claude Code only)

`/wt-switch-create [<branch>] [<repo>] [-- <task>]` starts a task in a fresh worktree without leaving the session: it creates the worktree, switches into it, and runs the task (all arguments optional). The worktree persists like any other; merge or remove it with `wt merge` / `wt remove`.

## Statusline (Claude Code only)

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. When the CI status cache is stale, this fetches from the network — typically 1–2 seconds — making it suitable for async statuslines but too slow for synchronous shell prompts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>#3035</span>  Opus  🌔 65%  <span style='color:#a70'>1.4×(10am–3pm)</span></code>

When Claude Code provides context window usage via stdin JSON, a moon phase gauge appears (🌕→🌑 as context fills). A `<n>×(<window>)` segment appears when Claude's 5-hour or weekly rate limit is on track to be hit before reset — `1.4×(10am–3pm)` reads as 1.4× the pace that would exactly fill that window. Its colour deepens with severity — dim, then dim-yellow, then yellow — as more of the window would be spent locked out at the cap, so a fast pace that would only tip over near the reset stays dim. Above 90% used it shows usage instead of pace — `93%(10am–3pm)` — near the cap, how much is left matters more than how fast it's going.

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
