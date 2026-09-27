# Agent Integration

Worktrunk ships a plugin for each supported agent CLI. What a plugin provides depends on the hooks that CLI exposes:

| Capability | Claude Code | Codex | OpenCode | Pi | oh-my-pi | Gemini CLI |
|---|:-:|:-:|:-:|:-:|:-:|:-:|
| Configuration skill | ✓ | ✓ |  |  |  | ✓ |
| Activity tracking (🤖/💬 in `wt list`) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Worktree isolation | ✓ |  |  |  |  |  |
| `/wt-switch-create` skill\* | ✓ |  |  |  |  |  |

\* Codex and Gemini also load the `/wt-switch-create` skill from the shared skill set, but neither lets a skill change the session's working directory, so it does nothing there.

The configuration skill is documentation the agent reads to help set up LLM commits, hooks, and troubleshooting. Activity tracking shows which worktrees have running sessions. Worktree isolation needs worktree-lifecycle hooks, which only Claude Code exposes, so Codex, OpenCode, Pi, oh-my-pi, and Gemini users invoke `wt switch --create` and `wt remove` directly. Codex tracks activity through its own `Stop` and `SessionEnd` hooks.

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

`wt config plugins claude uninstall` removes the plugin and its marketplace entry.

### Codex

```bash
wt config plugins codex install
```

Manual equivalent:

```bash
codex plugin marketplace add max-sixty/worktrunk
codex plugin add worktrunk@worktrunk
```

`wt config plugins codex uninstall` removes the plugin and its marketplace entry.

### OpenCode

```bash
wt config plugins opencode install
```

This writes the activity-tracking plugin to OpenCode's global plugins directory, `~/.config/opencode/plugins/worktrunk.ts` (honoring `$OPENCODE_CONFIG_DIR` and `$XDG_CONFIG_HOME`). `wt config plugins opencode uninstall` removes it.

### Pi

```bash
wt config plugins pi install
```

This writes the activity extension to `~/.pi/agent/extensions/worktrunk.ts`, where Pi discovers it on startup; `$PI_CODING_AGENT_DIR` replaces the agent directory. `wt config plugins pi uninstall` removes the extension.

### oh-my-pi

```bash
wt config plugins omp install
```

oh-my-pi (`omp`) is a Pi-derived agent with its own config root and hook API, so it takes a separate command. This writes the activity hook to `~/.omp/agent/hooks/pre/worktrunk.ts`. Named `$OMP_PROFILE` or `$PI_PROFILE` profiles use `~/.omp/profiles/<profile>/agent`, `$PI_CONFIG_DIR` changes the `.omp` config root, and `$PI_CODING_AGENT_DIR` overrides the agent directory for the default profile. `wt config plugins omp uninstall` removes the hook.

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

Every plugin tracks agent sessions with status markers in `wt list`:

```console
$ wt list
  Branch       Status      HEAD±     main↕    main…±    Remote⇅  Commit    Age  Message
@ main             ^⇡                                    ⇡1      33323bc    1d  Initial commit
+ feature-api      ↑ 🤖              ↑1        +1                70343f0    1d  Add REST API endpo…
+ review-ui      ? ↑ 💬    +1        ↑1        +1                a585d6e    1d  Add dashboard comp…
+ wip-docs       ? –       +1                                    33323bc    1d  Initial commit

○ Showing 4 worktrees, 2 with changes, 2 ahead, hidden: Path
```

- 🤖 — agent is working
- 💬 — agent is waiting or idle

Every plugin clears the marker when a session ends. A stale marker can remain if the agent process is killed before its session-end hook runs. In every case, `wt config state marker clear` removes a marker manually.

### Manual status markers

Set status markers manually for any workflow:

```console
$ wt config state marker set "🚧"                   # Current branch
$ wt config state marker set "✅" --branch feature  # Specific branch
$ git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct
```

### Agent CLIs without a plugin

Activity tracking is not plugin-specific. The plugins above only call `wt` on their host's session events, and the marker itself is plain git config — so any CLI that can run a command on session lifecycle events drives the same 🤖/💬 markers with no worktrunk plugin:

| Host event | Command |
|---|---|
| Session starts, or the agent resumes work | `wt config state marker set "🤖"` |
| Agent finishes a turn and waits for input | `wt config state marker set "💬"` |
| Session ends | `wt config state marker clear` |

## Worktree isolation (Claude Code only)

Claude Code agents can run in isolated worktrees (`isolation: "worktree"`). By default, Claude Code creates these with `git worktree add`. The plugin's `WorktreeCreate` and `WorktreeRemove` hooks route this through `wt switch --create` and `wt remove` instead, so worktrees created by agents get worktrunk's naming conventions, hooks, and lifecycle management.

## `/wt-switch-create` skill (Claude Code only)

`/wt-switch-create [<branch>] [<repo>] [-- <task>]` starts a task in a fresh worktree without leaving the session: it creates the worktree, switches into it, and runs the task (all arguments optional). The worktree shows up in `wt list`; merge or remove it with `wt merge` / `wt remove`.

Claude Code asks for confirmation before a session enters an existing worktree by path outside `.claude/worktrees/`, which in worktrunk's default layout is every worktree. The plugin's `PermissionRequest` hook answers yes for a worktree of the repository the session is working in that sits where `worktree-path` puts its branch, so a background session doesn't stop at that prompt. Entering any other worktree still asks.

## Statusline (Claude Code only)

`wt list statusline --format=claude-code` outputs a single-line status for the Claude Code statusline. Claude Code runs it in the background, which is what makes the occasional 1–2 second CI fetch invisible.

<code>~/w/myproject.feature-auth  !🤖  @<span style='color:#0a0'>+42</span> <span style='color:#a00'>-8</span>  <span style='color:#0a0'>↑3</span>  <span style='color:#0a0'>⇡1</span>  <span style='color:#0a0'>#3035</span>  Opus  🌔 65%  <span style='color:#a70'>1.4×(10am–3pm)</span></code>

Worktree state comes from the same cells [`wt list`](https://worktrunk.dev/list/) renders; Claude Code's stdin JSON adds the model, the `🌔 65%` context gauge, and the rate-limit pace notice. [`wt list statusline`](https://worktrunk.dev/list/#wt-list-statusline) documents every segment, how the links behave, and the JSON fields behind them.

Add to `~/.claude/settings.json`:

```json title="~/.claude/settings.json"
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}
```
