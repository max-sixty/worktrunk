# Agent Integration

Worktrunk ships a plugin for each supported agent CLI. What a plugin provides depends on the hooks that CLI exposes:

| Capability | Claude Code | Codex | OpenCode | Gemini CLI |
|---|:-:|:-:|:-:|:-:|
| Configuration skill | ✓ | ✓ |  | ✓ |
| Activity tracking (🤖/💬 in `wt list`) | ✓ |  | ✓ | ✓ |
| Worktree isolation | ✓ |  |  |  |
| `/wt-switch-create` command | ✓ |  |  |  |

The configuration skill is documentation the agent reads to help set up LLM commits, hooks, and troubleshooting. Activity tracking shows which worktrees have running sessions. Worktree isolation and `/wt-switch-create` need worktree-lifecycle hooks that only Claude Code exposes, so Codex, OpenCode, and Gemini users invoke `wt switch --create` and `wt remove` directly. Codex omits activity tracking because its hooks have no turn-end event, so a 🤖 marker could never clear back to 💬.

## Installation

### Claude Code

```bash
wt config plugins claude install
```

Manual equivalent:

```bash
claude plugin marketplace add max-sixty/worktrunk
claude plugin install worktrunk@worktrunk
```

### Codex

```bash
wt config plugins codex install
```

This configures the Worktrunk marketplace in Codex. Then run `/plugins` in Codex and install Worktrunk from the marketplace. Manual equivalent:

```bash
codex plugin marketplace add max-sixty/worktrunk
```

To remove the marketplace entry, run `wt config plugins codex uninstall`. Already-installed plugins are left unchanged.

### OpenCode

```bash
wt config plugins opencode install
```

This writes the activity-tracking plugin to OpenCode's global plugins directory, `~/.config/opencode/plugins/worktrunk.ts` (honoring `$OPENCODE_CONFIG_DIR` and `$XDG_CONFIG_HOME`). `wt config plugins opencode uninstall` removes it.

### Gemini CLI

```bash
gemini extensions install https://github.com/max-sixty/worktrunk
```

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

```bash
$ wt list
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>Path</b>                 <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ main             <span class=d>^</span><span class=d>⇡</span>                         <span class=g>⇡1</span>      .                    <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span> 🤖              <span class=g>↑1</span>               ../repo.feature-api  <span class=d>70343f03</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span> 💬              <span class=g>↑1</span>               ../repo.review-ui    <span class=d>a585d6ed</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ wip-docs       <span class=c>?</span> <span class=d>–</span>                                  ../repo.wip-docs     <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 with changes, 2 ahead</span>
```

- 🤖 — agent is working
- 💬 — agent is waiting or idle

The plugin clears the marker when a session ends. A stale marker can remain if the agent process is killed before its session-end hook runs; `wt config state marker clear` removes a marker manually.

### Manual status markers

Set status markers manually for any workflow:

```bash
$ wt config state marker set "🚧"                   # Current branch
$ wt config state marker set "✅" --branch feature  # Specific branch
$ git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct
```

## Worktree isolation (Claude Code only)

Claude Code agents can run in isolated worktrees (`isolation: "worktree"`). By default, Claude Code creates these with `git worktree add`. The plugin's `WorktreeCreate` and `WorktreeRemove` hooks route this through `wt switch --create` and `wt remove` instead, so worktrees created by agents get worktrunk's naming conventions, hooks, and lifecycle management.

## `/wt-switch-create` command (Claude Code only)

`/wt-switch-create [<branch>] [<repo>] [-- <task>]` creates or re-enters a worktrunk worktree, switches the session into it, and runs the task there. The repo and branch name can be supplied or will be inferred from the task description. The skill uses the Claude Code `WorktreeCreate` & `WorktreeRemove` hooks.

## Statusline (Claude Code only)

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. When the CI status cache is stale, this fetches from the network — typically 1–2 seconds — making it suitable for async statuslines but too slow for synchronous shell prompts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>●</span>  Opus  🌔 65%  <span style='color:#a70'>1.4×pace(10am–3pm)</span></code>

When Claude Code provides context window usage via stdin JSON, a moon phase gauge appears (🌕→🌑 as context fills). A yellow `<n>×pace(<window>)` segment appears when Claude's 5-hour or weekly rate limit is on track to be hit before reset — `1.4×pace(10am–3pm)` reads as 1.4× the pace that would exactly fill that window.

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}
```
