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
  <source srcset="/assets/docs/dark/wt-select.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-select.gif" alt="wt select demo" width="1600" height="800">
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
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch

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
- [wt switch](@/switch.md) — Direct switching to a known target branch

## Command reference

{% terminal() %}
wt select - Interactive worktree selector

Browse and switch worktrees with live preview.

Usage: <b><span class=c>wt select</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

<!-- END AUTO-GENERATED from `wt select --help-page` -->
