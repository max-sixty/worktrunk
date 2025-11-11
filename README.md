# Worktrunk

<!-- User badges -->

[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![GitHub CI Status](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yml?event=push&branch=main&logo=github&style=for-the-badge)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

Worktrunk is a CLI tool to make working with git worktrees much much easier.
It's designed for those running concurrent AI coding agents.

Git worktrees let multiple agents work on one repo without colliding; each agent
gets a separate directory. But creating worktrees, tracking paths, and cleaning
up afterward is manual. Worktrunk automates that lifecycle.

## Quick Start

Worktrunk automates the full worktree lifecycle: create worktree, work, merge back, remove worktree.

<!-- Output generated from:
  Switch: tests/snapshots/integration__integration_tests__merge__readme_example_simple_switch.snap
  Merge: tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap
  (snapshot output includes [SHA]/[REPO]; replace with representative values when copying into README) -->

**Create a worktree:**

```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth/
```

**Work on your changes, then merge back:**

```bash
$ wt merge
🔄 Merging 1 commit to main @ a1b2c3d (no commit/squash/rebase needed)

   * a1b2c3d (HEAD -> fix-auth) Implement JWT validation
    auth.rs | 13 +++++++++++++
    1 file changed, 13 insertions(+)
✅ Merged to main (1 commit, 1 file, +13)
🔄 Removing worktree & branch...
✅ Removed worktree & branch for fix-auth, returned to primary at ../repo/
# Shell back in main
```

**See all active worktrees:**

```bash
$ wt list
Branch     Status  HEAD±    main↕  Path
main                               ./myapp/
feature-x  ↑!      +5 -2    ↑3     ./myapp.feature-x/
bugfix-y   ↑       +0 -0    ↑1     ./myapp.bugfix-y/
```

## Installation

```bash
cargo install worktrunk
wt config shell  # Sets up shell integration
```

## Tips

**Create an alias for your favorite agent** - Shell aliases streamline common workflows. For example, to create a worktree and immediately start Claude:

```bash
alias wsl='wt switch --create --execute=claude'
```

Now `wsl new-feature` creates a branch, sets up the worktree, runs initialization hooks, and launches Claude in that directory.

**Automatic branch status in Claude Code** - The Claude Code integration shows which branches have active AI sessions. When Claude starts working, the branch shows `🤖` in `wt list`. When waiting for input, it shows `💬`. Setup instructions: [Custom Worktree Status](#custom-worktree-status).

**Auto-generated commit messages** - Simon Willison's [llm](https://llm.datasette.io/) tool integrates seamlessly with worktrunk's commit generation. Install it, configure the command, and `wt merge` will automatically generate contextual commit messages. Setup guide: [LLM-Powered Commit Messages](#llm-powered-commit-messages).

**Environment setup with hooks** - Each worktree is a separate directory. Use `post-create-command` to ensure consistent environments:

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

## Automation Features

### LLM-Powered Commit Messages

During merge operations, worktrunk can generate commit messages using an LLM. The LLM analyzes the staged diff and recent commit history to write messages matching the project's style.

<!-- Config and output from: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap (uses mock llm) -->

Add to `~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

Then `wt merge` will generate commit messages automatically:

```bash
$ wt merge
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

Set up LLM integration: `wt config help` shows the setup guide, `wt config init` creates example config.

<details>
<summary><b>Advanced: Custom Prompt Templates</b></summary>

Worktrunk uses [minijinja templates](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) for commit message prompts, giving you full control over what the LLM sees.

**Inline template for normal commits:**

```toml
[commit-generation]
command = "llm"
args = ["-s"]
template = """
Generate a commit message for {{ repo | upper }}.

Branch: {{ branch }}
{%- if recent_commits %}

Recent commit style ({{ recent_commits | length }} commits):
{%- for commit in recent_commits %}
  {{ loop.index }}. {{ commit }}
{%- endfor %}
{%- endif %}

Changes to commit:
```

{{ git_diff }}

```

Requirements:
- Follow the style of recent commits above
- First line under 50 chars
- Focus on WHY, not HOW
"""
```

**Inline template for squash commits:**

```toml
[commit-generation]
command = "llm"
squash-template = """
Squashing {{ commits | length }} commit(s) from {{ branch }} to {{ target_branch }}.

{% if commits | length > 1 -%}
Commits being combined:
{%- for c in commits %}
  {{ loop.index }}/{{ loop.length }}: {{ c }}
{%- endfor %}
{%- else -%}
Single commit: {{ commits[0] }}
{%- endif %}

Generate one cohesive commit message that captures the overall change.
Use conventional commit format (feat/fix/docs/refactor).
"""
```

**External template files:**

```toml
[commit-generation]
command = "claude"
template-file = "~/.config/worktrunk/commit-template.jinja"
squash-template-file = "~/.config/worktrunk/squash-template.jinja"
```

**Available template variables:**

Normal commits:

- `{{ git_diff }}` - Staged changes
- `{{ branch }}` - Current branch name
- `{{ recent_commits }}` - Array of recent commit messages (for style matching)
- `{{ repo }}` - Repository name

Squash commits:

- `{{ commits }}` - Array of commit messages being squashed
- `{{ target_branch }}` - Branch being merged into (e.g., "main")
- `{{ branch }}` - Current branch name
- `{{ repo }}` - Repository name

See the [minijinja template documentation](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) for complete syntax reference (filters, conditionals, loops, whitespace control, etc.).

</details>

### Project Hooks

Automate common tasks by creating `.config/wt.toml` in your repository root. Run tests before merging, install dependencies when creating worktrees, start dev servers automatically.

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

```bash
$ wt switch --create feature-x
🔄 Creating worktree for feature-x...
✅ Created worktree, changed directory to ../repo.feature-x/
🔄 Running post-create install:
  uv sync

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s

🔄 Running post-start dev (background):
  uv run dev

  Starting dev server on http://localhost:3000...
```

**Example: Merging with pre-merge hooks:**

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
  collected 18 items

  tests/test_auth.py::test_login_success PASSED                            [ 11%]
  tests/test_auth.py::test_login_invalid_password PASSED                   [ 22%]
  tests/test_auth.py::test_token_refresh PASSED                            [ 33%]
  tests/test_auth.py::test_token_validation PASSED                         [ 44%]

  ============================== 18 passed in 0.8s ===============================

🔄 Running pre-merge lint:
  uv run ruff check

  All checks passed!

🔄 Merging 1 commit to main @ a1b2c3d (no rebase needed)

  * a1b2c3d (HEAD -> feature-auth) feat(api): Add user authentication endpoints

   api/auth.py  | 32 ++++++++++++++++++++++++++++++++
   tests/test_auth.py | 13 +++++++++++++
   2 files changed, 45 insertions(+)

✅ Merged to main (1 commit, 2 files, +45)
🔄 Removing worktree & branch...
✅ Removed worktree & branch for feature-auth, returned to primary at ../repo/
```

<details>
<summary>All available hooks</summary>

| Hook                    | When It Runs                                                                   | Execution                                     | Failure Behavior                                |
| ----------------------- | ------------------------------------------------------------------------------ | --------------------------------------------- | ----------------------------------------------- |
| **post-create-command** | After `git worktree add` completes                                             | Sequential, blocking                          | Logs warning, continues with remaining commands |
| **post-start-command**  | After post-create completes                                                    | Parallel, non-blocking (background processes) | Logs warning, doesn't affect switch result      |
| **pre-commit-command**  | Before committing changes during `wt merge` (both squash and no-squash modes)  | Sequential, blocking, fail-fast               | Terminates merge immediately                    |
| **pre-merge-command**   | After rebase completes during `wt merge` (validates rebased state before push) | Sequential, blocking, fail-fast               | Terminates merge immediately                    |
| **post-merge-command**  | After successful merge and push to target branch, before cleanup               | Sequential, blocking                          | Logs warning, continues with remaining commands |

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`

**Skipping hooks:** `wt switch --no-verify` or `wt merge --no-verify`

**Security:** Commands require approval on first run. Use `--force` to bypass.

</details>

## Design Philosophy

Worktrunk is opinionated! It's not designed to be all things to all people. The choices optimize for agent workflows:

- Lots of short-lived worktrees
- CLI-based Agents
- Local inner dev loops
- Navigated using the shell
- Commits are squashed, linear histories
- Maximum automation

Standard `git worktree` commands continue working fine — adopting Worktrunk for a portion of a workflow doesn't require adopting it for everything.

## All Commands

- `wt switch [branch]` - Switch to existing worktree
- `wt switch --create [branch]` - Create and switch (supports `--base=@` to branch from current HEAD)
- `wt remove [branch]` - Remove worktree (use `@` for current)
- `wt merge [target]` - Merge, push, cleanup
- `wt list` - Show all worktrees
- `wt config` - Manage configuration
- `wt beta` - Development and testing utilities (see below)

See `wt --help` for details.

<details>
<summary>Beta commands (<code>wt beta</code>)</summary>

Experimental commands for advanced workflows. These are subject to change.

- `wt beta commit` - Commit changes with LLM-generated message
- `wt beta squash [target]` - Squash commits with LLM-generated message
- `wt beta push [target]` - Push changes to target branch (auto-stashes non-conflicting edits)
- `wt beta rebase [target]` - Rebase current branch onto target
- `wt beta ask-approvals` - Approve commands in project config
- `wt beta run-hook <hook-type>` - Run a project hook for testing
- `wt beta select` - Interactive worktree selector (Unix only)

**Note:** Beta commands may have breaking changes between releases.

</details>

<details>
<summary>Status column symbols in <code>wt list</code></summary>

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

**Dimming logic:** **Dimmed rows** indicate worktrees with no marginal information beyond main (no unique work). Lines dim when they have either `≡` (matches main) OR `∅` (no commits). Both conditions use OR logic: either is sufficient to dim. This focuses attention on worktrees containing work.

**Branch-only entries:** Branches without worktrees show `·` in the Status column, indicating git status is not applicable (no working directory to check).

</details>

## Configuration

```bash
wt config list  # Show all config files and locations
wt config init  # Create global config with examples
wt config help  # Show LLM setup guide
```

<details>
<summary>Configuration details</summary>

Global config at `~/.config/worktrunk/config.toml`:

```toml
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

Project config at `.config/wt.toml` in the repository root (see Project Hooks above).

Worktree path defaults: `../repo.branch/` (siblings to main repo). Variables: `{{ main_worktree }}`, `{{ branch }}`, `{{ repo }}`.

</details>

## Advanced Features

### Custom Worktree Status

Add emoji status markers to worktrees that appear in `wt list`. Perfect for tracking work-in-progress states, CI status, or team coordination.

**Set status manually:**

```bash
# Set an emoji status for a branch (works everywhere)
git config worktrunk.status.feature-x "🚧"

# Clear the status
git config --unset worktrunk.status.feature-x
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
<summary><b>Automation with Claude Code Hooks</b></summary>

Claude Code can automatically set/clear emoji status when coding sessions start and end. This shows which branches have active AI sessions.

**Easy setup:** The Worktrunk repository includes a `.claude-plugin` directory with pre-configured hooks. If you're working in this repository, the hooks are automatically available.

**Manual setup for other repositories:** Copy the hooks from [`.claude-plugin/hooks/hooks.json`](.claude-plugin/hooks/hooks.json) to your `~/.claude/settings.json`.

Now when you use Claude:

- Sets status to `🤖` for the current branch when you submit a prompt (working)
- Changes to `💬` when Claude needs your input (waiting for permission or idle)
- Clears the status completely when the session ends

**Status from other terminal:**

<!-- from tests/snapshots/integration__integration_tests__list__with_user_status.snap -->

```bash
$ wt list
Branch              Status  HEAD±  main↕  Path
main                                      ./myapp/
dirty-with-status   ≡?🤖                  ./myapp.dirty-with-status/
```

**How it works:**

- Status is stored as `worktrunk.status.<branch>` in `.git/config`
- Each branch can have its own status emoji
- The hooks automatically detect the current branch and set/clear its status
- Status is shared across all worktrees on the same branch (by design)
- Works with any git repository, no special configuration needed

<details>
<summary><b>Alternative: Per-Worktree Status (Advanced)</b></summary>

For true per-worktree isolation (different status for multiple worktrees on the same branch), use worktree-specific config:

**One-time setup (enables per-worktree config for the repo):**

```bash
git config extensions.worktreeConfig true
```

**Set status from within a worktree:**

```bash
# From within the worktree
git config --worktree worktrunk.status "🚧"

# Clear status
git config --worktree --unset worktrunk.status
```

**Claude Code hooks for per-worktree:**

Copy the hooks from [`.claude-plugin/hooks/hooks.worktree.json`](.claude-plugin/hooks/hooks.worktree.json) to your `~/.claude/settings.json`.

**Priority:** Worktree-specific config takes precedence over branch-keyed config when both exist.

</details>

</details>

### Worktree Paths

By default, worktrees live as siblings to the main repo:

```
myapp/               # primary worktree
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

Worktrunk can automatically configure your shell:

```bash
wt config shell
```

This adds shell integration to your config files (supports Bash, Zsh, Fish, Nushell, PowerShell, Elvish, Xonsh, Oil). The integration enables `wt switch` to change directories and `wt remove` to return to the previous location.

For manual setup instructions, see `wt config shell --help`.

## Status

Worktrunk is in active development. The core features are stable and ready for use. While the project is pre-1.0, the CLI interface and major features are unlikely to change significantly.

<details>
<summary><b>Developing</b></summary>

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

### Installation fails with C compilation errors

If you encounter errors related to tree-sitter or C compilation (like "error: 'for' loop initial declarations are only allowed in C99 mode" or "undefined reference to le16toh"), install without syntax highlighting:

```bash
cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.
