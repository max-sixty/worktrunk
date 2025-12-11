+++
title = "wt step"
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

Run individual git workflow operations: commits, squashes, rebases, and pushes.

## Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Manual merge workflow with review between steps:

```bash
wt step commit
wt step squash
# Review the squashed commit
wt step rebase
wt step push
```

## Operations

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Push to target branch (default: main)
- `for-each` — [experimental] Run a command in every worktree

## See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [wt hook](@/hook.md) — Run hooks independently

---

## Command reference

```
wt step - Run individual workflow operations

Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit    Commit changes with LLM commit message
  squash    Squash commits down to target
  push      Push changes to local target branch
  rebase    Rebase onto target
  for-each  [experimental] Run command in each worktree

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

<!-- END AUTO-GENERATED from `wt step --help-page` -->
