+++
title = "wt merge"
weight = 13

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-page` — edit src/cli.rs to update -->

Merge the current branch into the target branch and clean up. Handles the full workflow: commit uncommitted changes, squash commits, rebase, run hooks, push to target, and remove the worktree.

When already on the target branch or in the main worktree, the worktree is preserved automatically.

## Examples

Basic merge to main:

```bash
wt merge
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

1. **Commit** — Stages and commits uncommitted changes. Commit messages are LLM-generated. Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`.

2. **Squash** — Combines all commits into one (like GitHub's "Squash and merge"). Skip with `--no-squash` to preserve individual commits. A backup ref is saved to `refs/wt-backup/<branch>`.

3. **Rebase** — Rebases onto the target branch. Conflicts abort immediately.

4. **Pre-merge hooks** — Project commands run after rebase, before push. Failures abort. See [Hooks](@/hooks.md).

5. **Push** — Fast-forward push to the target branch. Non-fast-forward pushes are rejected.

6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree.

7. **Post-merge hooks** — Project commands run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip steps 1-3 and only run hooks and push. Requires a clean working tree and `--no-remove`.

## See also

- [wt step](@/step.md) — Run individual merge steps (commit, squash, rebase, push)
- [wt remove](@/remove.md) — Remove worktrees without merging
- [wt switch](@/switch.md) — Navigate to other worktrees

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-page` — edit cli.rs to update -->

```
wt merge - Merge worktree into target branch
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
          Skip all project hooks

  -f, --force
          Skip approval prompts

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
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
