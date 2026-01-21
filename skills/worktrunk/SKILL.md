---
name: worktrunk
description: Manage git worktrees for parallel development workflows. Use when working with git worktrees, feature branch workflows, parallel AI agent tasks, or when the user mentions wt, worktrees, branch switching, or merging feature branches.
allowed-tools: Bash(wt:*)
---

# Worktrunk (wt)

## Quick start

```bash
wt switch --create feature    # Create worktree and branch
wt switch feature             # Switch to worktree
wt list                       # Show all worktrees
wt merge                      # Squash, rebase, merge to main, remove worktree
wt remove                     # Remove worktree; delete branch if merged
wt config shell install       # Install shell integration (required for cd)
```

## Core concepts

**Worktrees** are separate working directories per branch (unlike `git switch` which changes branches in place).
**Branches** are addressed by name; paths are computed from templates.
**Default branch** is the merge target (main, master, etc.).

## Switch

```bash
wt switch <branch>                        # Create worktree if needed, cd to it
wt switch --create feature                # Create new branch from default
wt switch --create fix --base production  # Create from specific base
wt switch -                               # Previous worktree (like cd -)
wt switch ^                               # Default branch worktree
wt switch pr:123                          # GitHub PR #123 (requires gh)
wt switch mr:456                          # GitLab MR !456 (requires glab)
```

**Flags:**
```bash
--base <branch>               # Base branch for creation
--execute <cmd>               # Run command after switch (replaces wt process)
--yes                         # Skip approval prompts
--clobber                     # Remove stale paths at target
--no-verify                   # Skip hooks
```

**Execute flag** launches editors/agents (supports template variables):
```bash
wt switch --create feature --execute claude
wt switch --create fix --execute "code {{ worktree_path }}"
wt switch --create feature --execute "tmux new -s {{ branch | sanitize }}"
```

## List

```bash
wt list                       # Show all worktrees
wt list --full                # Include CI status and line diffs
wt list --branches            # Include branches without worktrees
wt list --format=json         # JSON output for scripts
```

**Columns:** Branch, Status (symbols), HEAD± (uncommitted), main↕ (ahead/behind default), Remote⇅, Path, CI, Commit, Age, Message

**Status symbols:**
```
+ staged  ! modified  ? untracked  ✘ conflicts  ⤴ rebase  ⤵ merge
_ same commit (safe)  ⊂ integrated (safe)  ↕ diverged  ↑ ahead  ↓ behind
```

**JSON queries:**
```bash
wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'
wt list --format=json | jq '.[] | select(.working_tree.modified)'
wt list --format=json | jq '.[] | select(.kind == "branch") | .branch'
```

**Fields:** `branch`, `path`, `kind`, `commit`, `working_tree`, `main_state`, `main`, `remote`, `ci`, `statusline`, `is_current`, `is_previous`, etc.

## Merge

`wt merge [target]` merges current branch into target (default: default branch).

**Pipeline:** squash → rebase (if behind) → pre-merge hooks → fast-forward merge → pre-remove hooks → remove worktree/branch → post-merge hooks

```bash
wt merge                      # Merge to default branch
wt merge develop              # Merge to specific branch
wt merge --no-squash          # Preserve commit history
wt merge --no-commit          # Skip committing (rebase still runs)
wt merge --no-rebase          # Skip rebase
wt merge --no-remove          # Keep worktree after merge
wt merge --no-verify          # Skip hooks
wt merge --stage tracked      # Only stage tracked files (default: all)
```

## Remove

```bash
wt remove                     # Remove current worktree
wt remove feature old-fix     # Remove specific worktrees
wt remove --force feature     # Remove worktree with untracked files
wt remove -D feature          # Delete unmerged branch
wt remove --no-delete-branch  # Keep branch
wt remove --foreground        # Run removal in foreground (default: background)
```

**Branch cleanup** (auto-deletes when merging adds nothing): same commit → ancestor → no added changes → trees match → merge adds nothing

## Step

```bash
wt step commit                # Stage and commit with LLM-generated message
wt step squash                # Squash all commits into one with LLM message
wt step rebase                # Rebase onto target branch
wt step push                  # Fast-forward target to current branch
wt step copy-ignored          # Copy gitignored files between worktrees
```

## Hooks

Hooks run at lifecycle points. Define in `.config/wt.toml` (project) or `~/.config/worktrunk/config.toml` (user).

**Types:**
```
post-start      # After worktree created (background, parallel)
post-create     # After worktree created (blocking)
post-switch     # After every switch (background)
pre-commit      # Before commit during merge
pre-merge       # Before merging to target
post-merge      # After successful merge
pre-remove      # Before worktree removed
post-remove     # After worktree removed (background)
```

```toml
[post-create]
install = "npm ci"
env = "echo 'PORT={{ branch | hash_port }}' > .env.local"

[pre-merge]
test = "npm test"
lint = "npm run lint"

[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[post-remove]
kill-server = "lsof -ti :{{ branch | hash_port }} | xargs kill 2>/dev/null || true"
```

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree_path }}`, `{{ commit }}`, `{{ remote }}`, `{{ target }}`, etc.

**Filters:** `sanitize` (filesystem-safe), `hash_port` (port 10000-19999), `sanitize_db` (database-safe).

**Run manually:**
```bash
wt hook pre-merge              # Run all pre-merge hooks
wt hook pre-merge test         # Run hooks named "test"
wt hook pre-merge user:        # Run user hooks only
wt hook pre-merge project:test # Run project's "test" hook
wt hook pre-merge --yes        # Skip approval prompts
```

## Config

```bash
wt config shell install        # Install shell integration (required for cd)
wt config create               # Create user config
wt config create --project     # Create project config (.config/wt.toml)
wt config show                 # Show current config and file locations
wt config state                # Manage saved state
wt config hook approvals       # Manage hook approvals
```

## Select

`wt select` — interactive worktree picker with live preview (Unix only).

**Keybindings:** `↑`/`↓` navigate, `Enter` switch, `1`/`2`/`3`/`4` preview tabs, `Esc` cancel

## Shell integration

Required for `wt switch` to change directories. Install with `wt config shell install`.

**Manual install:**
```bash
# bash/zsh: add to ~/.bashrc or ~/.zshrc
eval "$(wt config shell init bash)"

# fish: add to ~/.config/fish/config.fish
wt config shell init fish | source
```

## Worktree paths

Configure in user config (`~/.config/worktrunk/config.toml`):

```toml
# Default: siblings in parent (creates ~/code/myproject.feature-auth)
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

# Inside repository (creates ~/code/myproject/.worktrees/feature-auth)
worktree-path = ".worktrees/{{ branch | sanitize }}"

# Namespaced (creates ~/code/worktrees/myproject/feature-auth)
worktree-path = "../worktrees/{{ repo }}/{{ branch | sanitize }}"
```

## Examples

**Feature branch workflow:**
```bash
wt switch --create feature
# ... work ...
wt merge
```

**Parallel agent tasks:**
```bash
wt switch --create task1 --execute claude
wt switch --create task2 --execute claude
wt list
```

**Dev servers per worktree:**
```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[post-remove]
kill = "lsof -ti :{{ branch | hash_port }} | xargs kill 2>/dev/null || true"
```

**Local CI (fast validation before merge):**
```toml
[pre-commit]
format = "cargo fmt -- --check"
lint = "cargo clippy"

[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

## Files

- **User config:** `~/.config/worktrunk/config.toml`
- **Project config:** `.config/wt.toml`
- **Logs:** `.git/wt-logs/`
