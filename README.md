<!-- markdownlint-disable MD033 -->

<h1><img src="docs/static/logo.png" alt="Worktrunk logo" width="50" align="absmiddle">&nbsp;&nbsp;Worktrunk</h1>

[![Docs](https://img.shields.io/badge/docs-worktrunk.dev-blue?style=for-the-badge&logo=gitbook)](https://worktrunk.dev)
[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![CI](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yaml?event=push&branch=main&style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci)
[![Codecov](https://img.shields.io/codecov/c/github/max-sixty/worktrunk?style=for-the-badge&logo=codecov)](https://codecov.io/gh/max-sixty/worktrunk)
[![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers)

> **January 2026**: Worktrunk was [released](https://x.com/max_sixty/status/2006077845391724739?s=20) over the holidays, and lots of folks seem to be using it. It's built with love (there's no slop!). If social proof is helpful: I also created [PRQL](https://github.com/PRQL/prql) (10k stars) and am a maintainer of [Xarray](https://github.com/pydata/xarray) (4k stars), [Insta](https://github.com/mitsuhiko/insta), & [Numbagg](https://github.com/numbagg/numbagg). Please let me know any frictions at all; I'm intensely focused on making Worktrunk excellent, and the biggest gap is understanding how others experience using it.

Worktrunk is a CLI for git worktree management, designed for running AI agents in parallel.

Worktrunk's three core commands make worktrees as easy as branches. Plus, Worktrunk has a bunch of quality-of-life features to simplify working with many parallel changes, including hooks to automate local workflows.

Scaling agents becomes trivial. A quick demo:

![Worktrunk Demo](https://cdn.jsdelivr.net/gh/max-sixty/worktrunk-assets@main/assets/docs/light/wt-core.gif)

> ### 📚 Full documentation at [worktrunk.dev](https://worktrunk.dev) 📚

<!-- ⚠️ AUTO-GENERATED from docs/content/worktrunk.md#context-git-worktrees..worktrunk-makes-git-worktrees-as-easy-as-branches — edit source to update -->

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
      <td><pre>wt switch feat</pre></td>
      <td><pre>cd ../repo.feat</pre></td>
    </tr>
    <tr>
      <td>Create + start Claude</td>
      <td><pre>wt switch -c -x claude feat</pre></td>
      <td><pre>git worktree add -b feat ../repo.feat && \
cd ../repo.feat && \
claude</pre></td>
    </tr>
    <tr>
      <td>Clean up</td>
      <td><pre>wt remove</pre></td>
      <td><pre>cd ../repo && \
git worktree remove ../repo.feat && \
git branch -d feat</pre></td>
    </tr>
    <tr>
      <td>List with status</td>
      <td><pre>wt list</pre></td>
      <td><pre>git worktree list</pre> (paths only)</td>
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

![Worktrunk omnibus demo: multiple Claude agents in Zellij tabs with hooks, LLM commits, and merge workflow](https://raw.githubusercontent.com/max-sixty/worktrunk-assets/main/assets/docs/light/wt-zellij-omnibus.gif)

<!-- END AUTO-GENERATED -->

<!-- ⚠️ AUTO-GENERATED from docs/content/worktrunk.md#install..further-reading — edit source to update -->

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

<!-- END AUTO-GENERATED -->

## Contributing

- ⭐ Star the repo
- Tell a friend about Worktrunk
- [Open an issue](https://github.com/max-sixty/worktrunk/issues/new?title=&body=%23%23%20Description%0A%0A%3C!--%20Describe%20the%20bug%20or%20feature%20request%20--%3E%0A%0A%23%23%20Context%0A%0A%3C!--%20Any%20relevant%20context%3A%20your%20workflow%2C%20what%20you%20were%20trying%20to%20do%2C%20etc.%20--%3E) — feedback, feature requests, even a small friction or imperfect user message, or [a worktree pain not yet solved](https://github.com/max-sixty/worktrunk/issues/new?title=Worktree%20friction%3A%20&body=%23%23%20The%20friction%0A%0A%3C!--%20What%20worktree-related%20task%20is%20still%20painful%3F%20--%3E%0A%0A%23%23%20Current%20workaround%0A%0A%3C!--%20How%20do%20you%20handle%20this%20today%3F%20--%3E%0A%0A%23%23%20Ideal%20solution%0A%0A%3C!--%20What%20would%20make%20this%20easier%3F%20--%3E)
- Share: [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fworktrunk.dev) · [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fworktrunk.dev&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management) · [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fworktrunk.dev)

> ### 📚 Full documentation at [worktrunk.dev](https://worktrunk.dev) 📚

### Star history

<a href="https://star-history.com/#max-sixty/worktrunk&Date">
  <img src="https://api.star-history.com/svg?repos=max-sixty/worktrunk&type=Date" width="500" alt="Star History Chart">
</a>
