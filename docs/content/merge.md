+++
title = "wt merge"
description = "Merge current branch into the target branch. Squash & rebase, fast-forward the target branch, remove the worktree."
weight = 13

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-page` — edit cli.rs to update -->

Merge current branch into the target branch. Squash & rebase, fast-forward the target branch, remove the worktree.

Unlike `git merge`, this merges the current branch into the target branch — not the target into current. Similar to clicking "Merge pull request" on GitHub, but locally. The target defaults to the default branch.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-merge.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-merge.gif" alt="wt merge demo" width="1600" height="900">
</picture>
</figure>

## Examples

Merge to the default branch:

{% terminal(cmd="wt merge") %}
<span class=c>◎</span> <span class=c>Running pre-merge <b>project:test</b></span>
<span style='background:var(--bright-white,#fff)'> </span> <span class=d><span style='color:var(--blue,#00a)'>cargo</span></span><span class=d> nextest run</span>
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.02s
     Summary [   0.002s] 2 tests run: 2 passed, 0 skipped
<span class=c>◎</span> <span class=c>Merging 1 commit to <b>main</b> @ <span class=d>a1b2c3d</span> (no commit/squash/rebase needed)</span>
<span style='background:var(--bright-white,#fff)'> </span> * <span style='color:var(--yellow,#a60)'>a1b2c3d</span> feat: add hook registration
<span style='background:var(--bright-white,#fff)'> </span>  hook.rs | 31 <span class=g>+++++++++++++++++++++++++++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>  1 file changed, 31 insertions(+)
<span class=g>✓</span> <span class=g>Merged to <b>main</b> <span style='color:var(--bright-black,#555)'>(1 commit, 1 file, +31</span></span><span style='color:var(--bright-black,#555)'>)</span>
<span class=c>◎</span> <span class=c>Removing <b>hooks</b> worktree &amp; branch in background (same commit as <b>main</b>,</span> <span class=d>_</span><span class=c>)</span>
<span class=d>○</span> Switched to worktree for <b>main</b> @ <b>~/repo</b>
{% end %}

Merge to a different branch:

{{ terminal(cmd="wt merge develop") }}

Keep the worktree after merging:

{{ terminal(cmd="wt merge --no-remove") }}

Preserve commit history (no squash):

{{ terminal(cmd="wt merge --no-squash") }}

Create a merge commit — semi-linear history:

{{ terminal(cmd="wt merge --no-ff") }}

Skip committing/squashing (rebase still runs unless --no-rebase):

{{ terminal(cmd="wt merge --no-commit") }}

## Pipeline

`wt merge` runs these steps:

1. **Commit** — Pre-commit hooks run, then uncommitted changes are committed. Post-commit hooks run in background. Skipped when squashing (the default) — changes are staged during the squash step instead. With `--no-squash`, this is the only commit step.
2. **Squash** — Combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, individual commits are preserved.
3. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
4. **Pre-merge hooks** — Hooks run after rebase, before merge. Failures abort. See [`wt hook`](@/hook.md).
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
[[pre-merge]]
test = "cargo test"
lint = "cargo clippy"
```

## See also

- [`wt step`](@/step.md) — Run individual operations (commit, squash, rebase, push)
- [`wt remove`](@/remove.md) — Remove worktrees without merging
- [`wt switch`](@/switch.md) — Navigate to other worktrees

## Command reference

{% terminal() %}
wt merge - Merge current branch into the target branch

Squash &amp; rebase, fast-forward the target branch, remove the worktree.

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

      <b><span class=c>--no-ff</span></b>
          Create a merge commit (no fast-forward)

      <b><span class=c>--stage</span></b><span class=c> &lt;STAGE&gt;</span>
          What to stage before committing [default: all]

          Possible values:
          - <b><span class=c>all</span></b>:     Stage everything: untracked files + unstaged tracked changes
          - <b><span class=c>tracked</span></b>: Stage tracked changes only (like <b>git add -u</b>)
          - <b><span class=c>none</span></b>:    Stage nothing, commit only what&#39;s already in the index

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Automation:</span></b>
      <b><span class=c>--no-hooks</span></b>
          Skip hooks

      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format

          JSON prints structured result to stdout after merge completes.

          Possible values:
          - <b><span class=c>text</span></b>: Human-readable text output
          - <b><span class=c>json</span></b>: JSON output

          [default: text]

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: info logs + hook/alias template variable &amp; output; -vv: debug logs +
          diagnostic report + trace.log/output.log under .git/wt/logs/)

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts
{% end %}

<!-- END AUTO-GENERATED from `wt merge --help-page` -->
