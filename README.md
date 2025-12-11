<!-- markdownlint-disable MD033 -->

<h1><img src="docs/static/logo.png" alt="Worktrunk logo" width="50" align="absmiddle">&nbsp;&nbsp;Worktrunk</h1>

[![Docs](https://img.shields.io/badge/docs-worktrunk.dev-blue?style=for-the-badge&logo=gitbook)](https://worktrunk.dev)
[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![CI](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yaml?event=push&branch=main&style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

> **December 2025**: I've been using Worktrunk as my daily driver, and am releasing it as Open Source this week; I think folks will find it really helpful. It's built with love (there's no slop!). If social proof is helpful: I also created [PRQL](https://github.com/PRQL/prql) (10k stars) and am a maintainer of [Xarray](https://github.com/pydata/xarray) (4k stars), [Insta](https://github.com/mitsuhiko/insta), & [Numbagg](https://github.com/numbagg/numbagg).

Worktrunk is a CLI for git worktree management, designed for parallel AI agent workflows. Git worktrees give each agent an isolated branch and directory; Worktrunk adds branch-based navigation, unified status, and lifecycle hooks. Creating a new agent workspace is as immediate as `git switch`.

Here's a quick demo:

![Worktrunk Demo](https://cdn.jsdelivr.net/gh/max-sixty/worktrunk-assets@main/demos/wt-demo.gif)

> ## 📚 Full documentation at [worktrunk.dev](https://worktrunk.dev) 📚

<!-- ⚠️ AUTO-GENERATED from docs/content/worktrunk.md#git-worktrees-are-a-great-primitive..worktrunk-makes-git-worktrees-easy — edit source to update -->

## Git worktrees are a great primitive

For context, AI agents like Claude Code and Codex can increasingly handle longer tasks
without supervision, and it's very practical to run several in parallel. Git
worktrees provide each agent with its own working directory, avoiding agents
stepping on each other's changes.

But the git worktree UX is clunky. Even something as simple as "start a new worktree" is verbose:
`git worktree add -b feature ../repo.feature`, then `cd ../repo.feature`.

## Worktrunk makes git worktrees easy

Worktrunk makes git worktrees easy to use — branch-based navigation, unified status, and workflow automation.

**Core commands:**

| Task                  | Worktrunk                        | Plain git                                                                     |
| --------------------- | -------------------------------- | ----------------------------------------------------------------------------- |
| Switch worktrees      | `wt switch feature`              | `cd ../repo.feature`                                                          |
| Create + start Claude | `wt switch -c -x claude feature` | `git worktree add -b feature ../repo.feature && cd ../repo.feature && claude` |
| Clean up              | `wt remove`                      | `cd ../repo && git worktree remove ../repo.feature && git branch -d feature`  |
| List with status      | `wt list`                        | `git worktree list` (paths only)                                              |

**Workflow automation:**

- **[Lifecycle hooks](https://worktrunk.dev/hook/)** — run commands on create, pre-merge, post-merge
- **[LLM commit messages](https://worktrunk.dev/llm-commits/)** — generate commit messages from diffs via [llm](https://llm.datasette.io/)
- **[Merge workflow](https://worktrunk.dev/merge/)** — squash, rebase, merge, clean up in one command
- ...and [lots more](#next-steps)

<!-- END AUTO-GENERATED -->

## Core commands in practice

Create a worktree for a new task:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```

<!-- END AUTO-GENERATED -->

Switch to an existing worktree:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_switch_back.snap — edit source to update -->

```bash
$ wt switch feature-api
✅ Switched to worktree for feature-api at ../repo.feature-api
```

<!-- END AUTO-GENERATED -->

See all worktrees at a glance:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list.snap — edit source to update -->

```console
$ wt list
  Branch       Status        HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  ./repo.feature-api   ⇡3      ec97decc  30m   Add API tests
^ main             ^⇅                        ./repo               ⇡1  ⇣1  6088adb3  4d    Merge fix-auth:…
+ fix-auth         ↕|                ↑2  ↓1  ./repo.fix-auth        |     127407de  5h    Add secure token…

⚪ Showing 3 worktrees, 1 with changes, 2 ahead
```

<!-- END AUTO-GENERATED -->

Clean up when done:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_remove.snap — edit source to update -->

```bash
$ wt remove
🔄 Removing feature-api worktree & branch in background (already in main)
```

<!-- END AUTO-GENERATED -->

<!-- ⚠️ AUTO-GENERATED from docs/content/worktrunk.md#install..further-reading — edit source to update -->

## Install

**Homebrew (macOS & Linux):**

```bash
$ brew install max-sixty/worktrunk/wt
$ wt config shell install  # allows commands to change directories
```

**Cargo:**

```bash
$ cargo install worktrunk
$ wt config shell install
```

## Next steps

- Learn the core commands: [wt switch](https://worktrunk.dev/switch/), [wt list](https://worktrunk.dev/list/), [wt merge](https://worktrunk.dev/merge/), [wt remove](https://worktrunk.dev/remove/)
- Set up [project hooks](https://worktrunk.dev/hook/) for automated setup
- Explore [LLM commit messages](https://worktrunk.dev/llm-commits/), [fzf-like picker](https://worktrunk.dev/select/), [Claude Code integration](https://worktrunk.dev/claude-code/), [CI status & PR links](https://worktrunk.dev/list/#ci-status)
- Run `wt --help` or `wt <command> --help` for quick CLI reference

## Further reading

- [Claude Code: Best practices for agentic coding](https://www.anthropic.com/engineering/claude-code-best-practices) — Anthropic's official guide, including the worktree pattern
- [Shipping faster with Claude Code and Git Worktrees](https://incident.io/blog/shipping-faster-with-claude-code-and-git-worktrees) — incident.io's workflow for parallel agents
- [Git worktree pattern discussion](https://github.com/anthropics/claude-code/issues/1052) — Community discussion in the Claude Code repo
- [git-worktree documentation](https://git-scm.com/docs/git-worktree) — Official git reference

<!-- END AUTO-GENERATED -->

## Contributing

- ⭐ [Star the repo](https://github.com/max-sixty/worktrunk/stargazers)
- [Open an issue](https://github.com/max-sixty/worktrunk/issues/new?title=&body=%23%23%20Description%0A%0A%3C!--%20Describe%20the%20bug%20or%20feature%20request%20--%3E%0A%0A%23%23%20Context%0A%0A%3C!--%20Any%20relevant%20context%3A%20your%20workflow%2C%20what%20you%20were%20trying%20to%20do%2C%20etc.%20--%3E) — feedback, feature requests, or [a worktree friction we don't yet solve](https://github.com/max-sixty/worktrunk/issues/new?title=Worktree%20friction%3A%20&body=%23%23%20The%20friction%0A%0A%3C!--%20What%20worktree-related%20task%20is%20still%20painful%3F%20--%3E%0A%0A%23%23%20Current%20workaround%0A%0A%3C!--%20How%20do%20you%20handle%20this%20today%3F%20--%3E%0A%0A%23%23%20Ideal%20solution%0A%0A%3C!--%20What%20would%20make%20this%20easier%3F%20--%3E)
- Share: [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fworktrunk.dev) · [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fworktrunk.dev&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management) · [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fworktrunk.dev)

> ## 📚 Full documentation at [worktrunk.dev](https://worktrunk.dev) 📚
