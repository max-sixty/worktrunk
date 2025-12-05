+++
title = "wt select"
weight = 14

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt select --help-page` — edit src/cli.rs to update -->

Interactive worktree picker with live preview. Navigate worktrees with keyboard shortcuts and press Enter to switch.

![wt select demo](/assets/wt-select.gif)

## Examples

Open the selector:

```bash
wt select
```

## Preview tabs

Toggle between views with number keys:

1. **HEAD±** — Uncommitted changes
2. **history** — Recent commits on the branch
3. **main…±** — Changes relative to main branch

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` or `j`/`k` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` or `q` | Cancel |
| `/` | Filter worktrees |
| `1`/`2`/`3` | Switch preview tab |
| `Alt+p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

## See also

- [wt list](@/list.md) — Static table view with all worktree metadata
- [wt switch](@/switch.md) — Direct switching when you know the target branch

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt select --help-page` — edit cli.rs to update -->

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
