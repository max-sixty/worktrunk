# Worktrunk

<!-- User badges -->

[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![GitHub CI Status](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yml?event=push&branch=main&logo=github&style=for-the-badge)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

Worktrunk is a CLI tool which handles git worktree mechanics, enabling many
parallel AI agents

Git worktrees let multiple agents work on a single repo without colliding; each
agent gets a separate directory with a version of the code. But creating
worktrees, tracking paths & statuses, cleaning up, etc, is manual. Worktrunk
offers control, transparency & automation for this workflow; allowing
scaling the number of parallel agents.

## Demo

List worktrees, create a worktree, make a trivial change, merge the change:

![Worktrunk Demo](dev/wt-demo/out/wt-demo.gif)

## Quick Start

**Create a worktree:**

<!-- README:snapshot:tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap -->
```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```
<!-- README:end -->

...then do work. When ready:

**Merge it:**

<!-- README:snapshot:tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap -->
```bash
$ wt merge
🔄 Merging 1 commit to main @ a1b2c3d (no commit/squash/rebase needed)
   * a1b2c3d Implement JWT validation
    auth.rs | 13 +++++++++++++
    1 file changed, 13 insertions(+)
✅ Merged to main (1 commit, 1 file, +13)
🔄 Removing fix-auth worktree & branch in background
```
<!-- README:end -->

See [`wt merge`](#wt-merge) for all options.

**List worktrees:**

<!-- README:snapshot:tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap -->
```bash
$ wt list
  Branch     Status         HEAD±    main↕  Path         Remote⇅  Commit    Age   Message
@ main           ^                          ./test-repo   ↑0  ↓0  b834638e  10mo  Initial commit
+ bugfix-y       ↑                  ↑1      ./bugfix-y            412a27c8  10mo  Fix bug
+ feature-x  +   ↑        +5        ↑3      ./feature-x           7fd821aa  10mo  Add file 3

⚪ Showing 3 worktrees, 1 with changes, 2 ahead
```
<!-- README:end -->

See [`wt list`](#wt-list) for all options.

## Installation

```bash
cargo install worktrunk
wt config shell install  # Sets up shell integration
```

See [Shell Integration](#shell-integration) for details.

## Design Philosophy

Worktrunk is opinionated! It's designed for workflows which are:

- Trunk-based — lots of short-lived worktrees, linear commit histories
- Local — terminal-based agents, local inner dev loops

...and that means...

- Maximum automation: LLM commit messages, lifecycle hooks, Claude Code hooks
  - A robust "auto-merge when 'local-CI' passes" approach
- A small surface area: three core commands
- 1:1 mapping between worktree and branch, worktrees are addressed by their branch
- Defaults to "stage everything and squash merge" (but configurable)
- Extreme UI responsiveness; slow ops can't delay fast ones
- Pluggable; adopting Worktrunk for a portion of a workflow doesn't require
  adopting it for everything. Standard `git worktree` commands continue working
  fine!

## Automation Features

### LLM Commit Messages

Worktrunk can invoke external commands during merge operations to generate
commit messages, by passing the diff & a configurable prompt, and reading back a
formatted commit message. Simon Willison's [llm](https://llm.datasette.io/) tool
is recommended.

Add to `~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

Then `wt merge` will generate commit messages automatically:

<!-- README:snapshot:tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap -->
```bash
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
<!-- README:end -->

For more details, including custom prompt templates: `wt config --help`

### Project Hooks

Automate tasks at different points in the worktree lifecycle. Configure hooks in `.config/wt.toml`.

| Hook                    | When                               | On Failure     |
| ----------------------- | ---------------------------------- | -------------- |
| **post-create-command** | After worktree created             | Warn, continue |
| **post-start-command**  | After success message (background) | Warn, continue |
| **pre-commit-command**  | Before squash commit created       | Stop merge     |
| **pre-merge-command**   | After squash, before push          | Stop merge     |
| **post-merge-command**  | After successful merge             | Warn, continue |

```toml
# Install dependencies, build setup
[post-create-command]
"install" = "uv sync"

# Dev servers, file watchers (runs in background)
[post-start-command]
"dev" = "uv run dev"

# Tests and lints before merging (blocks on failure)
[pre-merge-command]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
```

<!-- README:snapshot:tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_post_create.snap -->
```bash
$ wt switch --create feature-x
🔄 Running post-create install:
   uv sync

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ Created new worktree for feature-x from main at ../repo.feature-x
🔄 Running post-start dev:
   uv run dev
```
<!-- README:end -->

<details>
<summary>Merging with pre-merge hooks</summary>

<!-- README:snapshot:tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_pre_merge.snap -->
```bash
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
<!-- README:end -->

</details>

See `wt switch --help` and `wt merge --help` for skipping hooks, template variables, security details.

### Shell Integration

Worktrunk requires shell integration in order to switch directories, during `wt
switch` & `wt merge`/`wt remove`. To add automatic setup to shell config files
(Bash, Zsh, and Fish):

```bash
wt config shell
```

For manual setup instructions, see `wt config shell --help`.

## Tips

**Create an alias for creating a new worktree + launching an agent** — Start a
new agent-in-worktree in a couple of seconds. For example, to create a worktree
and immediately start Claude:

```bash
alias wsl='wt switch --create --execute=claude'
```

Then:

```bash
wsl new-feature
```

...creates a branch, sets up the worktree, runs initialization hooks, and
launches Claude Code in that directory.

**Auto-generate commit messages** — Configure an LLM to generate commit
messages during merge. See [LLM Commit Messages](#llm-commit-messages).

**Automate startup with hooks** — Use `post-create-command` for environment
setup, `post-start-command` for non-blocking tasks. For example, worktrunk uses
`post-start-command` to bootstrap build caches from main via copy-on-write,
eliminating cold compiles (see [worktrunk's config](.config/wt.toml)). See
[Project Hooks](#project-hooks) for details.

**Use `post-merge-command` as a "local CI"** — Running `wt merge` on a worktree
and gating the merge on tests passing is very freeing — `main` is protected from
one agent forgetting to run all tests, without you having to babysit it.

**View Claude Code status from `wt list`** — The Claude Code integration shows
which branches have active sessions in `wt list`. When the agent is working, the
branch shows `🤖`; when it's waiting for the user, it shows `💬`. Setup
instructions: [Custom Worktree Status](#custom-worktree-status).

**Delegate to task runners** — Reference existing Taskfile/Justfile/Makefile commands
instead of duplicating logic:

```toml
[post-create-command]
"setup" = "task install"

[pre-merge-command]
"validate" = "just test lint"
```

**Use `^` as shorthand for the default branch** — Works everywhere: `wt switch ^`,
`wt merge ^`, `--base=^`.

## All Commands

<details>
<summary><strong><code>wt switch [branch]</code></strong> - Switch to existing worktree or create a new one</summary>

<!-- README:help:wt switch --help-md -->
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

```bash
wt switch feature-branch
```

Create new worktree from main:

```bash
wt switch --create new-feature
```

Switch to previous worktree:

```bash
wt switch -
```

Create from specific base:

```bash
wt switch --create hotfix --base production
```

Create and run command:

```bash
wt switch --create docs --execute "code ."
```

Skip hooks during creation:

```bash
wt switch --create temp --no-verify
```

## Shortcuts

Use `@` for current HEAD, `-` for previous, `^` for main:

```bash
wt switch @                              # Switch to current branch's worktree
wt switch -                              # Switch to previous worktree
wt switch --create new-feature --base=^  # Branch from main (default)
wt switch --create bugfix --base=@       # Branch from current HEAD
wt remove @                              # Remove current worktree
```
<!-- README:end -->

</details>

<details id="wt-merge">
<summary><strong><code>wt merge [target]</code></strong> - Merge, push, and cleanup</summary>

<!-- README:help:wt merge --help-md -->
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

Uncommitted changes are staged and committed with LLM message.
Use `--stage=tracked` to stage only tracked files, or `--stage=none` to commit only what's already staged.

### Squash

Multiple commits are squashed into one with LLM message.
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

```bash
wt merge
```

Merge without squashing:

```bash
wt merge --no-squash
```

Keep worktree after merging:

```bash
wt merge --no-remove
```

Skip all hooks:

```bash
wt merge --no-verify
```
<!-- README:end -->

</details>

<details>
<summary><strong><code>wt remove [worktree]</code></strong> - Remove worktree and branch</summary>

<!-- README:help:wt remove --help-md -->
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

- Returns immediately so you can continue working
- Logs: `.git/wt-logs/{branch}-remove.log`
- Use `--no-background` for foreground (blocking)

### Cleanup

Stops any git fsmonitor daemon for the worktree before removal. This prevents orphaned processes when using builtin fsmonitor (`core.fsmonitor=true`). No effect on Watchman users.

## Examples

Remove current worktree and branch:

```bash
wt remove
```

Remove specific worktree and branch:

```bash
wt remove feature-branch
```

Remove worktree but keep branch:

```bash
wt remove --no-delete-branch feature-branch
```

Remove multiple worktrees:

```bash
wt remove old-feature another-branch
```

Remove in foreground (blocking):

```bash
wt remove --no-background feature-branch
```

Switch to default in main:

```bash
wt remove  # (when already in main worktree)
```
<!-- README:end -->

</details>

<details id="wt-list">
<summary><strong><code>wt list</code></strong> - Show all worktrees and branches</summary>

<!-- README:help:wt list --help-md -->
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
- **Remote⇅:** Commits ahead↑/behind↓ relative to tracking branch (e.g. `origin/branch`)
- **CI** (`--full`): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
  - `●` **passed** (green) - All checks passed
  - `●` **running** (blue) - Checks in progress
  - `●` **failed** (red) - Checks failed
  - `●` **conflicts** (yellow) - Merge conflicts with base
  - `●` **no-ci** (gray) - PR/MR or workflow found but no checks configured
  - (blank) - No PR/MR or workflow found, or `gh`/`glab` CLI unavailable
  - (dimmed) - Stale: unpushed local changes differ from PR/MR head
- **Commit:** Short commit hash (8 chars)
- **Age:** Time since last commit (relative)
- **Message:** Last commit message (truncated)

## Status Symbols

Order: `?!+»✘ ✖⚠≡∅ ↻⋈ ↑↓↕ ⇡⇣⇅ ⎇⌫⊠`

- `?` Untracked files present
- `!` Modified files (unstaged changes)
- `+` Staged files (ready to commit)
- `»` Renamed files
- `✘` Deleted files
- `✖` **Merge conflicts** - unresolved conflicts in working tree (fix before continuing)
- `⚠` **Would conflict** - merging into main would fail
- `≡` Working tree matches main (identical contents, regardless of commit history)
- `∅` No commits (no commits ahead AND no uncommitted changes)
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

*Rows are dimmed when no unique work (≡ matches main OR ∅ no commits).*

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

**Query examples:**

```bash
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
```
<!-- README:end -->

</details>

<details>
<summary><strong><code>wt config</code></strong> - Manage configuration</summary>

<!-- README:help:wt config --help-md -->
```text
wt config — Manage configuration and shell integration
Usage: config <COMMAND>

Commands:
  shell          Shell integration setup
  create         Create global configuration file
  list           List configuration files & locations
  refresh-cache  Refresh default branch from remote
  status         Manage branch status markers
  approvals      Manage command approvals
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

```

## Setup Guide

1. Set up shell integration

   ```bash
   wt config shell install
   ```

   Or manually add to your shell config:

   ```bash
   eval "$(wt config shell init bash)"
   ```

2. (Optional) Create config file

   ```bash
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

```bash
llm install llm-anthropic
llm keys set anthropic
llm models default claude-3.5-sonnet
```

For OpenAI:

```bash
llm keys set openai
```

Use `wt config list` to view your current configuration.
Docs: <https://llm.datasette.io/> | <https://github.com/sigoden/aichat>

## Configuration Files

**Global config** (user settings):

- Location: `~/.config/worktrunk/config.toml` (or `WORKTRUNK_CONFIG_PATH`)
- Run `wt config create --help` to view documented examples

**Project config** (repository hooks):

- Location: `.config/wt.toml` in repository root
- Contains: post-create, post-start, pre-commit, pre-merge, post-merge hooks
<!-- README:end -->

</details>

<details>
<summary><strong><code>wt step</code></strong> - Building blocks for workflows</summary>

<!-- README:help:wt step --help-md -->
```text
wt step — Primitive operations (building blocks for workflows)
Usage: step <COMMAND>

Commands:
  commit       Commit changes with LLM message
  squash       Squash commits with LLM message
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
<!-- README:end -->

</details>

## Configuration

By default, worktrees live as siblings to the main repo (`myapp.feature-x/`).

```bash
wt config list    # Show all config files and locations
wt config create  # Create global config with examples
wt config --help  # Show setup guide, file locations, and options
```

## Advanced Features

### Custom Worktree Status

Add emoji status markers to branches that appear in `wt list`.

```bash
# Set status for current branch
wt config status set "🤖"

# Or use git config directly
git config worktrunk.status.feature-x "💬"
```

**Status appears in the Status column:**

<!-- README:snapshot:tests/snapshots/integration__integration_tests__list__with_user_status.snap -->
```bash
$ wt list
  Branch             Status         HEAD±    main↕  Path                 Remote⇅  Commit    Age   Message
@ main                   ^                          ./test-repo                   b834638e  10mo  Initial commit
+ clean-no-status       ∅                           ./clean-no-status             b834638e  10mo  Initial commit
+ clean-with-status     ∅   💬                      ./clean-with-status           b834638e  10mo  Initial commit
+ dirty-no-status     !           +1   -1           ./dirty-no-status             b834638e  10mo  Initial commit
+ dirty-with-status    ?∅   🤖                      ./dirty-with-status           b834638e  10mo  Initial commit

⚪ Showing 5 worktrees, 1 with changes
```
<!-- README:end -->

The custom emoji appears directly after the git status symbols.

<details>
<summary>Automation with Claude Code Hooks</summary>

Claude Code can automatically set/clear emoji status when coding sessions start and end.

When using Claude:

- Sets status to `🤖` for the current branch when submitting a prompt (working)
- Changes to `💬` when Claude needs input (waiting for permission or idle)
- Clears the status completely when the session ends

<!-- README:snapshot:tests/snapshots/integration__integration_tests__list__with_user_status.snap -->
```bash
$ wt list
  Branch             Status         HEAD±    main↕  Path                 Remote⇅  Commit    Age   Message
@ main                   ^                          ./test-repo                   b834638e  10mo  Initial commit
+ clean-no-status       ∅                           ./clean-no-status             b834638e  10mo  Initial commit
+ clean-with-status     ∅   💬                      ./clean-with-status           b834638e  10mo  Initial commit
+ dirty-no-status     !           +1   -1           ./dirty-no-status             b834638e  10mo  Initial commit
+ dirty-with-status    ?∅   🤖                      ./dirty-with-status           b834638e  10mo  Initial commit

⚪ Showing 5 worktrees, 1 with changes
```
<!-- README:end -->

**How it works:**

- Status is stored as `worktrunk.status.<branch>` in `.git/config`
- Each branch can have its own status emoji
- The hooks automatically detect the current branch and set/clear its status
- Works with any git repository, no special configuration needed

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

Commands from project hooks and LLM configuration require approval on first run. Approved commands are saved to `~/.config/worktrunk/config.toml` under the project's configuration. If a command changes, worktrunk requires new approval.

**Example approval prompt:**

<!-- README:snapshot:tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_approval_prompt.snap -->
```
🟡 test-repo needs approval to execute 3 commands:

🔄 post-create install:
   echo 'Installing dependencies...'

🔄 post-create build:
   echo 'Building project...'

🔄 post-create test:
   echo 'Running tests...'

💡 Allow and remember? [y/N]
```
<!-- README:end -->

Use `--force` to bypass prompts (useful for CI/automation).

</details>

<details>
<summary><strong>How does Worktrunk compare to alternatives?</strong></summary>

### vs. Branch Switching

`git checkout` forces all work through a single directory. Switching branches means rebuilding artifacts, restarting dev servers, and stashing changes. Only one branch can be active at a time.

Worktrunk gives each branch its own directory with independent build caches, processes, and editor state. Work on multiple branches simultaneously without rebuilding or stashing.

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

Worktrunk adds path management, lifecycle hooks, and `wt list --full` for viewing all worktrees—branches, uncommitted changes, commits ahead/behind, CI status, and conflicts—in a single view.

### vs. git-machete / git-town

Different scopes:

- **git-machete**: Branch stack management in a single directory
- **git-town**: Git workflow automation in a single directory
- **worktrunk**: Multi-worktree management with hooks and status aggregation

These tools can be used together—run git-machete or git-town inside individual worktrees.

### vs. Git TUIs (lazygit, gh-dash, etc.)

Git TUIs operate on a single repository. Worktrunk manages multiple worktrees, runs automation hooks, and aggregates status across branches (`wt list --full`). Use your preferred TUI inside each worktree directory.

</details>

<details>
<summary><strong>Installation fails with C compilation errors</strong></summary>

If you encounter errors related to tree-sitter or C compilation (like "error: 'for' loop initial declarations are only allowed in C99 mode" or "undefined reference to le16toh"), install without syntax highlighting:

```bash
cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.

</details>

<details>
<summary>Developing</summary>

### Releases

Use [cargo-release](https://github.com/crate-ci/cargo-release) to publish new versions:

```bash
cargo install cargo-release

# Bump version, update Cargo.lock, commit, tag, and push
cargo release patch --execute   # 0.1.0 -> 0.1.1
cargo release minor --execute   # 0.1.0 -> 0.2.0
cargo release major --execute   # 0.1.0 -> 1.0.0
```

This updates Cargo.toml and Cargo.lock, creates a commit and tag, then pushes to GitHub. The tag push triggers GitHub Actions to build binaries, create the release, and publish to crates.io.

Run without `--execute` to preview changes first.

</details>
