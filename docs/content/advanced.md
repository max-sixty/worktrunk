+++
title = "Advanced Features"
weight = 5
+++

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

`wt select` opens a fzf-like fuzzy-search worktree picker with diff preview.

### Preview tabs

Toggle with number keys:

1. **Tab 1**: Working tree changes (uncommitted)
2. **Tab 2**: Commit history (commits not on main highlighted)
3. **Tab 3**: Branch diff (changes ahead of main)

## Tips & Patterns

### Alias for new worktree + agent

```bash
alias wsc='wt switch --create --execute=claude'
wsc new-feature  # Creates worktree, runs hooks, launches Claude
```

### Eliminate cold starts

`post-create` hooks install deps and copy caches. Use copy-on-write on macOS:

```toml
[post-create]
"cache" = "cp -c -r ../.cache .cache"  # Uses APFS clones
"install" = "npm ci"
```

### Local CI gate

`pre-merge` hooks run before merging. Failures abort:

```toml
[pre-merge]
"test" = "cargo test"
"lint" = "cargo clippy -- -D warnings"
```

### Monitor CI across branches

```bash
$ wt list --full --branches
```

Shows PR/CI status for all branches, including those without worktrees.

### JSON API

```bash
$ wt list --format=json
```

For dashboards, statuslines, and scripts.

### Task runners in hooks

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

### Stacked branches

```bash
$ wt switch --create feature-part2 --base=@
```

Branches from current HEAD, not main.
