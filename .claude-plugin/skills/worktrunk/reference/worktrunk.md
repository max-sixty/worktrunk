# Worktrunk

Worktrunk is a CLI for git worktree management, designed for running AI agents
in parallel.

Worktrunk's three core commands make worktrees as easy as branches.
Plus, Worktrunk has a bunch of quality-of-life features to simplify working
with many parallel changes, including hooks to automate local workflows.

Scaling agents becomes trivial. A quick demo:

## Context: git worktrees

AI agents like Claude Code and Codex can handle longer tasks without
supervision, such that it's possible to manage 5-10+ in parallel. Git's native
worktree feature give each agent its own working directory, so they don't step
on each other's changes.

But the git worktree UX is clunky. Even a task as small as starting a new
worktree requires typing the branch name three times: `git worktree add -b feat
../repo.feat`, then `cd ../repo.feat`.

## Worktrunk makes git worktrees as easy as branches

Worktrees are addressed by branch name; paths are computed from a configurable template.

> Start with the core commands

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
      <td>{% rawcode() %}wt switch feat{% end %}</td>
      <td>{% rawcode() %}cd ../repo.feat{% end %}</td>
    </tr>
    <tr>
      <td>Create + start Claude</td>
      <td>{% rawcode() %}wt switch -c -x claude feat{% end %}</td>
      <td>{% rawcode() %}git worktree add -b feat ../repo.feat && \
cd ../repo.feat && \
claude{% end %}</td>
    </tr>
    <tr>
      <td>Clean up</td>
      <td>{% rawcode() %}wt remove{% end %}</td>
      <td>{% rawcode() %}cd ../repo && \
git worktree remove ../repo.feat && \
git branch -d feat{% end %}</td>
    </tr>
    <tr>
      <td>List with status</td>
      <td>{% rawcode() %}wt list{% end %}</td>
      <td>{% rawcode() %}git worktree list{% end %} (paths only)</td>
    </tr>
  </tbody>
</table>

**Workflow automation:**

> Expand into the more advanced commands as needed

- **[Hooks](https://worktrunk.dev/hook/)** — run commands on create, pre-merge, post-merge, etc
- **[LLM commit messages](https://worktrunk.dev/llm-commits/)** — generate commit messages from diffs via [llm](https://llm.datasette.io/)
- **[Merge workflow](https://worktrunk.dev/merge/)** — squash, rebase, merge, clean up in one command
- ...and **[lots more](#next-steps)**

A demo with some advanced features:

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
<summary><strong>Windows</strong></summary>

On Windows, `wt` defaults to Windows Terminal's command. Winget additionally installs Worktrunk as `git-wt` to avoid the conflict:

```bash
winget install max-sixty.worktrunk
git-wt config shell install
```

Alternatively, disable Windows Terminal's alias (Settings → Privacy & security → For developers → App Execution Aliases → disable "Windows Terminal") to use `wt` directly.

</details>

**Arch Linux:**

```bash
paru worktrunk-bin && wt config shell install
```

## Quick start

Create a worktree for a new feature:

<span class="cmd">wt switch --create feature-auth</span>
<span class=g>✓</span> <span class=g>Created branch <b>feature-auth</b> from <b>main</b> and worktree @ <b>repo.feature-auth</b></span>

This creates a new branch and worktree, then switches to it. Do your work, then check all worktrees with [`wt list`](https://worktrunk.dev/list/):

<span class="cmd">wt list</span>
  <b>Branch</b>        <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ feature-auth  <span class=c>+</span>   <span class=d>–</span>      <span class=g>+53</span>                         <span class=d>0e631add</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
^ main              <span class=d>^</span><span class=d>⇡</span>                         <span class=g>⇡1</span>      <span class=d>0e631add</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 2 worktrees, 1 with changes, 1 column hidden</span>

The `@` marks the current worktree. `+` means uncommitted changes, `↕` means unpushed commits.

When done, either:

**PR workflow** — commit, push, open a PR, merge via GitHub/GitLab, then clean up:

```bash
wt step commit                    # commit staged changes
gh pr create                      # or glab mr create
wt remove                         # after PR is merged
```

**Local merge** — squash, rebase onto main, fast-forward merge, clean up:

<span class="cmd">wt merge main</span>
<span class=c>◎</span> <span class=c>Generating commit message and committing changes... <span style='color:var(--bright-black,#555)'>(2 files, <span class=g>+53</span></span></span>, no squashing needed<span style='color:var(--bright-black,#555)'>)</span>
<span style='background:var(--bright-white,#fff)'> </span> <b>Add authentication module</b>
<span class=g>✓</span> <span class=g>Committed changes @ <span class=d>a1b2c3d</span></span>
<span class=c>◎</span> <span class=c>Merging 1 commit to <b>main</b> @ <span class=d>a1b2c3d</span> (no rebase needed)</span>
<span style='background:var(--bright-white,#fff)'> </span> * <span style='color:var(--yellow,#a60)'>a1b2c3d</span> Add authentication module
<span style='background:var(--bright-white,#fff)'> </span>  auth.rs | 51 <span class=g>+++++++++++++++++++++++++++++++++++++++++++++++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>  lib.rs  |  2 <span class=g>++</span>
<span style='background:var(--bright-white,#fff)'> </span>  2 files changed, 53 insertions(+)
<span class=g>✓</span> <span class=g>Merged to <b>main</b> <span style='color:var(--bright-black,#555)'>(1 commit, 2 files, +53</span></span><span style='color:var(--bright-black,#555)'>)</span>
<span class=c>◎ Removing <b>feature-auth</b> worktree &amp; branch in background (same commit as <b>main</b>,</span> <span class=d>_</span><span class=c>)</span>
<span class=d>○</span> Switched to worktree for <b>main</b> @ <b>repo</b>

For parallel agents, create multiple worktrees and launch an agent in each:

```bash
wt switch -x claude -c feature-a -- 'Add user authentication'
wt switch -x claude -c feature-b -- 'Fix the pagination bug'
wt switch -x claude -c feature-c -- 'Write tests for the API'
```

The `-x` flag runs a command after switching; arguments after `--` are passed to it. Configure [post-start hooks](https://worktrunk.dev/hook/) to automate setup (install deps, start dev servers).

## Next steps

- Learn the core commands: [`wt switch`](https://worktrunk.dev/switch/), [`wt list`](https://worktrunk.dev/list/), [`wt merge`](https://worktrunk.dev/merge/), [`wt remove`](https://worktrunk.dev/remove/)
- Set up [project hooks](https://worktrunk.dev/hook/) for automated setup
- Explore [LLM commit messages](https://worktrunk.dev/llm-commits/), [fzf-like
  selector](https://worktrunk.dev/select/), [Claude Code integration](https://worktrunk.dev/claude-code/), [CI
  status & PR links](https://worktrunk.dev/list/#ci-status)
- Run `wt --help` or `wt <command> --help` for quick CLI reference

## Further reading

- [Claude Code: Best practices for agentic coding](https://www.anthropic.com/engineering/claude-code-best-practices) — Anthropic's official guide, including the worktree pattern
- [Shipping faster with Claude Code and Git Worktrees](https://incident.io/blog/shipping-faster-with-claude-code-and-git-worktrees) — incident.io's workflow for parallel agents
- [Git worktree pattern discussion](https://github.com/anthropics/claude-code/issues/1052) — Community discussion in the Claude Code repo
- [git-worktree documentation](https://git-scm.com/docs/git-worktree) — Official git reference
