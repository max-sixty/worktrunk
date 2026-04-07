# wt merge

Merge current branch into the target branch. Squash & rebase, fast-forward the target branch, remove the worktree.

Unlike `git merge`, this merges the current branch into the target branch — not the target into current. Similar to clicking "Merge pull request" on GitHub, but locally. The target defaults to the default branch.

## Examples

Merge to the default branch:

```bash
$ wt merge
```

Merge to a different branch:

```bash
$ wt merge develop
```

Keep the worktree after merging:

```bash
$ wt merge --no-remove
```

Preserve commit history (no squash):

```bash
$ wt merge --no-squash
```

Create a merge commit — semi-linear history:

```bash
$ wt merge --no-ff
```

Skip committing/squashing (rebase still runs unless --no-rebase):

```bash
$ wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Commit** — Pre-commit hooks run, then uncommitted changes are committed. Post-commit hooks run in background. Skipped when squashing (the default) — changes are staged during the squash step instead. With `--no-squash`, this is the only commit step.
2. **Squash** — Combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, individual commits are preserved.
3. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
4. **Pre-merge hooks** — Hooks run after rebase, before merge. Failures abort. See [`wt hook`](https://worktrunk.dev/hook/).
5. **Merge** — Fast-forward merge to the target branch. With `--no-ff`, a merge commit is created instead — semi-linear history with rebased commits plus a merge commit. Non-fast-forward merges are rejected.
6. **Pre-remove hooks** — Hooks run before removing worktree. Failures abort.
7. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree. When already on the target branch or in the primary worktree, the worktree is preserved.
8. **Post-remove + post-merge hooks** — Run in background after cleanup.

Use `--no-commit` to skip committing uncommitted changes and squashing; rebase still runs by default and can rewrite commits unless `--no-rebase` is passed. Useful after preparing commits manually with `wt step commit`. Requires a clean working tree.

## Local CI

For personal projects, pre-merge hooks open up the possibility of a workflow with much faster iteration — an order of magnitude more small changes instead of fewer large ones.

Historically, ensuring tests ran before merging was difficult to enforce locally. Remote CI was valuable for the process as much as the checks: it guaranteed validation happened. `wt merge` brings that guarantee local.

The full workflow: start an agent (one of many) on a task, work elsewhere, return when it's ready. Review the diff, run `wt merge`, move on. Pre-merge hooks validate before merging — if they pass, the branch goes to the default branch and the worktree cleans up.

```toml
[pre-merge]
test = "cargo test"
lint = "cargo clippy"
```

## Command reference

```
wt merge - Merge current branch into the target branch

Squash & rebase, fast-forward the target branch, remove the worktree.

Usage: wt merge [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --no-squash
          Skip commit squashing

      --no-commit
          Skip commit and squash

      --no-rebase
          Skip rebase (fail if not already rebased)

      --no-remove
          Keep worktree after merge

      --no-ff
          Create a merge commit (no fast-forward)

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

  -h, --help
          Print help (see a summary with '-h')

Automation:
  -y, --yes
          Skip approval prompts

      --no-hooks
          Skip hooks

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```
