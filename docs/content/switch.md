+++
title = "wt switch"
weight = 10

[extra]
group = "Commands"
+++

Two distinct operations:

- **Switch to existing worktree** — Changes directory, nothing else
- **Create new worktree** (`--create`) — Creates branch and worktree, runs [hooks](/hooks/)

## Examples

```bash
wt switch feature-auth           # Switch to existing worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create branch and worktree
wt switch --create hotfix --base production
```

For interactive selection, use [`wt select`](/select/).

## Shortcuts

| Symbol | Meaning |
|--------|---------|
| `-` | Previous worktree |
| `@` | Current branch's worktree |
| `^` | Default branch worktree |

```bash
wt switch -                      # Back to previous
wt switch ^                      # Main worktree
wt switch --create fix --base=@  # Branch from current HEAD
```

## Creating Worktrees

With `--create`, worktrunk:

1. Creates branch from `--base` (defaults to default branch)
2. Creates worktree at configured path
3. Runs [post-create hooks](/hooks/#post-create) (blocking)
4. Switches to new directory
5. Spawns [post-start hooks](/hooks/#post-start) (background)

```bash
wt switch --create api-refactor
wt switch --create fix --base release-2.0
wt switch --create docs --execute "code ."
wt switch --create temp --no-verify      # Skip hooks
```

## Path-First Lookup

Arguments resolve by checking the filesystem before git branches:

1. Compute expected path from argument (using configured path template)
2. If worktree exists at that path, switch to it
3. Otherwise, treat argument as branch name

**Edge case**: If `repo.foo/` exists but tracks branch `bar`:
- `wt switch foo` → switches to `repo.foo/` (the `bar` worktree)
- `wt switch bar` → also works (branch lookup finds same worktree)

## See Also

- [wt select](/select/) — Interactive worktree selection
- [wt list](/list/) — View all worktrees
- [wt remove](/remove/) — Delete worktrees when done
- [wt merge](/merge/) — Integrate changes back to main

---

## Command Reference

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-page` — edit cli.rs to update -->

```
wt switch - Switch to a worktree
Usage: wt switch [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>
          Branch or worktree name

          Shortcuts: '^' (main), '-' (previous), '@' (current)

Options:
  -c, --create
          Create a new branch

  -b, --base <BASE>
          Base branch

          Defaults to default branch.

  -x, --execute <EXECUTE>
          Command to run after switch

  -f, --force
          Skip approval prompts

      --no-verify
          Skip all project hooks

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
