+++
title = "Concepts"
weight = 2
+++

## Why git worktrees?

When working with multiple AI agents (or multiple tasks), you have a few options:

| Approach | Pros | Cons |
|----------|------|------|
| **One working tree, many branches** | Simple setup | Agents step on each other, can't use git for staging/committing |
| **Multiple clones** | Full isolation | Slow to set up, drift out of sync |
| **Git worktrees** | Isolation + shared history | Requires management |

Git worktrees give you multiple directories backed by a single `.git` directory. Each worktree has its own branch and working tree, but shares the repository history and refs.

## Why Worktrunk?

Git's built-in `worktree` commands require remembering worktree locations and composing git + `cd` commands. Worktrunk bundles creation, navigation, status, and cleanup into simple commands.

### Comparison

| Task | Worktrunk | Plain git |
|------|-----------|-----------|
| Switch worktrees | `wt switch feature` | `cd ../repo.feature` |
| Create + start Claude | `wt switch -c -x claude feature` | `git worktree add -b feature ../repo.feature main && cd ../repo.feature && claude` |
| Clean up | `wt remove` | `cd ../repo && git worktree remove ../repo.feature && git branch -d feature` |
| List | `wt list` (with diffstats & status) | `git worktree list` (just names & paths) |
| List with CI status | `wt list --full` | N/A |

### What Worktrunk adds

- **Branch-based navigation**: Address worktrees by branch name, not path
- **Consistent directory naming**: Predictable locations for all worktrees
- **Lifecycle hooks**: Run commands on create, start, pre-merge, post-merge
- **Unified status**: See changes, commits, CI status across all worktrees
- **Safe cleanup**: Validates changes are merged before deleting branches

## Worktree addressing

Worktrunk uses **path-first lookup** when resolving arguments:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, use it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

This means `wt switch foo` will switch to `repo.foo/` even if that worktree is on a different branch.
