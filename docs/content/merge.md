+++
title = "wt merge"
weight = 13

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-page` — edit cli.rs to update -->

When already on the target branch or in the main worktree, the worktree is preserved automatically.

## Examples

Basic merge to main:

```bash
wt merge
```

Merge to a different branch:

```bash
wt merge develop
```

Keep the worktree after merging:

```bash
wt merge --no-remove
```

Preserve commit history (no squash):

```bash
wt merge --no-squash
```

Skip git operations, only run hooks and push:

```bash
wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Squash** — Stages uncommitted changes, then combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, uncommitted changes are committed separately and individual commits are preserved.
2. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
3. **Pre-merge hooks** — Project commands run after rebase, before merge. Failures abort. See [wt hook](@/hook.md).
4. **Merge** — Fast-forward merge to the target branch. Non-fast-forward merges are rejected.
5. **Pre-remove hooks** — Project commands run before removing worktree. Failures abort.
6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree.
7. **Post-merge hooks** — Project commands run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip all git operations (steps 1-2) and only run hooks and merge. Useful after preparing commits manually with `wt step`. Requires a clean working tree.

## See also

- [wt step](@/step.md) — Run individual merge steps (commit, squash, rebase, push)
- [wt remove](@/remove.md) — Remove worktrees without merging
- [wt switch](@/switch.md) — Navigate to other worktrees

---

## Command reference

```
wt merge - Merge worktree into target branch

Squashes commits, rebases, runs hooks, merges to target, and removes the
worktree.

Usage: wt merge [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --no-squash
          Skip commit squashing

      --no-commit
          Skip commit, squash, and rebase

      --no-remove
          Keep worktree after merge

      --no-verify
          Skip hooks

  -f, --force
          Skip approval prompts

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked
            changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

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

<!-- END AUTO-GENERATED from `wt merge --help-page` -->
