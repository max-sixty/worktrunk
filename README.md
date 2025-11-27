# Worktrunk

<!-- User badges -->

[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![GitHub CI Status](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yml?event=push&branch=main&logo=github&style=for-the-badge)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

Worktrunk is a CLI for Git worktree management, designed for parallel AI agent workflows. Worktrees give each agent an isolated branch and directory; Worktrunk adds branch-based navigation, lifecycle hooks, and unified status. The goal is to make spinning up a new AI "developer" for a task feel as routine as `git switch`.

## Demo

![Worktrunk Demo](dev/wt-demo/out/wt-demo.gif)

## Quick Start

### 1. Install

```console
$ cargo install worktrunk
$ wt config shell install  # allows commands to change directories
```

### 2. Create a worktree

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

```console
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```

<!-- END AUTO-GENERATED -->

This creates `../repo.fix-auth` on branch `fix-auth`.

### 3. Merge it

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap — edit source to update -->

```console
$ wt merge
🔄 Merging 1 commit to main @ a1b2c3d (no commit/squash/rebase needed)
   * a1b2c3d Implement JWT validation
    auth.rs | 13 +++++++++++++
    1 file changed, 13 insertions(+)
✅ Merged to main (1 commit, 1 file, +13)
🔄 Removing fix-auth worktree & branch in background
```

<!-- END AUTO-GENERATED -->

`wt merge` handles the full workflow: stage, commit, squash, rebase, run hooks,
merge, cleanup.

### 4. List worktrees

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap — edit source to update -->

```console
$ wt list
Branch     Status         HEAD±    main↕  Path              Remote⇅  Commit    Age   Message
@ main           ^                          ./repo             ⇡0  ⇣0  a1b2c3d     1d    Initial commit
+ feature-x  +   ↑        +5        ↑3      ./repo.feature-x           a1b2c3d     1h    Add file 3
+ bugfix-y       ↑                  ↑1      ./repo.bugfix-y            a1b2c3d     2h    Fix bug

⚪ Showing 3 worktrees, 1 with changes, 2 ahead
```

<!-- END AUTO-GENERATED -->

`--full` adds CI status and conflicts. `--branches` includes all branches.

## Why worktrees?

Parallel agents need isolated working directories that share one Git history.

We can get that a few ways:

- multiple clones — slow to set up, waste disk, drift out of sync
- one working tree with many branches — constant stashing, rebasing, and conflict risk
- git worktrees — multiple directories backed by a single `.git` directory

So we use git worktrees: many working directories, one repository.

## Why Worktrunk?

Git's worktree commands handle the basics, but leave lifecycle management to you.
Worktrunk wraps creation, hooks, merging, and cleanup into three commands — `wt switch`, `wt merge`, `wt list`. A comparison:

<table>
<tr>
<th>Task</th>
<th>Worktrunk</th>
<th>Plain git</th>
</tr>
<tr>
<td>Create + start Claude</td>
<td><pre lang="bash">wt switch -c feature -x claude</pre></td>
<td><pre lang="bash">git worktree add -b feature ../repo.feature main
cd ../repo.feature && claude</pre></td>
</tr>
<tr>
<td>Merge + clean up</td>
<td><pre lang="bash">wt merge</pre></td>
<td><pre lang="bash">cd ../repo && git merge feature
git worktree remove ../repo.feature
git branch -d feature</pre></td>
</tr>
<tr>
<td>Full workflow</td>
<td><pre lang="bash"># after configuring hooks
wt merge</pre></td>
<td><pre lang="bash">git add -A
git reset --soft $(git merge-base HEAD main)
# commit-generation
git diff --staged | llm "msg" | git commit -F -
git rebase main
# pre-merge hook
cargo test
cd ../repo && git merge --ff-only feature
git worktree remove ../repo.feature
git branch -d feature
# post-merge hook
cargo install --path .</pre></td>
</tr>
</table>

## Automation

### LLM commit messages

Worktrunk can invoke external commands to generate commit messages.
[llm](https://llm.datasette.io/) is recommended.

`~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

`wt merge` generates commit messages automatically:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap — edit source to update -->

```console
$ wt merge
🔄 Squashing 3 commits into a single commit (3 files, +33)...
🔄 Generating squash commit message...
   feat(auth): Implement JWT authentication system

   Add comprehensive JWT token handling including validation, refresh logic,
   and authentication tests. This establishes the foundation for secure
   API authentication.

   - Implement token refresh mechanism with expiry handling
   - Add JWT encoding/decoding with signature verification
   - Create test suite covering all authentication flows
✅ Squashed @ a1b2c3d
🔄 Running pre-merge test:
   cargo test
🔄 Running pre-merge lint:
   cargo clippy
🔄 Merging 1 commit to main @ a1b2c3d (no rebase needed)
   * a1b2c3d feat(auth): Implement JWT authentication system
    auth.rs      |  8 ++++++++
    auth_test.rs | 17 +++++++++++++++++
    jwt.rs       |  8 ++++++++
    3 files changed, 33 insertions(+)
✅ Merged to main (1 commit, 3 files, +33)
🔄 Removing feature-auth worktree & branch in background
🔄 Running post-merge install:
   cargo install --path .
```

<!-- END AUTO-GENERATED -->

`wt step commit` runs just the commit step. Custom prompt templates: `wt config --help`.

### Project hooks

Configure hooks in `.config/wt.toml`:

| Hook             | When                                | Example                        |
| ---------------- | ----------------------------------- | ------------------------------ |
| **post-create**  | After worktree created              | `cp -r .cache`, `ln -s`        |
| **post-start**   | After worktree created (background) | `npm install`, `cargo build`   |
| **pre-commit**   | Before squash commit created        | `pre-commit run`               |
| **pre-merge**    | After squash, before push           | `cargo test`, `pytest`         |
| **post-merge**   | After successful merge              | `cargo install --path .`       |

```toml
# Install dependencies, build setup
[post-create]
"install" = "uv sync"

# Dev servers, file watchers (runs in background)
[post-start]
"dev" = "uv run dev"

# Tests and lints before merging (blocks on failure)
[pre-merge]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
```

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_post_create.snap — edit source to update -->

```console
$ wt switch --create feature-x
🔄 Running post-create install:
   uv sync

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ Created new worktree for feature-x from main at ../repo.feature-x
🔄 Running post-start dev:
   uv run dev
```

<!-- END AUTO-GENERATED -->

<details>
<summary>Merging with pre-merge hooks</summary>

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_pre_merge.snap — edit source to update -->

```console
$ wt merge
🔄 Squashing 3 commits into a single commit (2 files, +45)...
🔄 Generating squash commit message...
   feat(api): Add user authentication endpoints

   Implement login and token refresh endpoints with JWT validation.
   Includes comprehensive test coverage and input validation.
✅ Squashed @ a1b2c3d
🔄 Running pre-merge test:
   uv run pytest

============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================

🔄 Running pre-merge lint:
   uv run ruff check

All checks passed!

🔄 Merging 1 commit to main @ a1b2c3d (no rebase needed)
   * a1b2c3d feat(api): Add user authentication endpoints
    api/auth.py        | 31 +++++++++++++++++++++++++++++++
    tests/test_auth.py | 14 ++++++++++++++
    2 files changed, 45 insertions(+)
✅ Merged to main (1 commit, 2 files, +45)
🔄 Removing feature-auth worktree & branch in background
```

<!-- END AUTO-GENERATED -->

</details>

See `wt switch --help` and `wt merge --help` for skipping hooks, template variables, security details.

### Shell integration

Shell integration lets `wt switch`, `wt merge`, and `wt remove` change
directories:

```console
wt config shell install  # Bash, Zsh, Fish
```

Manual setup: `wt config shell --help`.

## Tips & patterns

**Alias for new worktree + agent:**

```console
alias wsl='wt switch --create --execute=claude'
wsl new-feature  # Creates worktree, runs hooks, launches Claude
```

**Eliminate cold starts** — `post-create` hooks install deps and copy caches.
See [`.config/wt.toml`](.config/wt.toml) for an example using copy-on-write.

**Local CI gate** — `pre-merge` hooks run before merging. Failures abort the
merge.

**Track agent status** — Custom emoji markers show agent state in `wt list`.
Claude Code hooks can set these automatically. See [Custom Worktree
Status](#custom-worktree-status).

**Monitor CI across branches** — `wt list --full --branches` shows PR/CI status
for all branches, including those without worktrees. CI column links to PR pages
in terminals with hyperlink support.

**JSON API** — `wt list --format=json` for dashboards, statuslines, scripts.

**Task runners** — Reference Taskfile/Justfile/Makefile in hooks:

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

**Shortcuts** — `^` = default branch, `@` = current branch, `-` = previous
worktree. Example: `wt switch --create hotfix --base=@` branches from current
HEAD.

## Commands

<details>
<summary><strong><code>wt switch [branch]</code></strong> - Switch to existing worktree or create a new one</summary>

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-md` — edit source to update -->

```text
wt switch — Switch to a worktree
Usage: switch [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>
          Branch, path, '@' (HEAD), '-' (previous), or '^' (main)

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

```

## Operation

### Switching to Existing Worktree

- If worktree exists for branch, changes directory via shell integration
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

</details>

<details id="wt-merge">
<summary><strong><code>wt merge [target]</code></strong> - Merge, push, and cleanup</summary>

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-md` — edit source to update -->

```text
wt merge — Merge worktree into target branch
Usage: merge [OPTIONS] [TARGET]

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

</details>

<details>
<summary><strong><code>wt remove [worktree]</code></strong> - Remove worktree and branch</summary>

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-md` — edit source to update -->

```text
wt remove — Remove worktree and branch
Usage: remove [OPTIONS] [WORKTREES]...

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

```

## Operation

Removes worktree directory, git metadata, and branch. Requires clean working tree.

### No arguments (remove current)

- Removes current worktree and switches to main worktree
- In main worktree: switches to default branch

### By name (remove specific)

- Removes specified worktree(s) and branches
- Current worktree removed last (switches to main first)

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

</details>

<details id="wt-list">
<summary><strong><code>wt list</code></strong> - Show all worktrees and branches</summary>

<!-- ⚠️ AUTO-GENERATED from `wt list --help-md` — edit source to update -->

```text
wt list — List worktrees and optionally branches
Usage: list [OPTIONS]

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

*Rows are dimmed when there's no marginal contribution (≡ matches main OR _ no commits).*

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

- `is_main`: boolean - is the main/default worktree
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

</details>

<details id="wt-config">
<summary><strong><code>wt config</code></strong> - Manage configuration</summary>

<!-- ⚠️ AUTO-GENERATED from `wt config --help-md` — edit source to update -->

```text
wt config — Manage configuration and shell integration
Usage: config <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create global configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  status     Manage branch status markers
  approvals  Manage command approvals
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

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

2. (Optional) Create config file

   ```console
   wt config create
   ```

   This creates ~/.config/worktrunk/config.toml with examples.

3. (Optional) Enable LLM commit messages

   Install: `uv tool install -U llm`
   Configure: `llm keys set anthropic`
   Add to config.toml:

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

**Global config** (user settings):

- Location: `~/.config/worktrunk/config.toml` (or `WORKTRUNK_CONFIG_PATH`)
- Run `wt config create --help` to view documented examples

**Project config** (repository hooks):

- Location: `.config/wt.toml` in repository root
- Contains: post-create, post-start, pre-commit, pre-merge, post-merge hooks

<!-- END AUTO-GENERATED -->

</details>

<details>
<summary><strong><code>wt step</code></strong> - Building blocks for workflows</summary>

<!-- ⚠️ AUTO-GENERATED from `wt step --help-md` — edit source to update -->

```text
wt step — Workflow building blocks
Usage: step <COMMAND>

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
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

<!-- END AUTO-GENERATED -->

</details>

## Advanced Features

### Custom Worktree Status

Add emoji status markers to branches that appear in `wt list`.

```console
# Set status for current branch
wt config status set "🤖"

# Or use git config directly
git config worktrunk.status.feature-x "💬"
```

**Status appears in the Status column:**

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__with_user_status.snap — edit source to update -->

```console
$ wt list
Branch             Status         HEAD±    main↕  Path                      Remote⇅  Commit    Age   Message
@ main                   ^                          ./repo                             a1b2c3d     1d    Initial commit
+ clean-no-status       _                           ./repo.clean-no-status             a1b2c3d     1d    Initial commit
+ clean-with-status     _   💬                      ./repo.clean-with-status           a1b2c3d     1d    Initial commit
+ dirty-no-status     !           +1   -1           ./repo.dirty-no-status             a1b2c3d     1d    Initial commit
+ dirty-with-status    ?_   🤖                      ./repo.dirty-with-status           a1b2c3d     1d    Initial commit

⚪ Showing 5 worktrees, 1 with changes
```

<!-- END AUTO-GENERATED -->

The custom emoji appears directly after the git status symbols.

<details>
<summary>Automation with Claude Code Hooks</summary>

Claude Code can automatically set/clear emoji status when coding sessions start and end.

When using Claude:

- Sets status to `🤖` for the current branch when submitting a prompt (working)
- Changes to `💬` when Claude needs input (waiting for permission or idle)
- Clears the status completely when the session ends

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__with_user_status.snap — edit source to update -->

```console
$ wt list
Branch             Status         HEAD±    main↕  Path                      Remote⇅  Commit    Age   Message
@ main                   ^                          ./repo                             a1b2c3d     1d    Initial commit
+ clean-no-status       _                           ./repo.clean-no-status             a1b2c3d     1d    Initial commit
+ clean-with-status     _   💬                      ./repo.clean-with-status           a1b2c3d     1d    Initial commit
+ dirty-no-status     !           +1   -1           ./repo.dirty-no-status             a1b2c3d     1d    Initial commit
+ dirty-with-status    ?_   🤖                      ./repo.dirty-with-status           a1b2c3d     1d    Initial commit

⚪ Showing 5 worktrees, 1 with changes
```

<!-- END AUTO-GENERATED -->

**How it works:**

- Status is stored as `worktrunk.status.<branch>` in `.git/config`
- Each branch can have its own status emoji
- The hooks automatically detect the current branch and set/clear its status
- Works with any git repository, no special configuration needed

</details>

Experimental commands under `wt beta`. Interface may change.

### `wt beta select`

Interactive worktree selector with fuzzy search and diff preview. Unix only.

Preview tabs (toggle with `1`/`2`/`3`):
- **Tab 1**: Working tree changes (uncommitted)
- **Tab 2**: History (commits not on main highlighted)
- **Tab 3**: Branch diff (changes ahead of main)

### `wt beta statusline`

Single-line status for shell prompts, starship, or editor integrations.

**Claude Code integration** (`--claude-code`): Reads workspace context from
stdin, outputs directory, branch status, and model name.

```
~/w/myproject.feature-auth  !🤖  ±+42 -8  ↑3  ⇡1  ●  | Opus
```

<details>
<summary>Claude Code configuration</summary>

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt beta statusline --claude-code"
  }
}
```

</details>

## Project Status

Worktrunk is in active development. The core features are stable and ready for
use. There may be backward-incompatible changes.

The most helpful way to contribute:

- Use it!
- Star the repo / tell friends / post about it
- Find bugs, file reproducible bug reports

## FAQ

<details>
<summary><strong>What commands does Worktrunk execute?</strong></summary>

Worktrunk executes commands in three contexts:

1. **Project hooks** (`.config/wt.toml`) - Automation for worktree lifecycle
2. **LLM commands** (`~/.config/worktrunk/config.toml`) - Commit message generation
3. **--execute flag** - Commands provided explicitly

Commands from project hooks and LLM configuration require approval on first run. Approved commands are saved to `~/.config/worktrunk/config.toml` under the project's configuration. If a command changes, Worktrunk requires new approval.

**Example approval prompt:**

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_approval_prompt.snap — edit source to update -->

```
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

