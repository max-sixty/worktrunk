+++
title = "Worktrunk"
weight = 1
+++

Worktrunk is a CLI for git worktree management, designed for parallel AI agent
workflows. Worktrees give each branch its own directory, so agents work in
isolation. Navigate by branch name, see status at a glance, automate setup with
hooks.

Here's a quick demo:

<figure class="demo">
<img src="/assets/wt-demo.gif" alt="Worktrunk demo showing wt list, wt switch, and wt merge">
<figcaption>Listing worktrees, creating a worktree, working, merging back</figcaption>
</figure>

## Git worktrees are a great primitive

AI agents like Claude Code and Codex can handle longer tasks without supervision,
and running several in parallel is practical. Git worktrees give each agent its
own working directory — no stepping on each other's changes.

But the git worktree UX is clunky. Even starting a new worktree means typing the
branch name three times: `git worktree add -b feature ../repo.feature`, then
`cd ../repo.feature`.

## Worktrunk makes git worktrees easy

Worktrunk makes git worktrees easy to use — branch-based navigation, unified
status, and workflow automation. Start with the core commands; add workflow
automation as needed.

**Core commands:**

| Task                  | Worktrunk                        | Plain git                                                                     |
| --------------------- | -------------------------------- | ----------------------------------------------------------------------------- |
| Switch worktrees      | `wt switch feature`              | `cd ../repo.feature`                                                          |
| Create + start Claude | `wt switch -c -x claude feature` | `git worktree add -b feature ../repo.feature && cd ../repo.feature && claude` |
| Clean up              | `wt remove`                      | `cd ../repo && git worktree remove ../repo.feature && git branch -d feature`  |
| List with status      | `wt list`                        | `git worktree list` (paths only)                                              |

**Workflow automation:**

- **[Lifecycle hooks](@/hook.md)** — run commands on create, pre-merge, post-merge
- **[LLM commit messages](@/llm-commits.md)** — generate commit messages from diffs via [llm](https://llm.datasette.io/)
- **[Merge workflow](@/merge.md)** — squash, rebase, merge, clean up in one command
- ...and [lots more](#next-steps)

## In action

Create a worktree for a new task:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt switch --create fix-auth</span>
✅ <span class=g>Created new worktree for <b>fix-auth</b> from <b>main</b> at <b>../repo.fix-auth</b></span>
{% end %}

<!-- END AUTO-GENERATED -->

Switch to an existing worktree:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_switch_back.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt switch feature-api</span>
✅ <span class=g>Switched to worktree for <b>feature-api</b> at <b>../repo.feature-api</b></span>
{% end %}

<!-- END AUTO-GENERATED -->

See all worktrees at a glance:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__readme_example_list.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list</span>
<b>Branch</b> <b>Status</b> <b>HEAD±</b> <b>main↕</b> <b>Path</b> <b>Remote⇅</b> <b>Commit</b> <b>Age</b> <b>Message</b>
@ <b>feature-api</b> <span class=c>+</span> <span class=d>↕</span><span class=d>⇡</span> <span class=g>+54</span> <span class=r>-5</span> <span class=g>↑4</span> <span class=d><span class=r>↓1</span></span> <b>./repo.feature-api</b> <span class=g>⇡3</span> <span class=d>ec97decc</span> <span class=d>30m</span> <span class=d>Add API tests</span>
^ main <span class=d>^</span><span class=d>⇅</span> ./repo <span class=g>⇡1</span> <span class=d><span class=r>⇣1</span></span> <span class=d>6088adb3</span> <span class=d>4d</span> <span class=d>Merge fix-auth:…</span>

- fix-auth <span class=d>↕</span><span class=d>|</span> <span class=g>↑2</span> <span class=d><span class=r>↓1</span></span> ./repo.fix-auth <span class=d>|</span> <span class=d>127407de</span> <span class=d>5h</span> <span class=d>Add secure token…</span>

⚪ <span class=d>Showing 3 worktrees, 1 with changes, 2 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

Clean up when done:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_remove.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt remove</span>
🔄 <span class=c>Removing <b>feature-api</b> worktree &amp; branch in background (same commit as main)</span>
{% end %}

<!-- END AUTO-GENERATED -->

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

- Learn the core commands: [wt switch](@/switch.md), [wt list](@/list.md), [wt merge](@/merge.md), [wt remove](@/remove.md)
- Set up [project hooks](@/hook.md) for automated setup
- Explore [LLM commit messages](@/llm-commits.md), [fzf-like picker](@/select.md), [Claude Code integration](@/claude-code.md), [CI status & PR links](@/list.md#ci-status)
- Run `wt --help` or `wt <command> --help` for quick CLI reference

## Further reading

- [Claude Code: Best practices for agentic coding](https://www.anthropic.com/engineering/claude-code-best-practices) — Anthropic's official guide, including the worktree pattern
- [Shipping faster with Claude Code and Git Worktrees](https://incident.io/blog/shipping-faster-with-claude-code-and-git-worktrees) — incident.io's workflow for parallel agents
- [Git worktree pattern discussion](https://github.com/anthropics/claude-code/issues/1052) — Community discussion in the Claude Code repo
- [git-worktree documentation](https://git-scm.com/docs/git-worktree) — Official git reference
