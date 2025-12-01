+++
title = "Advanced Features"
weight = 6
+++

Most Worktrunk users get everything they need from `wt switch`, `wt list`, `wt merge`, and `wt remove`. The features below are optional power-user capabilities.

## Claude Code Integration

Worktrunk includes a Claude Code plugin for tracking agent status across worktrees.

### Status tracking

The plugin adds status indicators to `wt list`:

```bash
$ wt list
  Branch       Status         HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ main             ^                          ./repo                       b834638e  1d    Initial commit
+ feature-api      ↑  🤖              ↑1      ./repo.feature-api           9606cd0f  1d    Add REST API endpoints
+ review-ui      ? ↑  💬              ↑1      ./repo.review-ui             afd3b353  1d    Add dashboard component
+ wip-docs       ?_                           ./repo.wip-docs              b834638e  1d    Initial commit
```

- `🤖` — Claude is working
- `💬` — Claude is waiting for input

### Install the plugin

```bash
$ claude plugin marketplace add max-sixty/worktrunk
$ claude plugin install worktrunk@worktrunk
```

### Manual status markers

Set status markers manually for any workflow:

```bash
$ wt config status set "🚧"                    # Current branch
$ wt config status set "✅" --branch feature   # Specific branch
$ git config worktrunk.status.feature "💬"     # Direct git config
```

## Statusline Integration

`wt list statusline` outputs a single-line status for shell prompts, starship, or editor integrations.

### Claude Code statusline

For Claude Code, outputs directory, branch status, and model:

```
~/w/myproject.feature-auth  !🤖  ±+42 -8  ↑3  ⇡1  ●  | Opus
```

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --claude-code"
  }
}
```

## Interactive Worktree Picker

`wt select` opens a fuzzy-search worktree picker with diff preview (Unix only).

Type to filter, use arrow keys or `j`/`k` to navigate, Enter to switch. Preview tabs show working tree changes, commit history, or branch diff — toggle with `1`/`2`/`3`.

See [wt select](/commands/#wt-select) for full keyboard shortcuts and details.
