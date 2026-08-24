# Twitter Thread: Worktrunk Launch

Published: Dec 30, 2025
URL: https://x.com/max_sixty/status/2006077845391724739

---

**1/**
Announcing Worktrunk! A git worktree manager, designed for running AI agents in parallel.

A few points on why I'm so excited about the project, and why I hope it becomes broadly adopted 🧵

[wt-core.gif]

**2/**
As models have improved this year, I've been running more & more Claude Code instances in parallel, often 5-10.

Each needs its own isolated working directory, otherwise they get confused by each other's changes.

**3/**
Git worktrees solve this, but the UX is terrible!

To create & navigate to a new worktree in git:

git worktree add -b feat ../repo.feat && cd ../repo.feat

...even for a simple command, we need to type the name three times.

**4/**
Worktrunk is a CLI, written in Rust, which makes git worktrees as easy as branches.

[worktrunk.dev card]

**5/**
In contrast to the git command, the Worktrunk command to create a new worktree is short (& aliasable):

wt switch --create api

[wt-switch.gif]

**6/**
Worktrunk's other core commands:

wt list: see all worktrees with status
wt remove: delete a worktree

[wt-list-remove.gif]

**7/**
Beyond core commands, Worktrunk has quality-of-life features to simplify working with many parallel changes:

Hooks: Post-start hooks run after creating a worktree: install deps, copy caches, start dev servers, etc. And there's a hook for every stage of a worktree lifecycle.

[wt-hooks.gif]

**8/**
wt list renders in ~50ms, then fills in details (CI status, diff stats) as they become available. Can also list branches with wt list --branches.

wt list --full: CI status as clickable dots. Green/blue/red. Clicking opens the PR.

[wt-list.gif]

**9/**
wt switch picker: fuzzy picker across all branches.

[wt-select-short.gif]

**10/**
LLM Commits: When running wt step commit or wt merge, worktrunk can have an LLM write the commit message, with a customizable template.

[wt-commit.gif]

**11/**
wt merge: squash, rebase, merge, remove worktree, delete branch, in one command.

[wt-merge.gif]

**12/**
.@claudeai status line integration. See branch, diff stats, CI status at a glance.

[wt-statusline.gif]

**13/**
Putting it all together: parallel Claude Code agents in Zellij tabs, each in its own worktree. The full lifecycle: wt switch, wt list, wt select, wt merge.

[wt-zellij-omnibus.gif]

**14/**
To install:

brew install max-sixty/worktrunk/wt
wt config shell install

Feedback very welcome. Open an issue or reply here.

⭐

[GitHub card: github.com/max-sixty/worktrunk]

**15/**
Big thanks to @AnthropicAI and the @claudeai team, including @bcherny @_catwu @alexalbert__, for building Claude Code. Worktrunk wouldn't exist without it 🙏

If this was useful, staring the repo / liking / RT-ing the first tweet helps spread the word.

[Quote tweet of tweet 1]
