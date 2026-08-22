# Worktrunk

Worktrunk is a CLI for Git worktree management, designed for **parallel AI agent
workflows**.

Worktrunk's three core commands make worktrees as easy as branches.
Plus, Worktrunk has a bunch of quality-of-life features to simplify working
with many parallel changes, including hooks to automate local workflows.

A quick demo:

## Context: git worktrees

AI agents like Claude Code and Codex can handle longer tasks without
supervision, such that it's possible to manage 5-10+ in parallel. Git's native
worktree feature give each agent its own working directory, so they don't step
on each other's changes.

But the git worktree UX is clunky. Even a task as small as starting a new
worktree requires typing the branch name three times: `git worktree add -b feat
../repo.feat`, then `cd ../repo.feat`.

## Worktrunk makes git worktrees as easy as branches

Worktrees are addressed by branch name; paths are computed from a configurable template. Commands that take a branch also accept the path of the worktree it is checked out in.

<p class="workflow-stage">Start with the core commands</p>

**Core commands:**

<table class="cmd-compare">
  <thead>
    <tr>
      <th>Task</th>
      <th>Worktrunk</th>
      <th>Plain git</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Switch worktrees</td>
      <td data-label="Worktrunk"><code>wt switch feat</code></td>
      <td data-label="Plain git"><code>cd ../repo.feat</code></td>
    </tr>
    <tr>
      <td>Create + start Claude</td>
      <td data-label="Worktrunk"><code>wt switch -c -x claude feat</code></td>
      <td data-label="Plain git"><pre><code>git worktree add -b feat ../repo.feat && \
cd ../repo.feat && \
claude</code></pre></td>
    </tr>
    <tr>
      <td>Clean up</td>
      <td data-label="Worktrunk"><code>wt remove</code></td>
      <td data-label="Plain git"><pre><code>cd ../repo && \
git worktree remove ../repo.feat && \
git branch -d feat</code></pre></td>
    </tr>
    <tr>
      <td>List with status</td>
      <td data-label="Worktrunk"><code>wt list</code></td>
      <td data-label="Plain git"><span class="cmd-compare-value"><code>git worktree list</code> (paths only)</span></td>
    </tr>
  </tbody>
</table>

<p class="workflow-stage">Expand into the more advanced commands as needed</p>

<p class="workflow-heading"><strong>Workflow automation:</strong></p>

