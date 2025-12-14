+++
title = "wt select"
weight = 14

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt select --help-page` — edit cli.rs to update -->

Interactive worktree picker with live preview. Navigate worktrees with keyboard shortcuts and press Enter to switch.

<figure class="demo">
<picture>
  <source srcset="/assets/wt-select-dark.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/wt-select.gif" alt="wt select demo" width="1600" height="800">
</picture>
</figure>

## Examples

Open the selector:

```bash
wt select
```

## Preview tabs

Toggle between views with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on main have dimmed hashes
3. **main…±** — Diff of all changes vs main branch

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` | Cancel |
| (type) | Filter worktrees |
| `1`/`2`/`3` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

Branches without worktrees are included — selecting one creates a worktree. (`wt list` requires `--branches` to show them.)

## See also

- [wt list](@/list.md) — Static table view with all worktree metadata
- [wt switch](@/switch.md) — Direct switching when you know the target branch

## Command reference

```
wt select - Interactive worktree selector

Toggle preview tabs with 1/2/3 keys. Toggle preview visibility with alt-p.

Usage: wt select [OPTIONS]

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

<!-- END AUTO-GENERATED from `wt select --help-page` -->
