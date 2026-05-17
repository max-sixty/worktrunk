+++
title = "Claude Code & Codex Integration"
description = "Worktrunk plugins for Claude Code and Codex: configuration skill, agent worktree isolation, and activity tracking for wt list."
weight = 23

[extra]
group = "Reference"
+++

Worktrunk ships plugins for Claude Code and Codex. Both bundle:

1. **Configuration skill** — Documentation the agent can read, so it can help set up LLM commits, hooks, and troubleshoot issues
2. **Activity tracking** — Status markers in `wt list` showing which worktrees have active agent sessions (🤖 working, 💬 waiting)

The Claude Code plugin additionally provides:

3. **Worktree isolation** — Routes agent-created isolated worktrees through `wt switch --create` / `wt remove` instead of raw `git`
4. **`/wt-switch-create` command** — Creates a worktrunk worktree and moves the current Claude session into it

Codex does not currently expose equivalent worktree-lifecycle hooks, so Codex users invoke `wt switch --create` and `wt remove` directly.

## Installation

### Claude Code

{{ terminal(cmd="wt config plugins claude install") }}

Manual equivalent:

{{ terminal(cmd="claude plugin marketplace add max-sixty/worktrunk|||claude plugin install worktrunk@worktrunk") }}

### Codex

{{ terminal(cmd="wt config plugins codex install") }}

This configures the Worktrunk marketplace in Codex. Then run `/plugins` in Codex and install Worktrunk from the marketplace. Manual equivalent:

{{ terminal(cmd="codex plugin marketplace add max-sixty/worktrunk") }}

To remove the marketplace entry, run `wt config plugins codex uninstall`. Already-installed plugins and global Codex hook feature flags are left unchanged.

If activity markers do not appear after installing the plugin, Codex may be gating plugin-bundled hooks. Run `codex features list`; if `plugin_hooks` is `false`, enable it (`codex features enable plugin_hooks`), or copy this repo's `plugins/worktrunk/hooks/hooks.json` to `~/.codex/hooks.json`.

## Configuration skill

The plugin includes a skill — documentation the agent can read — covering Worktrunk's configuration system. After installation, the agent can help with:

- Setting up LLM-generated commit messages
- Adding project hooks (pre-start, pre-merge, pre-commit)
- Configuring worktree path templates
- Fixing shell integration issues

Claude Code is designed to load the skill automatically when it detects worktrunk-related questions.

## Activity tracking

The plugins track agent sessions with status markers in `wt list`:

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

The Claude Code plugin clears the marker when a session ends; Codex exposes no session-end hook event, so a Codex worktree rests at 💬 after its session ends rather than clearing. Either plugin can also leave a stale marker if the agent process is killed before its stop hook runs. `wt config state marker clear` removes a marker manually.

### Manual status markers

Set status markers manually for any workflow:

{% terminal() %}
<span class="cmd">wt config state marker set "🚧"                   # Current branch</span>
<span class="cmd">wt config state marker set "✅" --branch feature  # Specific branch</span>
<span class="cmd">git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct</span>
{% end %}

## Worktree isolation (Claude Code only)

Claude Code agents can run in isolated worktrees (`isolation: "worktree"`). By default, Claude Code creates these with `git worktree add`. The plugin's `WorktreeCreate` and `WorktreeRemove` hooks route this through `wt switch --create` and `wt remove` instead, so worktrees created by agents get worktrunk's naming conventions, hooks, and lifecycle management.

Codex does not currently expose equivalent hook events, so Codex users should invoke `wt switch --create` and `wt remove` directly.

## `/wt-switch-create` command (Claude Code only)

`/wt-switch-create <branch> [<repo>] [-- <task>]` starts work in a fresh worktree without leaving the session. It creates (or re-enters) the named worktrunk worktree — sibling layout `<repo>.<branch>/`, not `.claude/worktrees/` — switches the session's working directory into it, then runs the task there. An optional second token names a different repository to create the worktree in; the task is whatever follows `--` (or, with no `--`, whatever follows the branch). The command rides the same `WorktreeCreate` hook as agent isolation, so the worktree gets worktrunk's naming, hooks, and lifecycle.

On session exit the worktree is offered for removal via the `WorktreeRemove` hook; one with uncommitted changes is kept rather than removed.

## Statusline (Claude Code only)

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. When the CI status cache is stale, this fetches from the network — typically 1–2 seconds — making it suitable for async statuslines but too slow for synchronous shell prompts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

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