</details>

<details>
<summary><strong>How does Worktrunk compare to alternatives?</strong></summary>

### vs. Branch Switching

Branch switching uses one directory, so only one agent can work at a time.
Worktrees give each agent its own directory.

### vs. Plain `git worktree`

Git's built-in worktree commands work but require manual lifecycle management:

```console
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

```console
wt switch --create feature-branch  # Creates worktree, runs setup hooks
# ...work...
wt merge                            # Squashes, merges, removes worktree
```

What `git worktree` doesn't provide:

- Consistent directory naming and cleanup validation
- Project-specific automation (install dependencies, start services)
- Unified status across all worktrees (commits, CI, conflicts, changes)

Worktrunk adds path management, lifecycle hooks, and `wt list --full` for viewing all worktrees—branches, uncommitted changes, commits ahead/behind, CI status, and conflicts—in a single view.

### vs. git-machete / git-town

Different scopes:

- **git-machete**: Branch stack management in a single directory
- **git-town**: Git workflow automation in a single directory
- **worktrunk**: Multi-worktree management with hooks and status aggregation

These tools can be used together—run git-machete or git-town inside individual worktrees.

### vs. Git TUIs (lazygit, gh-dash, etc.)

Git TUIs operate on a single repository. Worktrunk manages multiple worktrees,
runs automation hooks, and aggregates status across branches. TUIs work inside
each worktree directory.

</details>

<details>
<summary><strong>Installation fails with C compilation errors</strong></summary>

Errors related to tree-sitter or C compilation (C99 mode, `le16toh` undefined)
can be avoided by installing without syntax highlighting:

```console
cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.

</details>

<details>
<summary><strong>Any notes for developing this crate?</strong></summary>

### Running Tests

**Quick tests (no external dependencies):**
```bash
cargo test --lib --bins           # Unit tests (~200 tests)
cargo test --test integration     # Integration tests without shell tests (~300 tests)
```

**Full integration tests (requires bash, zsh, fish):**
```bash
cargo test --test integration --features shell-integration-tests
```

**Dependencies for shell integration tests:**
- bash, zsh, fish shells
- Quick setup: `./dev/setup-claude-code-web.sh` (installs shells on Linux)

### Releases

Use [cargo-release](https://github.com/crate-ci/cargo-release) to publish new versions:

```console
cargo install cargo-release

# Bump version, update Cargo.lock, commit, tag, and push
cargo release patch --execute   # 0.1.0 -> 0.1.1
cargo release minor --execute   # 0.1.0 -> 0.2.0
cargo release major --execute   # 0.1.0 -> 1.0.0
```

This updates Cargo.toml and Cargo.lock, creates a commit and tag, then pushes to GitHub. The tag push triggers GitHub Actions to build binaries, create the release, and publish to crates.io.

Run without `--execute` to preview changes first.

</details>
