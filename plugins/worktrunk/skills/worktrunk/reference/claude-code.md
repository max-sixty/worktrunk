# Agent Integration

Worktrunk ships a plugin for each supported agent CLI. What a plugin provides depends on the hooks that CLI exposes:

| Capability | Claude Code | Codex | OpenCode | Gemini CLI |
|---|:-:|:-:|:-:|:-:|
| Configuration skill | ✓ | ✓ |  | ✓ |
| Activity tracking (🤖/💬 in `wt list`) | ✓ | ✓ | ✓ | ✓ |
| Worktree isolation | ✓ |  |  |  |
| `/wt-switch-create` command | ✓ |  |  |  |

The configuration skill is documentation the agent reads to help set up LLM commits, hooks, and troubleshooting. Activity tracking shows which worktrees have running sessions. Worktree isolation needs worktree-lifecycle hooks and `/wt-switch-create` needs session working-directory switching — both Claude Code-only, so Codex, OpenCode, and Gemini users invoke `wt switch --create` and `wt remove` directly. Codex tracks activity through its own `Stop` and `SessionEnd` hooks.

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

The Claude Code, Codex, OpenCode, and Gemini plugins track agent sessions with status markers in `wt list`:

```bash
$ wt list
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Remote⇅</b>  <b>Path</b>                 <b>Commit</b>   <b>Age</b>   <b>Message</b>
@ main             <span class=d>^</span><span class=d>⇡</span>                                    <span class=g>⇡1</span>      .                    <span class=d>33323bc</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span> 🤖              <span class=g>↑1</span>        <span class=g>+1</span>                ../repo.feature-api  <span class=d>70343f0</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span> 💬              <span class=g>↑1</span>        <span class=g>+1</span>                ../repo.review-ui    <span class=d>a585d6e</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ wip-docs       <span class=c>?</span> <span class=d>–</span>                                             ../repo.wip-docs     <span class=d>33323bc</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 with changes, 2 ahead</span>
```

- 🤖 — agent is working
- 💬 — agent is waiting or idle

All four plugins clear the marker when a session ends. A stale marker can remain if the agent process is killed before its session-end hook runs. In every case, `wt config state marker clear` removes a marker manually.

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

`/wt-switch-create [<branch>] [<repo>] [-- <task>]` starts a task in a fresh worktree without leaving the session: it creates the worktree, switches into it, and runs the task (all arguments optional). The worktree shows up in `wt list`; merge or remove it with `wt merge` / `wt remove`.

## Statusline (Claude Code only)

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. Claude Code runs it in the background, which is what makes the occasional 1–2 second CI fetch invisible.

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>#3035</span>  Opus  🌔 65%  <span style='color:#a70'>1.4×(10am–3pm)</span></code>

Worktree state comes from the same cells [`wt list`](https://worktrunk.dev/list/) renders; Claude Code's stdin JSON adds the model, the `🌔 65%` context gauge, and the rate-limit pace notice. [`wt list statusline`](https://worktrunk.dev/list/#wt-list-statusline) documents every segment, how the links behave, and the JSON fields behind them.

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}
```
