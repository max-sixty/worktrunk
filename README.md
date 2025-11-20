# Worktrunk

<!-- User badges -->

[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![GitHub CI Status](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yml?event=push&branch=main&logo=github&style=for-the-badge)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

Worktrunk is a CLI tool which makes working with git worktrees much much easier.
It's designed for those running many concurrent AI coding agents.

For context, git worktrees let multiple agents work on a single repo without
colliding; each agent gets a separate directory with a version of the code. But
creating worktrees, tracking paths & statuses, cleaning up, etc is manual.
Worktrunk offers transparency and automation for this lifecycle.

## Quick Start

**Create a worktree:**

<!-- Output from: tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap -->

```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth/
```

**Work, make changes, then merge back:**

<!-- Output from: tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap -->

```bash
$ wt merge
🔄 Merging 1 commit to main @ a1b2c3d (no commit/squash/rebase needed)

   * a1b2c3d (HEAD -> fix-auth) Implement JWT validation
    auth.rs | 13 +++++++++++++
    1 file changed, 13 insertions(+)
✅ Merged to main (1 commit, 1 file, +13)
🔄 Removing worktree & branch...
✅ Removed worktree & branch for fix-auth, changed directory to ../repo/
# Shell back in main
```

**See all active worktrees:**

<!-- Output from: tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap -->

```bash
$ wt list
Branch     Status  HEAD±  main↕  Path         Remote⇅  Commit    Age            Message
main                             ./test-repo  ↑0 ↓0    b834638e  10 months ago  Initial commit
bugfix-y   ↑              ↑1     ./bugfix-y            412a27c8  10 months ago  Fix bug
feature-x  +       ↑      +5 ↑3  ./feature-x           7fd821aa  10 months ago  Add file 3

⚪ Showing 3 worktrees, 1 with changes, 2 ahead
```

## Installation

```bash
cargo install worktrunk
wt config shell  # Sets up shell integration
```

## Design Philosophy

Worktrunk is opinionated! It's not designed to be all things to all people. The choices optimize for agent workflows:

- Lots of short-lived worktrees
- Trunk-based development
- CLI-based agents
- Inner dev loops are local
- Shell navigation
- Commits are squashed into linear histories
- Maximum automation

Adopting Worktrunk for a portion of a workflow doesn't require adopting it for
everything — standard `git worktree` commands continue working fine.

## Automation Features

### LLM-Authored Commit Messages

Worktrunk can invoke an external commands during merge operations to generate
commit messages. Simon Willison's [llm](https://llm.datasette.io/) tool reads
the diff and a configurable prompt, then returns a formatted commit message.

Add to `~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

Then `wt merge` will generate commit messages automatically:

<!-- Output from: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap -->

```bash
$ wt merge
🔄 Squashing 3 commits into 1 (3 files, +33)...
🔄 Generating squash commit message...
  feat(auth): Implement JWT authentication system

  Add comprehensive JWT token handling including validation, refresh logic,
  and authentication tests. This establishes the foundation for secure
  API authentication.

  - Implement token refresh mechanism with expiry handling
  - Add JWT encoding/decoding with signature verification
  - Create test suite covering all authentication flows
✅ Squashed @ a1b2c3d
```

To set up integration: run `wt config --help` to see the setup guide, or `wt config init` to create an example config file.

<details>
<summary>Advanced: Custom Prompt Templates</summary>

Worktrunk uses [minijinja
templates](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) for
commit message prompts. Customize the prompts by setting `template` (inline) or
`template-file` (external file) in the `[commit-generation]` section. Use
`squash-template` / `squash-template-file` for squash commits.

See [`config.example.toml`](config.example.toml) for complete template examples
with all available variables (`git_diff`, `branch`, `recent_commits`, `commits`,
`target_branch`, `repo`).

</details>

### Project Hooks

Automate common tasks by creating `.config/wt.toml` in the repository root. Install dependencies when creating worktrees, start dev servers automatically, run tests before merging.

```toml
# Install deps when creating a worktree
[post-create-command]
"install" = "uv sync"

# Start dev server automatically
[post-start-command]
"dev" = "uv run dev"

# Run tests before merging
[pre-merge-command]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
```

**Example: Creating a worktree with hooks:**

<!-- Output from: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_post_create.snap -->

```bash
$ wt switch --create feature-x
🔄 Running post-create install:
  uv sync
✅ Created new worktree for feature-x from main at ../repo.feature-x/
🔄 Running post-start dev:
  uv run dev

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
```

**Example: Merging with pre-merge hooks:**

<!-- Output from: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_pre_merge.snap -->

```bash
$ wt merge
🔄 Squashing 3 commits into 1 (2 files, +45)...
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
  * a1b2c3d (HEAD -> feature-auth) feat(api): Add user authentication endpoints
   api/auth.py        | 31 +++++++++++++++++++++++++++++++
   tests/test_auth.py | 14 ++++++++++++++
   2 files changed, 45 insertions(+)
✅ Merged to main (1 commit, 2 files, +45)
🔄 Removing worktree & branch...
✅ Removed worktree & branch for feature-auth, changed directory to ../repo/
```

<details>
<summary>All available hooks</summary>

| Hook                    | When It Runs                                                                   | Execution                                     | Failure Behavior             |
| ----------------------- | ------------------------------------------------------------------------------ | --------------------------------------------- | ---------------------------- |
| **post-create-command** | After `git worktree add` completes                                             | Sequential, blocking                          | Logs warning, continues      |
| **post-start-command**  | After post-create completes                                                    | Parallel, non-blocking (background processes) | Logs warning, continues      |
| **pre-commit-command**  | Before committing changes during `wt merge` (both squash and no-squash modes)  | Sequential, blocking, fail-fast               | Terminates merge immediately |
| **pre-merge-command**   | After rebase completes during `wt merge` (validates rebased state before push) | Sequential, blocking, fail-fast               | Terminates merge immediately |
| **post-merge-command**  | After successful merge and push to target branch, before cleanup               | Sequential, blocking                          | Logs warning, continues      |

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`

**Skipping hooks:** `wt switch --no-verify` or `wt merge --no-verify`

**Security:** Commands require approval on first run. Use `--force` to bypass.

</details>

## Tips

**Create an alias for an agent** - Shell aliases streamline common workflows. For example, to create a worktree and immediately start Claude:

```bash
alias wsl='wt switch --create --execute=claude'
```

Now `wsl new-feature` creates a branch, sets up the worktree, runs initialization hooks, and launches Claude in that directory.

**Automatic branch status in Claude Code** - The Claude Code integration shows
which branches have active sessions. When Claude starts working, the branch
shows `🤖` in `wt list`. When waiting for input, it shows `💬`. Setup
instructions: [Custom Worktree Status](#custom-worktree-status).

**Auto-generated commit messages** - Simon Willison's
[llm](https://llm.datasette.io/) tool integrates seamlessly with worktrunk's
commit generation. Install it, configure the command, and `wt merge` will
automatically generate contextual commit messages. Setup guide: [LLM-Authored
Commit Messages](#llm-authored-commit-messages).

**Environment setup with hooks** - Use `post-create-command` (or
`post-start-command` for non-blocking) to run setup for that
path:

```toml
# In .config/wt.toml
[post-create-command]
"setup" = "uv sync && nvm install"
```

**Delegate to task runners** - Reference existing Justfile/Makefile commands instead of duplicating logic:

```toml
[post-create-command]
"setup" = "just install"

[pre-merge-command]
"validate" = "just test lint"
```

## All Commands

<details>
<summary><strong><code>wt switch [branch]</code></strong> - Switch to existing worktree or create a new one</summary>

```
Usage: wt switch [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>  Branch, path, '@' (HEAD), '-' (previous), or '^' (main)

Options:
  -c, --create             Create a new branch
  -C <path>                Change working directory
  -b, --base <BASE>        Base branch (defaults to default branch)
  -v, --verbose            Show git commands and debug info
  -x, --execute <EXECUTE>  Execute command after switching
  -f, --force              Skip approval prompts
      --no-verify          Skip project hooks
  -h, --help               Print help
```

**BEHAVIOR:**

Switching to Existing Worktree:
  - If worktree exists for branch, changes directory via shell integration
  - No hooks run
  - No branch creation

Creating New Worktree (--create):
  1. Creates new branch (defaults to current default branch as base)
  2. Creates worktree in configured location (default: `../{{ main_worktree }}.{{ branch }}`)
  3. Runs post-create hooks sequentially (blocking)
  4. Shows success message
  5. Spawns post-start hooks in background (non-blocking)
  6. Changes directory to new worktree via shell integration

**HOOKS:**

post-create (sequential, blocking):
  - Run after worktree creation, before success message
  - Typically: npm install, cargo build, setup tasks
  - Failures block the operation
  - Skip with --no-verify

post-start (parallel, background):
  - Spawned after success message shown
  - Typically: dev servers, file watchers, editors
  - Run in background, failures logged but don't block
  - Logs: `.git/wt-logs/{branch}-post-start-{name}.log`
  - Skip with --no-verify

**EXAMPLES:**

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

# Skip hooks during creation
wt switch --create temp --no-verify
```

**SHORTCUTS:**

Use '@' for current HEAD, '-' for previous, '^' for main:
```bash
wt switch @                              # Switch to current branch's worktree
wt switch -                              # Switch to previous worktree
wt switch --create new-feature --base=^  # Branch from main (default)
wt switch --create bugfix --base=@       # Branch from current HEAD
wt remove @                              # Remove current worktree
```

</details>

<details>
<summary><strong><code>wt merge [target]</code></strong> - Merge, push, and cleanup</summary>

```
Usage: wt merge [OPTIONS] [TARGET]

Arguments:
  [TARGET]  Target branch (defaults to default branch)

Options:
  -C <path>         Change working directory
      --no-squash   Skip commit squashing
      --no-commit   Skip commit, squash, and rebase
  -v, --verbose     Show git commands and debug info
      --no-remove   Keep worktree after merge
      --no-verify   Skip project hooks
  -f, --force       Skip approval prompts
      --tracked-only Stage tracked files only
  -h, --help        Print help
```

**LIFECYCLE:**

The merge operation follows a strict order designed for fail-fast execution:

1. **Validate branches**
   Verifies current branch exists (not detached HEAD) and determines target branch
   (defaults to repository's default branch).

2. **Auto-commit uncommitted changes**
   If working tree has uncommitted changes, stages all changes (git add -A) and commits
   with LLM-generated message.

3. **Squash commits (default)**
   By default, counts commits since merge base with target branch. When multiple
   commits exist, squashes them into one with LLM-generated message. Skip squashing
   with --no-squash.

   A safety backup is created before squashing if there are working tree changes.
   Recover with: `git reflog show refs/wt-backup/<branch>`

4. **Rebase onto target**
   Rebases current branch onto target branch. Detects conflicts and aborts if found.
   This fails fast before running expensive checks.

5. **Run pre-merge commands**
   Runs commands from project config's `[pre-merge-command]` after rebase completes.
   These receive `{{ target }}` placeholder for the target branch. Commands run sequentially
   and any failure aborts the merge immediately. Skip with --no-verify.

6. **Push to target**
   Fast-forward pushes to target branch. Rejects non-fast-forward pushes (ensures
   linear history). Temporarily stashes non-conflicting local edits in the target
   worktree so they don't block the push, then restores them after success.

7. **Clean up worktree and branch**
   Removes current worktree, deletes the branch, and switches primary worktree to target
   branch if needed. Skip removal with --no-remove.

**EXAMPLES:**

```bash
# Basic merge to main
wt merge

# Merge without squashing
wt merge --no-squash

# Keep worktree after merging
wt merge --no-remove

# Skip pre-merge commands
wt merge --no-verify
```

</details>

<details>
<summary><strong><code>wt remove [worktree]</code></strong> - Remove worktree and branch</summary>

```
Usage: wt remove [OPTIONS] [WORKTREES]...

Arguments:
  [WORKTREES]...  Worktree or branch (@ for current)

Options:
  -C <path>               Change working directory
      --no-delete-branch  Keep branch after removal
  -D, --force-delete      Delete unmerged branches
  -v, --verbose           Show commands and debug info
      --no-background     Run removal in foreground
  -h, --help              Print help
```

**BEHAVIOR:**

Remove Current Worktree (no arguments):
  - Requires clean working tree (no uncommitted changes)
  - If in worktree: removes it and switches to main worktree
  - If in main worktree: switches to default branch (e.g., main)
  - If already on default branch in main: does nothing

Remove Specific Worktree (by name):
  - Requires target worktree has clean working tree
  - Removes specified worktree(s) and associated branches
  - If removing current worktree, switches to main first
  - Can remove multiple worktrees in one command

Remove Multiple Worktrees:
  - When removing multiple, current worktree is removed last
  - Prevents deleting directory you're currently in
  - Each worktree must have clean working tree

**CLEANUP:**

When removing a worktree (by default):
  1. Validates worktree has no uncommitted changes
  2. Changes directory (if removing current worktree)
  3. Spawns background removal process (non-blocking)
     - Directory deletion happens in background
     - Git worktree metadata removed in background
     - Branch deletion in background (uses git branch -d, safe delete)
     - Logs: `.git/wt-logs/{branch}-remove.log`
  4. Returns immediately so you can continue working
     - Use --no-background for foreground removal (blocking)

**EXAMPLES:**

```bash
# Remove current worktree and branch
wt remove

# Remove specific worktree and branch
wt remove feature-branch

# Remove worktree but keep branch
wt remove --no-delete-branch feature-branch

# Remove multiple worktrees
wt remove old-feature another-branch

# Remove in foreground (blocking)
wt remove --no-background feature-branch

# Switch to default in main
wt remove  # (when already in main worktree)
```

</details>

<details>
<summary><strong><code>wt list</code></strong> - Show all worktrees and branches</summary>

```
Usage: wt list [OPTIONS]

Options:
  -C <path>
          Change working directory

      --format <FORMAT>
          Output format

          Possible values:
          - table: Human-readable table format
          - json:  JSON output

          [default: table]

      --branches
          Include branches without worktrees

  -v, --verbose
          Show git commands and debug info

      --full
          Show CI, conflicts, and full diffs

          Adds columns: CI (pipeline status), main…± (line diffs).
          Enables conflict detection (shows "=" symbol in Status column).
          Requires network requests and git merge-tree operations.

      --progressive
          Show rows progressively (auto-detects TTY)

      --no-progressive
          Disable progressive rendering

  -h, --help
          Print help
```

**COLUMNS:**

- **Branch:** Branch name
- **Status:** Quick status symbols (see STATUS SYMBOLS below)
- **HEAD±:** Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
- **main↕:** Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
- **main…± (--full):** Line diffs in commits ahead of main (+added -deleted)
- **Path:** Worktree directory location
- **Remote⇅:** Commits ahead↑/behind↓ relative to tracking branch (e.g. origin/branch)
- **CI (--full):** CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
  - ● passed (green) - All checks passed
  - ● running (blue) - Checks in progress
  - ● failed (red) - Checks failed
  - ● conflicts (yellow) - Merge conflicts with base
  - ● no-ci (gray) - PR/MR or workflow found but no checks configured
  - (blank) - No PR/MR or workflow found, or gh/glab CLI unavailable
  - (dimmed) - Stale: unpushed local changes differ from PR/MR head
- **Commit:** Short commit hash (8 chars)
- **Age:** Time since last commit (relative)
- **Message:** Last commit message (truncated)

**STATUS SYMBOLS:**

The Status column shows git repository state using compact symbols. Symbol order indicates priority: conflicts (blocking) → worktree state → git operations → branch divergence → working tree changes.

**Symbol order:** `= ≡∅ ↻⋈ ◇⊠⚠ ↑↓ ⇡⇣ ?!+»✘`

| Symbol | Meaning                                                                            | Category           | Dimmed? |
| ------ | ---------------------------------------------------------------------------------- | ------------------ | ------- |
| `·`    | Branch without worktree                                                            | N/A                | No      |
| `=`    | Conflicts with main                                                                | Blocking           | No      |
| `≡`    | Working tree matches main (identical to main branch, regardless of commit history) | Worktree state     | Yes     |
| `∅`    | No commits (no commits ahead AND no uncommitted changes)                           | Worktree state     | Yes     |
| `↻`    | Rebase in progress                                                                 | Git operation      | No      |
| `⋈`    | Merge in progress                                                                  | Git operation      | No      |
| `◇`    | Bare worktree (no working directory)                                               | Worktree attribute | No      |
| `⊠`    | Locked worktree                                                                    | Worktree attribute | No      |
| `⚠`    | Prunable worktree                                                                  | Worktree attribute | No      |
| `↑`    | Commits ahead of main                                                              | Branch divergence  | No      |
| `↓`    | Commits behind main                                                                | Branch divergence  | No      |
| `⇡`    | Commits ahead of remote                                                            | Remote divergence  | No      |
| `⇣`    | Commits behind remote                                                              | Remote divergence  | No      |
| `?`    | Untracked files                                                                    | Working tree       | No      |
| `!`    | Modified files (unstaged)                                                          | Working tree       | No      |
| `+`    | Staged files                                                                       | Working tree       | No      |
| `»`    | Renamed files                                                                      | Working tree       | No      |
| `✘`    | Deleted files                                                                      | Working tree       | No      |

Symbols combine to show complete state (e.g., `≡↓!` means matches main, behind main, and has unstaged changes).

**Dimming logic:** Dimmed rows indicate worktrees with no marginal information beyond main (no unique work). Lines dim when they have either `≡` (matches main) OR `∅` (no commits). Both conditions use OR logic: either is sufficient to dim. This focuses attention on worktrees containing work.

**Branch-only entries:** Branches without worktrees show `·` in the Status column, indicating git status is not applicable (no working directory to check).

</details>

<details>
<summary><strong><code>wt config</code></strong> - Manage configuration</summary>

```
Usage: wt config [OPTIONS] <COMMAND>

Commands:
  init           Initialize global configuration file with examples
  list           List configuration files & locations
  refresh-cache  Refresh default branch from remote
  shell          Configure shell integration
  status         Manage branch status markers

Options:
  -C <path>
          Change working directory

  -v, --verbose
          Show commands and debug info

  -h, --help
          Print help
```

**LLM SETUP GUIDE:**

Enable AI-generated commit messages

1. Install an LLM tool (llm, aichat)

   ```bash
   uv tool install -U llm
   ```

2. Configure a model

   For Claude:
   ```bash
   llm install llm-anthropic
   llm keys set anthropic
   # Paste your API key from: https://console.anthropic.com/settings/keys
   llm models default claude-3.5-sonnet
   ```

   For OpenAI:
   ```bash
   llm keys set openai
   # Paste your API key from: https://platform.openai.com/api-keys
   ```

3. Test it works

   ```bash
   llm "say hello"
   ```

4. Configure worktrunk

   Add to `~/.config/worktrunk/config.toml`:

   ```toml
   [commit-generation]
   command = "llm"
   ```

Use `wt config init` to create the config file if it doesn't exist.
Use `wt config list` to view your current configuration.

Docs: https://llm.datasette.io/ | https://github.com/sigoden/aichat

</details>

<details>
<summary><strong>Beta commands</strong> - Experimental commands for advanced workflows</summary>

These commands are subject to change:

- `wt beta commit` - Commit changes with LLM-generated message
- `wt beta squash [target]` - Squash commits with LLM-generated message
- `wt beta push [target]` - Push changes to target branch (auto-stashes non-conflicting edits)
- `wt beta rebase [target]` - Rebase current branch onto target
- `wt beta ask-approvals` - Approve commands in project config
- `wt beta run-hook <hook-type>` - Run a project hook for testing
- `wt beta select` - Interactive worktree selector (Unix only, WIP)

</details>

## Configuration

```bash
wt config list    # Show all config files and locations
wt config init    # Create global config with examples
wt config --help  # Show LLM setup guide
```

<details>
<summary>Configuration details</summary>

**Global config** (`~/.config/worktrunk/config.toml`):

- `worktree-path` - Path template for new worktrees
- `[commit-generation]` - LLM command and prompt templates
- `[projects."project-id"]` - Per-project approved commands (auto-populated)

**Project config** (`.config/wt.toml` in repository root):

- `[post-create-command]` - Commands after worktree creation
- `[post-start-command]` - Background commands after creation
- `[pre-commit-command]` - Validation before committing
- `[pre-merge-command]` - Validation before merge
- `[post-merge-command]` - Cleanup after merge

---

**Example global config** (`~/.config/worktrunk/config.toml`):

```toml
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]

# Approved commands (auto-populated when approving project hooks)
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
```

**Example project config** (`.config/wt.toml`): See Project Hooks section above.

**Path template defaults:** `../repo.branch/` (siblings to main repo). Available variables: `{{ main_worktree }}`, `{{ branch }}`, `{{ repo }}`.

</details>

## Advanced Features

### Custom Worktree Status

Add emoji status markers to branches that appear in `wt list`. Perfect for tracking work-in-progress states, CI status, or team coordination.

```bash
# Set status for current branch
wt config status set "🤖"

# Or use git config directly
git config worktrunk.status.feature-x "💬"
```

**Status appears in the Status column:**

<!-- Output from: tests/snapshots/integration__integration_tests__list__with_user_status.snap -->

```
Branch             Status  HEAD±  main↕  Path                 Remote⇅  Commit    Age            Message
main                                     ./test-repo                   b834638e  10 months ago  Initial commit
clean-no-status    ≡                     ./clean-no-status             b834638e  10 months ago  Initial commit
clean-with-status  ≡ 💬                  ./clean-with-status           b834638e  10 months ago  Initial commit
dirty-no-status     !      +1 -1         ./dirty-no-status             b834638e  10 months ago  Initial commit
dirty-with-status  ≡?🤖                  ./dirty-with-status           b834638e  10 months ago  Initial commit
```

The custom emoji appears directly after the git status symbols.

<details>
<summary>Automation with Claude Code Hooks</summary>

Claude Code can automatically set/clear emoji status when coding sessions start and end. This shows which branches have active AI sessions.

**Easy setup:** The Worktrunk repository includes a `.claude-plugin` directory with pre-configured hooks. When working in this repository, the hooks are automatically available.

**Manual setup for other repositories:** Copy the hooks from [`.claude-plugin/hooks/hooks.json`](.claude-plugin/hooks/hooks.json) to `~/.claude/settings.json`.

When using Claude:

- Sets status to `🤖` for the current branch when submitting a prompt (working)
- Changes to `💬` when Claude needs input (waiting for permission or idle)
- Clears the status completely when the session ends

**Status from other terminal:**

<!-- Output from: tests/snapshots/integration__integration_tests__list__with_user_status.snap -->

```bash
$ wt list
Branch             Status  HEAD±  main↕  Path                 Remote⇅  Commit    Age            Message
main                                     ./test-repo                   b834638e  10 months ago  Initial commit
clean-no-status    ≡                     ./clean-no-status             b834638e  10 months ago  Initial commit
clean-with-status  ≡ 💬                  ./clean-with-status           b834638e  10 months ago  Initial commit
dirty-no-status     !      +1 -1         ./dirty-no-status             b834638e  10 months ago  Initial commit
dirty-with-status  ≡?🤖                  ./dirty-with-status           b834638e  10 months ago  Initial commit

⚪ Showing 5 worktrees, 1 with changes
```

**How it works:**

- Status is stored as `worktrunk.status.<branch>` in `.git/config`
- Each branch can have its own status emoji
- The hooks automatically detect the current branch and set/clear its status
- Status is shared across all worktrees on the same branch (by design)
- Works with any git repository, no special configuration needed

</details>

</details>

### Custom Worktree Paths

By default, worktrees live as siblings to the main repo:

```
myapp/               # main worktree
myapp.feature-x/     # secondary worktree
myapp.bugfix-y/      # secondary worktree
```

Customize the pattern in `~/.config/worktrunk/config.toml`:

```toml
# Inside the repo (keeps everything contained)
worktree-path = ".worktrees/{{ branch }}"

# Shared directory with multiple repos
worktree-path = "../worktrees/{{ main_worktree }}/{{ branch }}"
```

### Shell Integration

Worktrunk can automatically configure the shell:

```bash
wt config shell
```

This adds shell integration to config files (supports Bash, Zsh, and Fish). The
integration enables `wt switch` to change directories and `wt remove` to return
to the previous location.

For manual setup instructions, see `wt config shell --help`.

## Status

Worktrunk is in active development. The core features are stable and ready for use. While the project is pre-1.0, the CLI interface and major features are unlikely to change significantly.

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

## FAQ

### What commands does Worktrunk execute?

Worktrunk executes commands in three contexts:

1. **Project hooks** (`.config/wt.toml`) - Automation for worktree lifecycle
2. **LLM commands** (`~/.config/worktrunk/config.toml`) - Commit message generation
3. **--execute flag** - Commands provided explicitly

Commands from project hooks and LLM configuration require approval on first run. Approved commands are saved to `~/.config/worktrunk/config.toml` under the project's configuration. If a command changes, worktrunk requires new approval.

**Example approval prompt:**

<!-- Output from: tests/integration_tests/snapshots/integration__integration_tests__approval_pty__approval_prompt_named_commands.snap -->

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

Use `--force` to bypass prompts (useful for CI/automation).

### Installation fails with C compilation errors

If you encounter errors related to tree-sitter or C compilation (like "error: 'for' loop initial declarations are only allowed in C99 mode" or "undefined reference to le16toh"), install without syntax highlighting:

```bash
cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.
