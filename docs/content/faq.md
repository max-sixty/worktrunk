+++
title = "FAQ"
weight = 8
+++

## What commands does Worktrunk execute?

Worktrunk executes commands in three contexts:

1. **Project hooks** (`.config/wt.toml`) — Automation for worktree lifecycle
2. **LLM commands** (`~/.config/worktrunk/config.toml`) — Commit message generation
3. **--execute flag** — Commands you provide explicitly

Commands from project hooks and LLM configuration require approval on first run. Approved commands are saved to user config. If a command changes, Worktrunk requires new approval.

### Example approval prompt

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_approval_prompt.snap — edit source to update -->

```txt
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

⚪ post-create build:
   echo 'Building project...'

⚪ post-create test:
   echo 'Running tests...'

💡 Allow and remember? [y/N]
```

<!-- END AUTO-GENERATED -->

Use `--force` to bypass prompts (useful for CI/automation).

## How does Worktrunk compare to alternatives?

### vs. Branch Switching

Branch switching uses one directory, so only one agent can work at a time. Worktrees give each agent its own directory.

### vs. Plain `git worktree`

Git's built-in worktree commands work but require manual lifecycle management:

```bash
# Plain git worktree workflow
git worktree add -b feature-branch ../myapp-feature main
cd ../myapp-feature
# ...work, commit, push...
cd ../myapp
git merge feature-branch
git worktree remove ../myapp-feature
git branch -d feature-branch
```

Worktrunk automates the full lifecycle:

```bash
wt switch --create feature-branch  # Creates worktree, runs setup hooks
# ...work...
wt merge                            # Squashes, merges, removes worktree
```

What `git worktree` doesn't provide:

- Consistent directory naming and cleanup validation
- Project-specific automation (install dependencies, start services)
- Unified status across all worktrees (commits, CI, conflicts, changes)

### vs. git-machete / git-town

Different scopes:

- **git-machete**: Branch stack management in a single directory
- **git-town**: Git workflow automation in a single directory
- **worktrunk**: Multi-worktree management with hooks and status aggregation

These tools can be used together—run git-machete or git-town inside individual worktrees.

### vs. Git TUIs (lazygit, gh-dash, etc.)

Git TUIs operate on a single repository. Worktrunk manages multiple worktrees, runs automation hooks, and aggregates status across branches. TUIs work inside each worktree directory.

## Installation fails with C compilation errors

Errors related to tree-sitter or C compilation (C99 mode, `le16toh` undefined) can be avoided by installing without syntax highlighting:

```bash
$ cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.

## How can I contribute?

- Star the repo
- Try it out and [open an issue](https://github.com/max-sixty/worktrunk/issues) with feedback
- Send to a friend
- Post about it on [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk), [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management), or [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk)

## Running tests (for contributors)

### Quick tests

```bash
$ cargo test --lib --bins           # Unit tests (~200 tests)
$ cargo test --test integration     # Integration tests (~300 tests)
```

### Full integration tests

Requires bash, zsh, and fish:

```bash
$ cargo test --test integration --features shell-integration-tests
```

### Releases

Use [cargo-release](https://github.com/crate-ci/cargo-release):

```bash
$ cargo release patch --execute   # 0.1.0 -> 0.1.1
$ cargo release minor --execute   # 0.1.0 -> 0.2.0
```
