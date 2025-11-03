# Worktrunk

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Git worktree lifecycle automation. Built for running multiple AI coding agents without conflicts.

<!-- Add after publishing to crates.io:
[![Crates.io](https://img.shields.io/crates/v/worktrunk.svg)](https://crates.io/crates/worktrunk)
-->

Git worktrees let multiple agents work on one repo without colliding—each gets a separate directory sharing history. But creating worktrees, tracking paths, and cleaning up afterward is manual. Worktrunk automates the lifecycle: create, switch, clean up—your shell stays put.

Running ten agents on different features? `wt switch --create feature-a`, `wt switch --create feature-b`, and they're isolated. Agent finishes? `wt remove feature-a` cleans up automatically. No path juggling, no stale directories.

## What It Does

Automates the full lifecycle: create worktree, work, merge back, remove worktree.

<!-- Output generated from: tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap -->
```bash
$ wt switch --create fix-auth
# Shell now in ../repo.fix-auth/

# Agent works, makes changes, then:
$ wt merge
🔄 Merging 1 commit to main @ a1b2c3d (no squashing needed, no rebasing needed)

* a1b2c3d (HEAD -> fix-auth) Implement JWT validation

 auth.rs | 1 +
 1 file changed, 1 insertion(+)

✅ Merged to main (1 commit, 1 file, +1)
🔄 Cleaning up worktree...
✅ Returned to primary at ../repo/
# Shell back in main
```

Shell integration means directories change automatically. Merge handles staging, committing, merging, pushing, cleanup. One command.

## Installation

```bash
cargo install worktrunk
wt config shell  # Sets up shell integration
```

## Three Commands

**Create workspace:**
```bash
wt switch --create feature-name
```

**Finish and merge:**
```bash
wt merge
```

**See active worktrees:**
```bash
wt list
```

## Automation Features

**LLM commits** - AI generates merge commits from diff and history:
```bash
wt merge
wt config help  # Setup guide
```

**Project hooks** - Auto-run tests, install deps:
```toml
# .config/wt.toml
[pre-merge-command]
"test" = "npm test"
```

**Shell integration** - Bash, Zsh, Fish, Nushell, PowerShell, Elvish, Xonsh, Oil.

## Design Philosophy

Worktrunk is opinionated. The choices optimize for AI agent workflows:

1. **Merge does everything** - Staging, committing all changes, merging, pushing, cleanup in one command
2. **Squash by default** - Linear history, configurable
3. **Automatic shell navigation** - No manual `cd` commands
4. **Fail-fast hooks** - Tests block bad merges

These trade manual control for automation. For fine-grained control, use `git worktree` directly.

## All Commands

- `wt switch [branch]` - Switch to existing worktree
- `wt switch --create [branch]` - Create and switch
- `wt remove` - Remove current, return to main
- `wt merge [target]` - Merge, push, cleanup
- `wt list` - Show all worktrees
- `wt config` - Manage configuration

See `wt --help` for details.

<details>
<summary>Worktree states in <code>wt list</code></summary>

The State column shows worktree status. **Dimmed rows** indicate worktrees with no marginal information beyond main (no unique work).

**Dimming logic:** Lines dim when they provide no marginal information - either no commits ahead OR working tree matches main exactly. This focuses attention on worktrees containing work.

| State | Meaning | Dimmed? |
|-------|---------|---------|
| **no commits** | No commits on top of main AND no uncommitted changes (`ahead == 0` and `working_tree_diff == (0, 0)`). | Yes |
| **matches main** | Working tree contents identical to main branch, regardless of commit history (`working_tree_diff_with_main == (0, 0)`). | Yes |
| **conflicts** | Merge conflicts detected with main | No |
| **[MERGING]**, **[REBASING]**, etc. | Git operation in progress | No |
| **bare** | Bare worktree (no working directory) | No |
| **locked**, **prunable** | Git worktree management states | No |

Both dimming conditions use OR logic: either is sufficient to dim. "no commits" means clean worktree with no commits ahead. "matches main" means the current working tree state is identical to main, even if commit history differs (e.g., commits were made but later reverted).

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
worktree-path = "../{main-worktree}.{branch}"

[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

Project config at `.config/wt.toml` in the repository root (see Project Automation above).

Worktree path defaults: `../repo.branch/` (siblings to main repo). Variables: `{main-worktree}`, `{branch}`, `{repo}`.

</details>

## Advanced Features

### LLM-Powered Commit Messages

During merge operations, worktrunk can generate commit messages using an LLM. The LLM analyzes the staged diff and recent commit history to write messages matching the project's style.

```bash
# Merge with LLM-generated commit message (squashes by default)
$ wt merge

# Merge without squashing commits
$ wt merge --no-squash

# Merge to a specific target branch
$ wt merge staging
```

Set up LLM integration: `wt config help` shows the setup guide, `wt config init` creates example config.

<details>
<summary>Manual configuration</summary>

Edit `~/.config/worktrunk/config.toml`:

```toml
[commit-generation]
command = "llm"  # or "claude", "gpt", etc.
args = ["-m", "claude-haiku-4-5-20251001"]
```

If the LLM is unavailable or fails, worktrunk falls back to a deterministic message.

</details>

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

### Project Automation

Automate common tasks by creating `.config/wt.toml` in your repository root. Run tests before merging, install dependencies when creating worktrees, start dev servers automatically.

```toml
# Install deps when creating a worktree
[post-create-command]
"install" = "npm install --frozen-lockfile"

# Start dev server automatically
[post-start-command]
"dev" = "npm run dev"

# Run tests before merging
[pre-merge-command]
"test" = "npm test"
"lint" = "npm run lint"
```

<details>
<summary>All available hooks</summary>

| Hook | When It Runs | Execution | Failure Behavior |
|------|--------------|-----------|------------------|
| **post-create-command** | After `git worktree add` completes | Sequential, blocking | Logs warning, continues with remaining commands |
| **post-start-command** | After post-create completes | Parallel, non-blocking (background processes) | Logs warning, doesn't affect switch result |
| **pre-commit-command** | Before committing changes during `wt merge` (both squash and no-squash modes) | Sequential, blocking, fail-fast | Terminates merge immediately |
| **pre-merge-command** | After rebase completes during `wt merge` (validates rebased state before push) | Sequential, blocking, fail-fast | Terminates merge immediately |
| **post-merge-command** | After successful merge and push to target branch, before cleanup | Sequential, blocking | Logs warning, continues with remaining commands |

**Template variables:** `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`

**Skipping hooks:** `wt switch --no-verify` or `wt merge --no-verify`

**Security:** Commands require approval on first run. Use `--force` to bypass.

**Example output with hooks:**

<!-- Output generated from: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap -->

```bash
$ wt merge
🔄 Squashing 3 commits into 1 (3 files, +3)...
🔄 Generating squash commit message...
  feat(auth): Implement JWT authentication system

  Add comprehensive JWT token handling including validation, refresh logic,
  and authentication tests. This establishes the foundation for secure
  API authentication.

  - Implement token refresh mechanism with expiry handling
  - Add JWT encoding/decoding with signature verification
  - Create test suite covering all authentication flows

✅ Squashed @ a1b2c3d
🔄 Running pre-merge: lint
  cargo clippy

    Checking worktrunk v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.23s

🔄 Running pre-merge: test
  cargo test

    Finished test [unoptimized + debuginfo] target(s) in 0.12s
     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)

running 18 tests
test auth::tests::test_jwt_decode ... ok
test auth::tests::test_jwt_encode ... ok
test auth::tests::test_token_refresh ... ok
test auth::tests::test_token_validation ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s

🔄 Merging 1 commit to main @ a1b2c3d (no rebasing needed)

* a1b2c3d (HEAD -> feature-auth) feat(auth): Implement JWT authentication system

 auth.rs      | 1 +
 auth_test.rs | 1 +
 jwt.rs       | 1 +
 3 files changed, 3 insertions(+)

✅ Merged to main (1 commit, 3 files, +3)
🔄 Cleaning up worktree...
✅ Returned to primary at ../repo/
🔄 Running post-merge: install
  cargo install --path .

  Installing worktrunk v0.1.0
   Compiling worktrunk v0.1.0
    Finished release [optimized] target(s) in 2.34s
  Installing ~/.cargo/bin/wt
   Installed package `worktrunk v0.1.0` (executable `wt`)
```

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
worktree-path = ".worktrees/{branch}"

# Shared directory with multiple repos
worktree-path = "../worktrees/{main-worktree}/{branch}"
```

### Shell Integration Details

Worktrunk automatically configures your shell:

```bash
wt config shell
```

This adds shell integration to your config files (supports Bash, Zsh, Fish, Nushell, PowerShell, Elvish, Xonsh, Oil). The integration enables `wt switch` to change directories and `wt remove` to return to the previous location.

<details>
<summary>Manual setup (if you prefer)</summary>

Add one line to your shell config:

**Bash** (`~/.bashrc`):
```bash
eval "$(wt init bash)"
```

**Fish** (`~/.config/fish/config.fish`):
```fish
wt init fish | source
```

**Zsh** (`~/.zshrc`):
```bash
eval "$(wt init zsh)"
```

**Nushell** (`~/.config/nushell/env.nu`):
```nu
wt init nushell | save -f ~/.cache/wt-init.nu
```

Then add to `~/.config/nushell/config.nu`:
```nu
source ~/.cache/wt-init.nu
```

**PowerShell** (profile):
```powershell
wt init powershell | Out-String | Invoke-Expression
```

**Elvish** (`~/.config/elvish/rc.elv`):
```elvish
eval (wt init elvish | slurp)
```

**Xonsh** (`~/.xonshrc`):
```python
execx($(wt init xonsh))
```

**Oil Shell** (`~/.config/oil/oshrc`):
```bash
eval "$(wt init oil)"
```

</details>

## Status

Worktrunk is in active development. The core features are stable and ready for use. While the project is pre-1.0, the CLI interface and major features are unlikely to change significantly.
