# wt select

Interactive worktree selector. Browse and switch worktrees with live preview.

## Examples

Open the selector:

```bash
wt select
```

## Preview tabs

Toggle between views with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Diff vs upstream tracking branch (ahead/behind)

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` | Cancel |
| (type) | Filter worktrees |
| `1`/`2`/`3`/`4` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

With `--branches`, branches without worktrees are included — selecting one creates a worktree. This matches `wt list --branches`.

## Configuration

### Pager

The preview panel pipes diff output through git's pager (typically `less` or `delta`). Override pager behavior in user config:

```toml
[select]
pager = "delta --paging=never"
```

This is useful when the default pager doesn't render correctly in the embedded preview panel.

## Command reference

wt select - Interactive worktree selector

Browse and switch worktrees with live preview.

Usage: <b><span class=c>wt select</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--branches</span></b>
          Include branches without worktrees

      <b><span class=c>--remotes</span></b>
          Include remote branches

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
