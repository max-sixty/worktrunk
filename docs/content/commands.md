+++
title = "Commands Reference"
weight = 4
+++

## wt switch

Switch to an existing worktree or create a new one.

```bash
$ wt switch [OPTIONS] <BRANCH>
```

### Arguments

- `<BRANCH>` — Branch, path, `@` (HEAD), `-` (previous), or `^` (main)

### Options

- `-c, --create` — Create a new branch
- `-b, --base <BASE>` — Base branch (defaults to default branch)
- `-x, --execute <EXECUTE>` — Command to run after switch
- `-f, --force` — Skip approval prompts
- `--no-verify` — Skip all project hooks

### Examples

```bash
# Switch to existing worktree
wt switch feature-branch

# Create new worktree from main
wt switch --create new-feature

# Switch to previous worktree
wt switch -

# Create from specific base
wt switch --create hotfix --base production

# Create and run command
wt switch --create docs --execute "code ."
```

### Hooks

- **post-create** (sequential, blocking) — Runs after creation, before success message
- **post-start** (parallel, background) — Spawned after success, logs to `.git/wt-logs/`

---

## wt merge

Merge worktree into target branch with full workflow automation.

```bash
$ wt merge [OPTIONS] [TARGET]
```

### Arguments

- `[TARGET]` — Target branch (defaults to default branch)

### Options

- `--no-squash` — Skip commit squashing
- `--no-commit` — Skip commit, squash, and rebase
- `--no-remove` — Keep worktree after merge
- `--no-verify` — Skip all project hooks
- `-f, --force` — Skip approval prompts
- `--stage <STAGE>` — What to stage: `all` (default), `tracked`, `none`

### Operation

1. **Commit** — Stage and commit with LLM message
2. **Squash** — Combine commits into one
3. **Rebase** — Rebase onto target
4. **Pre-merge hooks** — Run tests/lints
5. **Push** — Fast-forward to target
6. **Cleanup** — Remove worktree and branch
7. **Post-merge hooks** — Final automation

### Examples

```bash
# Full merge to main
wt merge

# Keep worktree after merging
wt merge --no-remove

# Skip hooks
wt merge --no-verify

# Merge without squashing
wt merge --no-squash
```

---

## wt remove

Remove worktree and optionally delete branch.

```bash
$ wt remove [OPTIONS] [WORKTREES]...
```

### Arguments

- `[WORKTREES]...` — Worktree or branch (`@` for current)

### Options

- `--no-delete-branch` — Keep branch after removal
- `-D, --force-delete` — Delete unmerged branches
- `--no-background` — Run removal in foreground

### Branch deletion

By default, branches are deleted only when their content is already in the target branch:

- No changes beyond the common ancestor
- Same content as target (handles squash/rebase merges)

Use `-D` to force delete unmerged branches.

### Examples

```bash
# Remove current worktree
wt remove

# Remove specific worktree
wt remove feature-branch

# Keep the branch
wt remove --no-delete-branch feature-branch

# Remove multiple
wt remove old-feature another-branch
```

---

## wt list

Show all worktrees and optionally branches.

```bash
$ wt list [OPTIONS]
```

### Options

- `--format <FORMAT>` — Output format: `table` (default), `json`
- `--branches` — Include branches without worktrees
- `--remotes` — Include remote branches
- `--full` — Show CI, conflicts, diffs
- `--progressive` — Show fast info first, update with slow info

### Columns

| Column | Description |
|--------|-------------|
| Branch | Branch name |
| Status | Quick status symbols |
| HEAD± | Uncommitted changes vs HEAD |
| main↕ | Commits ahead/behind main |
| main…± | Line diffs ahead of main (with `--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind remote |
| CI | Pipeline status (with `--full`) |
| Commit | Short hash |
| Age | Time since last commit |
| Message | Last commit message |

### Status Symbols

- `+` Staged files
- `!` Modified files
- `?` Untracked files
- `✖` Merge conflicts
- `⊘` Would conflict with main
- `≡` Matches main content
- `_` No commits
- `↑↓↕` Ahead/behind/diverged from main
- `⇡⇣⇅` Ahead/behind/diverged from remote

### JSON Output

```bash
# Get current worktree
wt list --format=json | jq '.[] | select(.is_current)'

# Find worktrees with conflicts
wt list --format=json | jq '.[] | select(.status.branch_state == "Conflicts")'

# Find branches ahead of main
wt list --format=json | jq '.[] | select(.status.main_divergence == "Ahead")'
```

---

## wt config

Manage configuration and shell integration.

```bash
$ wt config <COMMAND>
```

### Subcommands

- `shell install` — Install shell integration
- `shell init <SHELL>` — Output shell init script
- `create` — Create user config file with examples
- `show` — Show config file locations
- `cache` — Manage caches
- `status` — Manage branch status markers
- `approvals` — Manage command approvals

---

## wt step

Building blocks for custom workflows.

```bash
$ wt step <COMMAND>
```

### Subcommands

- `commit` — Commit changes with LLM message
- `squash` — Squash commits with LLM message
- `push` — Push to local target branch
- `rebase` — Rebase onto target
- `post-create` — Run post-create hook
- `post-start` — Run post-start hook
- `pre-commit` — Run pre-commit hook
- `pre-merge` — Run pre-merge hook
- `post-merge` — Run post-merge hook

---

## wt select

Interactive worktree picker with diff preview. Unix only.

Preview tabs (toggle with `1`/`2`/`3`):

1. Working tree changes (uncommitted)
2. Commit history (commits not on main highlighted)
3. Branch diff (changes ahead of main)
