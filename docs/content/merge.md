+++
title = "wt merge"
weight = 13

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-page` — edit cli.rs to update -->

Run from a feature worktree to merge into the default branch — like clicking "Merge pull request" on GitHub.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-merge.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-merge.gif" alt="wt merge demo" width="1600" height="900">
</picture>
</figure>

## Examples

Merge to the default branch:

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

Skip committing/squashing (rebase still runs unless --no-rebase):

```bash
wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Squash** — Stages uncommitted changes, then combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, uncommitted changes become a separate commit and individual commits are preserved.
2. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
3. **Pre-merge hooks** — Project commands run after rebase, before merge. Failures abort. See [wt hook](@/hook.md).
4. **Merge** — Fast-forward merge to the target branch. Non-fast-forward merges are rejected.
5. **Pre-remove hooks** — Project commands run before removing worktree. Failures abort.
6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree. When already on the target branch or in the main worktree, the worktree is preserved.
7. **Post-merge hooks** — Project commands run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip committing uncommitted changes and squashing; rebase still runs by default and can rewrite commits unless you pass `--no-rebase`. Useful after preparing commits manually with `wt step`. Requires a clean working tree.

## Local CI

For personal projects, pre-merge hooks open up the possibility of a workflow with much faster iteration — an order of magnitude more small changes instead of fewer large ones.

Historically, ensuring tests ran before merging was difficult to enforce locally. Remote CI was valuable for the process as much as the checks: it guaranteed validation happened. `wt merge` brings that guarantee local.

The full workflow: start an agent (one of many) on a task, work elsewhere, return when it's ready. Review the diff, run `wt merge`, move on. Pre-merge hooks validate before merging — if they pass, the branch goes to the default branch and the worktree cleans up.

```toml
[pre-merge]
test = "cargo test"
lint = "cargo clippy"
```

## See also

- [wt step](@/step.md) — Run individual merge steps (commit, squash, rebase, push)
- [wt remove](@/remove.md) — Remove worktrees without merging
- [wt switch](@/switch.md) — Navigate to other worktrees

## Command reference

{% terminal() %}
wt merge - Merge worktree into target branch

Squashes commits, rebases, runs hooks, merges to target, and removes the
worktree.

Usage: <b><span class=c>wt merge</span></b> <span class=c>[OPTIONS]</span> <span class=c>[TARGET]</span>

<b><span class=g>Arguments:</span></b>
  <span class=c>[TARGET]</span>
          Target branch

          Defaults to default branch.

<b><span class=g>Options:</span></b>
      <b><span class=c>--no-squash</span></b>
          Skip commit squashing

      <b><span class=c>--no-commit</span></b>
          Skip commit and squash

      <b><span class=c>--no-rebase</span></b>
          Skip rebase (fail if not already rebased)

      <b><span class=c>--no-remove</span></b>
          Keep worktree after merge

      <b><span class=c>--no-verify</span></b>
          Skip hooks

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked
            changes
          - <b><span class=c>tracked</span></b>: Stage tracked changes only (like <b>git add -u</b>)
          - <b><span class=c>none</span></b>:    Stage nothing, commit only what&#39;s already in the index

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

<!-- END AUTO-GENERATED from `wt merge --help-page` -->