- **[Hooks](https://worktrunk.dev/hook/)** — run commands on create, pre-merge, post-merge, etc
- **[LLM commit messages](https://worktrunk.dev/llm-commits/)** — generate commit messages from diffs
- **[Merge workflow](https://worktrunk.dev/merge/)** — squash, rebase, merge, clean up in one command
- **[Interactive picker](https://worktrunk.dev/switch/#interactive-picker)** — browse worktrees with live diff and log previews
- **[Copy build caches](https://worktrunk.dev/step/#wt-step-copy-ignored)** — skip cold starts by sharing `target/`, `node_modules/`, etc between worktrees
- **[`wt list --full`](https://worktrunk.dev/list/#full-mode)** — [CI status](https://worktrunk.dev/list/#ci-status) and [AI-generated summaries](https://worktrunk.dev/list/#llm-summaries) per branch
- **[PR checkout](https://worktrunk.dev/switch/#pull-requests-and-merge-requests)** — `wt switch pr:123` to jump straight to a PR's branch
- **[Dev server per worktree](https://worktrunk.dev/tips-patterns/#dev-server-per-worktree)** — `hash_port` template filter gives each worktree a unique port
- **[Aliases](https://worktrunk.dev/extending/#aliases) & [per-branch variables](https://worktrunk.dev/config/#wt-config-state-vars)** — custom `wt <name>` commands and branch-scoped state for hook templates
- ...and **[lots more](#next-steps)**

Multiple parallel agents, same simple commands:

## Install

**Homebrew (macOS & Linux):**

```bash
brew install worktrunk && wt config shell install
```

Shell integration allows commands to change directories.

**Cargo:**

```bash
cargo install worktrunk && wt config shell install
```

<details>
<summary><strong>Windows & other</strong></summary>

**Windows.** `wt` defaults to Windows Terminal's command, so Winget additionally installs Worktrunk as `git-wt` to avoid the conflict:

```bash
winget install max-sixty.worktrunk
git-wt config shell install
```

Alternatively, disable Windows Terminal's alias (Settings → Apps → Advanced app settings → App execution aliases → "Terminal"/"Terminal Preview") to use `wt` directly.

> Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/) — [policy](https://worktrunk.dev/code-signing/).

**Arch Linux:**

```bash
sudo pacman -S worktrunk && wt config shell install
```

**Conda / Pixi** (community-maintained [feedstock](https://github.com/conda-forge/worktrunk-feedstock)):

```bash
conda install -c conda-forge worktrunk && wt config shell install
```

Or with [Pixi](https://pixi.sh): `pixi global install worktrunk && wt config shell install`.

</details>

## Quick start

Create a worktree for a new feature:

```console
$ wt switch --create feature-auth
✓ Created branch feature-auth from main and worktree @ ~/repo.feature-auth
```

This creates a new branch and worktree, then switches to it. Do your work, then check all worktrees with [`wt list`](https://worktrunk.dev/list/):

```console
$ wt list
  Branch        Status        HEAD±    main↕     main…±  Remote⇅  Commit   Age   Message
@ feature-auth  +   ↑      +27   -8   ↑1       +31                4bc72dc  2h    Add authenticati…
^ main              ^⇡                                    ⇡1      0e631ad  1d    Initial commit

○ Showing 2 worktrees, 1 with changes, 1 ahead, 1 column hidden
```

The `@` marks the current worktree. `+` means staged changes, `↑1` means 1 commit ahead of main, `⇡` means unpushed commits.

When done, either:

**PR workflow** — commit, push, open a PR, merge via GitHub/GitLab, then clean up:

```bash
wt step commit                    # commit staged changes
gh pr create                      # or glab mr create
wt remove                         # after PR is merged
```

**Local merge** — squash, rebase onto main, fast-forward merge, clean up:

```console
$ wt merge main
◎ Generating commit message and committing changes... (2 files, +53, no squashing needed)
  Add authentication module
✓ Committed changes @ a1b2c3d
◎ Merging 1 commit to main @ a1b2c3d (no rebase needed)
  * a1b2c3d Add authentication module
   auth.rs | 51 +++++++++++++++++++++++++++++++++++++++++++++++++++
   lib.rs  |  2 ++
   2 files changed, 53 insertions(+)
✓ Merged to main (1 commit, 2 files, +53)
◎ Removing feature-auth worktree & branch in background (same commit as main, _)
○ Switched to worktree for main @ ~/repo
```

For parallel agents, create multiple worktrees and launch an agent in each:

```bash
wt switch -x claude -c feature-a -- 'Add user authentication'
wt switch -x claude -c feature-b -- 'Fix the pagination bug'
wt switch -x claude -c feature-c -- 'Write tests for the API'
```

The `-x` flag runs a command after switching; arguments after `--` are passed to it. Configure [post-start hooks](https://worktrunk.dev/hook/#hook-types) to automate setup (install deps, start dev servers).

## Next steps

- Learn the core commands: [`wt switch`](https://worktrunk.dev/switch/), [`wt list`](https://worktrunk.dev/list/), [`wt merge`](https://worktrunk.dev/merge/), [`wt remove`](https://worktrunk.dev/remove/)
- Set up [hooks](https://worktrunk.dev/hook/) for automated setup
- Explore [LLM commit messages](https://worktrunk.dev/llm-commits/), [interactive
  picker](https://worktrunk.dev/switch/#interactive-picker), [Claude Code integration](https://worktrunk.dev/claude-code/), [CI
  status & PR links](https://worktrunk.dev/list/#ci-status)
- Browse [tips & patterns](https://worktrunk.dev/tips-patterns/) for recipes: aliases, dev servers, databases, agent handoffs, and more
- [Extending Worktrunk](https://worktrunk.dev/extending/) — customize workflows with hooks & aliases
- Run `wt --help` or `wt <command> --help` for quick CLI reference

## Further reading

- [Claude Code: Best practices for agentic coding](https://www.anthropic.com/engineering/claude-code-best-practices) — Anthropic's official guide, including the worktree pattern
- [Shipping faster with Claude Code and Git Worktrees](https://incident.io/blog/shipping-faster-with-claude-code-and-git-worktrees) — incident.io's workflow for parallel agents
- [Git worktree pattern discussion](https://github.com/anthropics/claude-code/issues/1052) — Community discussion in the Claude Code repo
- [@DevOpsToolbox's video on Worktrunk](https://youtu.be/WBQiqr6LevQ?t=345)
- [git-worktree documentation](https://git-scm.com/docs/git-worktree) — Official git reference
