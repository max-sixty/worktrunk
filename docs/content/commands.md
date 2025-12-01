+++
title = "Commands Reference"
weight = 4
+++

## wt switch

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-md` — edit source to update -->

```text
wt switch — Switch to a worktree
Usage: wt switch [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>
          Branch or worktree name

          Shortcuts: '^' (main), '-' (previous), '@' (current)

Options:
  -c, --create
          Create a new branch

  -b, --base <BASE>
          Base branch

          Defaults to default branch.

  -x, --execute <EXECUTE>
          Command to run after switch

  -f, --force
          Skip approval prompts

      --no-verify
          Skip all project hooks

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

### Worktree resolution

Arguments are resolved using **path-first lookup**:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, switch to it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

**Example**: If `repo.foo/` exists but is on branch `bar`:

- `wt switch foo` switches to `repo.foo/` (the `bar` branch worktree)
- `wt switch bar` also works (falls back to branch lookup)

### Switching to Existing Worktree

- If worktree exists at expected path or for branch, changes directory via shell integration
- No hooks run
- No branch creation

### Creating New Worktree (`--create`)

1. Creates new branch (defaults to current default branch as base)
2. Creates worktree in configured location (default: `../{{ main_worktree }}.{{ branch }}`)
3. Runs post-create hooks sequentially (blocking)
4. Shows success message
5. Spawns post-start hooks in background (non-blocking)
6. Changes directory to new worktree via shell integration

## Hooks

### post-create (sequential, blocking)

- Run after worktree creation, before success message
- Typically: `npm install`, `cargo build`, setup tasks
- Failures block the operation
- Skip with `--no-verify`

### post-start (parallel, background)

- Spawned after success message shown
- Typically: dev servers, file watchers, editors
- Run in background, failures logged but don't block
- Logs: `.git/wt-logs/{branch}-post-start-{name}.log`
- Skip with `--no-verify`

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Switch to existing worktree:

```console
wt switch feature-branch
```

Create new worktree from main:

```console
wt switch --create new-feature
```

Switch to previous worktree:

```console
wt switch -
```

Create from specific base:

```console
wt switch --create hotfix --base production
```

Create and run command:

```console
wt switch --create docs --execute "code ."
```

Skip hooks during creation:

```console
wt switch --create temp --no-verify
```

## Shortcuts

Use `@` for current HEAD, `-` for previous, `^` for main:

```console
wt switch @                              # Switch to current branch's worktree
wt switch -                              # Switch to previous worktree
wt switch --create new-feature --base=^  # Branch from main (default)
wt switch --create bugfix --base=@       # Branch from current HEAD
wt remove @                              # Remove current worktree
```

<!-- END AUTO-GENERATED -->

---

## wt merge

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-md` — edit source to update -->

```text
wt merge — Merge worktree into target branch
Usage: wt merge [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --no-squash
          Skip commit squashing

      --no-commit
          Skip commit, squash, and rebase

      --no-remove
          Keep worktree after merge

      --no-verify
          Skip all project hooks

  -f, --force
          Skip approval prompts

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

Commit → Squash → Rebase → Pre-merge hooks → Push → Cleanup → Post-merge hooks

### Commit

Uncommitted changes are staged and committed with LLM commit message.
Use `--stage=tracked` to stage only tracked files, or `--stage=none` to commit only what's already staged.

### Squash

Multiple commits are squashed into one (like GitHub's "Squash and merge") with LLM commit message.
Skip with `--no-squash`. Safety backup: `git reflog show refs/wt-backup/<branch>`

### Rebase

Branch is rebased onto target. Conflicts abort the merge immediately.

### Hooks

Pre-merge commands run after rebase (failures abort). Post-merge commands
run after cleanup (failures logged). Skip all with `--no-verify`.

### Push

Fast-forward push to local target branch. Non-fast-forward pushes are rejected.

### Cleanup

Worktree and branch are removed. Skip with `--no-remove`.

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Basic merge to main:

```console
wt merge
```

Merge without squashing:

```console
wt merge --no-squash
```

Keep worktree after merging:

```console
wt merge --no-remove
```

Skip all hooks:

```console
wt merge --no-verify
```

<!-- END AUTO-GENERATED -->

---

## wt remove

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-md` — edit source to update -->

```text
wt remove — Remove worktree and branch
Usage: wt remove [OPTIONS] [WORKTREES]...

Arguments:
  [WORKTREES]...
          Worktree or branch (@ for current)

Options:
      --no-delete-branch
          Keep branch after removal

  -D, --force-delete
          Delete unmerged branches

      --no-background
          Run removal in foreground

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

Removes worktree directory, git metadata, and branch. Requires clean working tree.

### No arguments (remove current)

- Removes current worktree and switches to main worktree
- In main worktree: switches to default branch

### By name (remove specific)

- Removes specified worktree(s) and branches
- Current worktree removed last (switches to main first)

### Worktree resolution

Arguments are resolved to worktrees using **path-first lookup**:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, use it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

**Example**: If `repo.foo/` exists but is on branch `bar`:

- `wt remove foo` removes `repo.foo/` and the `bar` branch
- `wt remove bar` also works (falls back to branch lookup)

**Conflict detection**: If path `repo.foo/` has a worktree on branch `bar`, but
branch `foo` has a different worktree at `repo.bar/`, an error is raised.

**Special arguments**:

- `@` - current worktree (by path, works in detached HEAD)
- `-` - previous worktree (from switch history)
- `^` - main worktree

### Branch deletion

By default, branches are deleted only when their content is already in the target branch:

- no changes beyond the common ancestor — `git diff --name-only target...branch` is empty:
  no files changed between the merge base of `target`/`branch` and the tip of `branch`.
- same content as target — `git rev-parse branch^{tree}` equals `git rev-parse target^{tree}`:
  both branches point at the same tracked-files snapshot (tree), even if the commits differ.

This handles workflows where PRs are squash-merged or rebased, which don't preserve
commit ancestry but do integrate the content. Use `-D` to delete unintegrated
branches, or `--no-delete-branch` to always keep branches.

### Background removal (default)

- Returns immediately for continued work
- Logs: `.git/wt-logs/{branch}-remove.log`
- Use `--no-background` for foreground (blocking)

### Cleanup

Stops any git fsmonitor daemon for the worktree before removal. This prevents orphaned processes when using builtin fsmonitor (`core.fsmonitor=true`). No effect on Watchman users.

## Examples

Remove current worktree and branch:

```console
wt remove
```

Remove specific worktree and branch:

```console
wt remove feature-branch
```

Remove worktree but keep branch:

```console
wt remove --no-delete-branch feature-branch
```

Remove multiple worktrees:

```console
wt remove old-feature another-branch
```

Remove in foreground (blocking):

```console
wt remove --no-background feature-branch
```

Switch to default in main:

```console
wt remove  # (when already in main worktree)
```

<!-- END AUTO-GENERATED -->

---

## wt list

<!-- ⚠️ AUTO-GENERATED from `wt list --help-md` — edit source to update -->

```text
wt list — List worktrees and optionally branches
Usage: wt list [OPTIONS]
       wt list <COMMAND>

Commands:
  statusline  Single-line status for shell prompts

Options:
      --format <FORMAT>
          Output format (table, json)

          [default: table]

      --branches
          Include branches without worktrees

      --remotes
          Include remote branches

      --full
          Show CI, conflicts, diffs

      --progressive
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with remote data (CI, upstream) as it arrives. Auto-enabled for TTY.

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Columns

- **Branch:** Branch name
- **Status:** Quick status symbols (see Status Symbols below)
- **HEAD±:** Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
- **main↕:** Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
- **main…±** (`--full`): Line diffs in commits ahead of main (+added -deleted)
- **Path:** Worktree directory location
- **Remote⇅:** Commits ahead⇡/behind⇣ relative to tracking branch (e.g. `origin/branch`)
- **CI** (`--full`): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
  - `●` **passed** (green) - All checks passed
  - `●` **running** (blue) - Checks in progress
  - `●` **failed** (red) - Checks failed
  - `●` **conflicts** (yellow) - Merge conflicts with base
  - `●` **no-ci** (gray) - PR/MR or workflow found but no checks configured
  - **(blank)** - No PR/MR or workflow found, or `gh`/`glab` CLI unavailable
  - **(dimmed)** - Stale: unpushed local changes differ from PR/MR head
- **Commit:** Short commit hash (8 chars)
- **Age:** Time since last commit (relative)
- **Message:** Last commit message (truncated)

## Status Symbols

Order: `+!? ✖⚠≡_ ↻⋈ ↑↓↕ ⇡⇣⇅ ⎇⌫⊠`

- `+` Staged files (ready to commit)
- `!` Modified files (unstaged changes)
- `?` Untracked files present
- `✖` **Merge conflicts** - unresolved conflicts in working tree (fix before continuing)
- `⊘` **Would conflict** - merging into main would fail
- `≡` Working tree matches main (identical contents, regardless of commit history)
- `_` No commits (no commits ahead AND no uncommitted changes)
- `↻` Rebase in progress
- `⋈` Merge in progress
- `↑` Ahead of main branch
- `↓` Behind main branch
- `↕` Diverged (both ahead and behind main)
- `⇡` Ahead of remote tracking branch
- `⇣` Behind remote tracking branch
- `⇅` Diverged (both ahead and behind remote)
- `⎇` Branch indicator (shown for branches without worktrees)
- `⌫` Prunable worktree (directory missing, can be pruned)
- `⊠` Locked worktree (protected from auto-removal)

Rows are dimmed when there's no marginal contribution (`≡` matches main OR `_` no commits).

## JSON Output

Use `--format=json` for structured data. Each object contains two status maps
with the same fields in the same order as Status Symbols above:

**`status`** - variant names for querying:

- `working_tree`: `{untracked, modified, staged, renamed, deleted}` booleans
- `branch_state`: `""` | `"Conflicts"` | `"MergeTreeConflicts"` | `"MatchesMain"` | `"NoCommits"`
- `git_operation`: `""` | `"Rebase"` | `"Merge"`
- `main_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `upstream_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `user_status`: string (optional)

**`status_symbols`** - Unicode symbols for display (same fields, plus `worktree_attrs`: ⎇/⌫/⊠)

Note: `locked` and `prunable` are top-level fields on worktree objects, not in status.

**Worktree position fields** (for identifying special worktrees):

- `is_main`: boolean - is the main worktree
- `is_current`: boolean - is the current working directory (present when true)
- `is_previous`: boolean - is the previous worktree from `wt switch` (present when true)

**Query examples:**

```console
# Find worktrees with conflicts
jq '.[] | select(.status.branch_state == "Conflicts")'

# Find worktrees with untracked files
jq '.[] | select(.status.working_tree.untracked)'

# Find worktrees in rebase or merge
jq '.[] | select(.status.git_operation != "")'

# Get branches ahead of main
jq '.[] | select(.status.main_divergence == "Ahead")'

# Find locked worktrees
jq '.[] | select(.locked != null)'

# Get current worktree info (useful for statusline tools)
jq '.[] | select(.is_current == true)'
```

<!-- END AUTO-GENERATED -->

---

## wt config

<!-- ⚠️ AUTO-GENERATED from `wt config --help-md` — edit source to update -->

```text
wt config — Manage configuration and shell integration
Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create user configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  status     Manage branch status markers
  approvals  Manage command approvals

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Setup Guide

1. Set up shell integration

   ```console
   wt config shell install
   ```

   Or manually add to the shell config:

   ```console
   eval "$(wt config shell init bash)"
   ```

2. (Optional) Create user config file

   ```console
   wt config create
   ```

   This creates `~/.config/worktrunk/config.toml` with examples.

3. (Optional) Enable LLM commit messages

   Install: `uv tool install -U llm`
   Configure: `llm keys set anthropic`
   Add to user config:

   ```toml
   [commit-generation]
   command = "llm"
   ```

## LLM Setup Details

For Claude:

```console
llm install llm-anthropic
llm keys set anthropic
llm models default claude-haiku-4-5-20251001
```

For OpenAI:

```console
llm keys set openai
```

Use `wt config show` to view the current configuration.
Docs: <https://llm.datasette.io/> | <https://github.com/sigoden/aichat>

## Configuration Files

**User config**:

- Location: `~/.config/worktrunk/config.toml` (or `WORKTRUNK_CONFIG_PATH`)
- Run `wt config create --help` to view documented examples

**Project config**:

- Location: `.config/wt.toml` in repository root
- Contains: post-create, post-start, pre-commit, pre-merge, post-merge hooks

<!-- END AUTO-GENERATED -->

---

## wt step

<!-- ⚠️ AUTO-GENERATED from `wt step --help-md` — edit source to update -->

```text
wt step — Workflow building blocks
Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit       Commit changes with LLM commit message
  squash       Squash commits with LLM commit message
  push         Push changes to local target branch
  rebase       Rebase onto target
  post-create  Run post-create hook
  post-start   Run post-start hook
  pre-commit   Run pre-commit hook
  pre-merge    Run pre-merge hook
  post-merge   Run post-merge hook

Options:
  -h, --help  Print help

Global Options:
  -C <path>            Working directory for this command
      --config <path>  User config file path
  -v, --verbose        Show commands and debug info
```

<!-- END AUTO-GENERATED -->

---

## wt select

Interactive fzf-like fuzzy-search worktree picker with diff preview. Unix only.

Navigate worktrees with keyboard shortcuts:
- Arrow keys or `j`/`k` to move between worktrees
- Enter to switch to selected worktree
- `Esc` or `q` to cancel

### Preview tabs

Toggle with number keys:

| Key | Tab | Content |
|-----|-----|---------|
| `1` | Working tree | Uncommitted changes (like `git diff`) |
| `2` | Commit history | Recent commits; commits not on main are highlighted |
| `3` | Branch diff | Changes ahead of main (cumulative diff) |
