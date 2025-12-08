+++
title = "wt step"
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit src/cli.rs to update -->

Run individual workflow operations: commits, squashes, rebases, pushes, and [hooks](@/hooks.md).

## Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Run pre-merge hooks in CI:

```bash
wt step pre-merge --force
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

**Git operations:**

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Push to target branch (default: main)

**Hooks** — run project commands defined in [`.config/wt.toml`](@/hooks.md):

- `post-create` — After worktree creation (blocking)
- `post-start` — After worktree creation (background)
- `pre-commit` — Before committing
- `pre-merge` — Before pushing to target
- `post-merge` — After merge cleanup

## See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

```
wt step - Run individual workflow operations
Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit       Commit changes with LLM commit message
  squash       Squash commits down to target
  push         Push changes to local target branch
  rebase       Rebase onto target
  post-create  Run post-create hook
  post-start   Run post-start hook
  pre-commit   Run pre-commit hook
  pre-merge    Run pre-merge hook
  post-merge   Run post-merge hook
  pre-remove   Run pre-remove hook

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
