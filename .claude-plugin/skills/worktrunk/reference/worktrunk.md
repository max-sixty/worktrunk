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
brew install max-sixty/worktrunk/wt && wt config shell install
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
