+++
title = "Advanced Features"
weight = 23

[extra]
group = "Reference"
+++

Most Worktrunk users get everything they need from `wt switch`, `wt list`, `wt merge`, and `wt remove`. The features below are optional power-user capabilities.

## Claude Code integration

Worktrunk includes a Claude Code plugin for tracking agent status across worktrees.

### Status tracking

The plugin adds status indicators to `wt list`:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__with_user_marker.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt list
  <b>Branch</b>       <b>Status</b>         <b>HEAD±</b>    <b>main↕</b>  <b>Path</b>                <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ <b>main</b>             <span class=d>^</span>                          <b>./repo</b>                       <span class=d>b834638e</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span>  🤖              <span class=g>↑1</span>      ./repo.feature-api           <span class=d>9606cd0f</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span>  💬              <span class=g>↑1</span>      ./repo.review-ui             <span class=d>afd3b353</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ <span class=d>wip-docs</span>       <span class=c>?</span><span class=d>_</span>                           <span class=d>./repo.wip-docs</span>              <span class=d>b834638e</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

⚪ <span class=d>Showing 4 worktrees, 2 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

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
$ wt config var set marker "🚧"                   # Current branch
$ wt config var set marker "✅" --branch feature  # Specific branch
$ git config worktrunk.marker.feature "💬"        # Direct git config
```

## Statusline integration

`wt list statusline` outputs a single-line status for shell prompts, starship, or editor integrations.[^1]

[^1]: Currently this grabs CI status, so is too slow to use in synchronous contexts. If a faster version would be helpful, please [open an issue](https://github.com/max-sixty/worktrunk/issues).

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

## Interactive worktree picker

`wt select` opens a fuzzy-search worktree picker with diff preview (Unix only).

Type to filter, use arrow keys or `j`/`k` to navigate, Enter to switch. Preview tabs show working tree changes, commit history, or branch diff — toggle with `1`/`2`/`3`.

See [wt select](@/select.md) for full keyboard shortcuts and details.
