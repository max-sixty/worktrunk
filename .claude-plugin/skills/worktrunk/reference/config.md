# wt config

Manage user & project configs. Includes shell integration, hooks, and saved state.

## Examples

Install shell integration (required for directory switching):

```bash
wt config shell install
```

Create user config file with documented examples:

```bash
wt config create
```

Create project config file (`.config/wt.toml`) for hooks:

```bash
wt config create --project
```

Show current configuration and file locations:

```bash
wt config show
```

## Configuration files

| File | Location | Contains | Committed & shared |
|------|----------|----------|--------------------|
| **User config** | `~/.config/worktrunk/config.toml` | Worktree path template, LLM commit configs, etc | ✗ |
| **Project config** | `.config/wt.toml` | Project hooks, dev server URL | ✓ |

**User config** — personal preferences:

```toml
# ~/.config/worktrunk/config.toml
worktree-path = ".worktrees/{{ branch | sanitize }}"

[commit.generation]
command = "MAX_THINKING_TOKENS=0 claude -p --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''"
```

**Project config** — shared team settings:

```toml
# .config/wt.toml
[post-create]
deps = "npm ci"

[pre-merge]
test = "npm test"
```

---

<!-- USER_CONFIG_START -->
# User Configuration

Create with `wt config create`.

Location:

- macOS/Linux: `~/.config/worktrunk/config.toml` (or `$XDG_CONFIG_HOME` if set)
- Windows: `%APPDATA%\worktrunk\config.toml`

## Worktree path template

Controls where new worktrees are created.

**Variables:**

- `{{ repo_path }}` — absolute path to the repository (e.g., `/Users/me/code/myproject`)
- `{{ repo }}` — repository directory name (e.g., `myproject`)
- `{{ branch }}` — raw branch name (e.g., `feature/auth`)
- `{{ branch | sanitize }}` — filesystem-safe: `/` and `\` become `-` (e.g., `feature-auth`)
- `{{ branch | sanitize_db }}` — database-safe: lowercase, underscores, hash suffix (e.g., `feature_auth_x7k`)

**Examples** for repo at `~/code/myproject`, branch `feature/auth`:

```toml
# Default — sibling directory
# Creates: ~/code/myproject.feature-auth
# worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"

# Inside the repository
# Creates: ~/code/myproject/.worktrees/feature-auth
worktree-path = "{{ repo_path }}/.worktrees/{{ branch | sanitize }}"

# Centralized worktrees directory
# Creates: ~/worktrees/myproject/feature-auth
worktree-path = "~/worktrees/{{ repo }}/{{ branch | sanitize }}"
```

`~` expands to the home directory. Relative paths are relative to the repository root.

## LLM commit messages

Generate commit messages automatically during merge. Requires an external CLI tool.

See [LLM commits docs](https://worktrunk.dev/llm-commits/) for setup and [Custom prompt templates](#custom-prompt-templates) for template customization.

## Command config

### List

Persistent flag values for `wt list`. Override on command line as needed.

```toml
[list]
full = false       # Show CI status and main…± diffstat columns (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote-only branches (--remotes)
```

### Commit

Shared by `wt step commit`, `wt step squash`, and `wt merge`.

```toml
[commit]
stage = "all"      # What to stage before commit: "all", "tracked", or "none"
```

### Merge

All flags are on by default. Set to false to change default behavior.

```toml
[merge]
squash = true      # Squash commits into one (--no-squash to preserve history)
commit = true      # Commit uncommitted changes first (--no-commit to skip)
rebase = true      # Rebase onto target before merge (--no-rebase to skip)
remove = true      # Remove worktree after merge (--no-remove to keep)
verify = true      # Run project hooks (--no-verify to skip)
```

### Select

Pager behavior for `wt select` diff previews.

```toml
[select]
# Pager command with flags for diff preview (overrides git's core.pager)
# Use this to specify pager flags needed for non-TTY contexts
# Example:
# pager = "delta --paging=never"
```

### User project-specific settings

For context:

- [Project config](https://worktrunk.dev/config/#project-configuration) settings are shared with teammates.
- User configs generally apply to all projects.
- User configs _also_ has a `[projects]` table which holds project-specific settings for the user, such as approved hook commands and worktree layout. That's what this section covers.

Entries are keyed by project identifier (e.g., `github.com/user/repo`).

#### Approved hook commands

When a project hook runs for the first time, Worktrunk asks for approval. Approved commands are saved here, preventing repeated prompts.

```toml
[projects."github.com/user/repo"]
approved-commands = ["npm ci", "npm test"]
```

To reset, delete the entry or run `wt hook approvals clear`.

#### Setting overrides (Experimental)

Override global user config for a specific project. Scalar values (like `worktree-path`) replace the global value. Hooks append — both global and per-project hooks run.

```toml
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
list.full = true
merge.squash = false
post-create.env = "cp .env.example .env"
```

### Custom prompt templates

Templates use [minijinja](https://docs.rs/minijinja/) syntax.

#### Commit template

Available variables:

- `{{ git_diff }}`, `{{ git_diff_stat }}` — diff content
- `{{ branch }}`, `{{ repo }}` — context
- `{{ recent_commits }}` — recent commit messages

Default template:

<!-- DEFAULT_TEMPLATE_START -->
```toml
[commit.generation]
template = """
Write a commit message for the staged changes below.

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match recent commit style (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>

<context>
Branch: {{ branch }}
{% if recent_commits %}<recent_commits>
{% for commit in recent_commits %}- {{ commit }}
{% endfor %}</recent_commits>{% endif %}
</context>

"""
```
<!-- DEFAULT_TEMPLATE_END -->

#### Squash template

Available variables (in addition to commit template variables):

- `{{ commits }}` — list of commits being squashed
- `{{ target_branch }}` — merge target branch

Default template:

<!-- DEFAULT_SQUASH_TEMPLATE_START -->
```toml
[commit.generation]
squash-template = """
Combine these commits into a single commit message.

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match the style of commits being squashed (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for commit in commits %}- {{ commit }}
{% endfor %}</commits>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>

"""
```
<!-- DEFAULT_SQUASH_TEMPLATE_END -->
<!-- USER_CONFIG_END -->

---

# Project Configuration

Project config (`.config/wt.toml`) defines lifecycle hooks and project-specific settings. This file is checked into version control and shared with the team. Create with `wt config create --project`.

See [`wt hook`](https://worktrunk.dev/hook/) for hook types, execution order, template variables, and examples.

### Non-hook settings

```toml
# .config/wt.toml

# URL column in wt list (dimmed when port not listening)
[list]
url = "http://localhost:{{ branch | hash_port }}"

# Override CI platform detection for self-hosted instances
[ci]
platform = "github"  # or "gitlab"
```

---

# Shell Integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

```bash
wt config shell install
```

Or manually add to the shell config:

```bash
# For bash: add to ~/.bashrc
eval "$(wt config shell init bash)"

# For zsh: add to ~/.zshrc
eval "$(wt config shell init zsh)"

# For fish: add to ~/.config/fish/config.fish
wt config shell init fish | source
```

Without shell integration, `wt switch` prints the target directory but cannot `cd` into it.

### First-run prompts

On first run without shell integration, Worktrunk offers to install it. Similarly, on first commit without LLM configuration, it offers to configure a detected tool (`claude`, `codex`). Declining sets `skip-shell-integration-prompt` or `skip-commit-generation-prompt` automatically.

# Other

## Environment variables

All user config options can be overridden with environment variables using the `WORKTRUNK_` prefix.

### Naming convention

Config keys use kebab-case (`worktree-path`), while env vars use SCREAMING_SNAKE_CASE (`WORKTRUNK_WORKTREE_PATH`). The conversion happens automatically.

For nested config sections, use double underscores to separate levels:

| Config | Environment Variable |
|--------|---------------------|
| `worktree-path` | `WORKTRUNK_WORKTREE_PATH` |
| `commit.generation.command` | `WORKTRUNK_COMMIT__GENERATION__COMMAND` |
| `commit.stage` | `WORKTRUNK_COMMIT__STAGE` |

Note the single underscore after `WORKTRUNK` and double underscores between nested keys.

### Example: CI/testing override

Override the LLM command in CI to use a mock:

```bash
WORKTRUNK_COMMIT__GENERATION__COMMAND="echo 'test: automated commit'" wt merge
```

### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers (useful for testing dev builds) |
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_DIRECTIVE_FILE` | Internal: set by shell wrappers to enable directory changes |
| `WORKTRUNK_SHELL` | Internal: set by shell wrappers to indicate shell type (e.g., `powershell`) |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting file descriptor limits. |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

## Command reference

wt config - Manage user &amp; project configs

Includes shell integration, hooks, and saved state.

Usage: <b><span class=c>wt config</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>shell</span></b>   Shell integration setup
  <b><span class=c>create</span></b>  Create configuration file
  <b><span class=c>show</span></b>    Show configuration files &amp; locations
  <b><span class=c>state</span></b>   Manage internal data and cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

# Subcommands

## wt config show

Show configuration files & locations.

Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`).

If a config file doesn't exist, shows defaults that would be used.

### Full diagnostics

Use `--full` to run diagnostic checks:

```bash
wt config show --full
```

This tests:
- **CI tool status** — Whether `gh` (GitHub) or `glab` (GitLab) is installed and authenticated
- **Commit generation** — Whether the LLM command can generate commit messages

### Command reference

wt config show - Show configuration files &amp; locations

Usage: <b><span class=c>wt config show</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--full</span></b>
          Run diagnostic checks (CI tools, commit generation)

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

## wt config state

Manage internal data and cache.

State is stored in `.git/` (config entries and log files), separate from configuration files.
Use `wt config show` to view file-based configuration.

### Keys

- **default-branch**: The repository's default branch (`main`, `master`, etc.)
- **previous-branch**: Previous branch for `wt switch -`
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, no-ci, error)
- **marker**: Custom status marker for a branch (shown in `wt list`)
- **logs**: Background operation logs

### Examples

Get the default branch:
```bash
wt config state default-branch
```

Set the default branch manually:
```bash
wt config state default-branch set main
```

Set a marker for current branch:
```bash
wt config state marker set "🚧 WIP"
```

Clear all CI status cache:
```bash
wt config state ci-status clear --all
```

Show all stored state:
```bash
wt config state get
```

Clear all stored state:
```bash
wt config state clear
```

### Command reference

wt config state - Manage internal data and cache

Usage: <b><span class=c>wt config state</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>default-branch</span></b>   Default branch detection and override
  <b><span class=c>previous-branch</span></b>  Previous branch (for <b>wt switch -</b>)
  <b><span class=c>ci-status</span></b>        CI status cache
  <b><span class=c>marker</span></b>           Branch markers
  <b><span class=c>logs</span></b>             Background operation logs
  <b><span class=c>hints</span></b>            One-time hints shown in this repo
  <b><span class=c>get</span></b>              Get all stored state
  <b><span class=c>clear</span></b>            Clear all stored state

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

## wt config state default-branch

Default branch detection and override.

Useful in scripts to avoid hardcoding `main` or `master`:

```bash
git rebase $(wt config state default-branch)
```

Without a subcommand, runs `get`. Use `set` to override, or `clear` then `get` to re-detect.

### Detection

Worktrunk detects the default branch automatically:

1. **Worktrunk cache** — Checks `git config worktrunk.default-branch` (single command)
2. **Git cache** — Detects primary remote and checks its HEAD (e.g., `origin/HEAD`)
3. **Remote query** — If not cached, queries `git ls-remote` (100ms–2s)
4. **Local inference** — If no remote, infers from local branches

Once detected, the result is cached in `worktrunk.default-branch` for fast access.

The local inference fallback uses these heuristics in order:
- If only one local branch exists, uses it
- For bare repos or empty repos, checks `symbolic-ref HEAD`
- Checks `git config init.defaultBranch`
- Looks for common names: `main`, `master`, `develop`, `trunk`

### Command reference

wt config state default-branch - Default branch detection and override

Usage: <b><span class=c>wt config state default-branch</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get the default branch
  <b><span class=c>set</span></b>    Set the default branch
  <b><span class=c>clear</span></b>  Clear the default branch cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

## wt config state ci-status

CI status cache.

Caches GitHub/GitLab CI status for display in [`wt list`](https://worktrunk.dev/list/#ci-status).

Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated. Platform auto-detects from remote URL; override with `ci.platform = "github"` in `.config/wt.toml` for self-hosted instances.

Checks open PRs/MRs first, then branch pipelines for branches with upstream. Local-only branches (no remote tracking) show blank.

Results cache for 30-60 seconds. Indicators dim when local changes haven't been pushed.

### Status values

| Status | Meaning |
|--------|---------|
| `passed` | All checks passed |
| `running` | Checks in progress |
| `failed` | Checks failed |
| `conflicts` | PR has merge conflicts |
| `no-ci` | No checks configured |
| `error` | Fetch error (rate limit, network, auth) |

See [`wt list` CI status](https://worktrunk.dev/list/#ci-status) for display symbols and colors.

Without a subcommand, runs `get` for the current branch. Use `clear` to reset cache for a branch or `clear --all` to reset all.

### Command reference

wt config state ci-status - CI status cache

Usage: <b><span class=c>wt config state ci-status</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get CI status for a branch
  <b><span class=c>clear</span></b>  Clear CI status cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

## wt config state marker

Branch markers.

Custom status text or emoji shown in the `wt list` Status column.

### Display

Markers appear at the start of the Status column:

```
Branch    Status   Path
main      ^        ~/code/myproject
feature   🚧↑      ~/code/myproject.feature
bugfix    🤖!↑⇡    ~/code/myproject.bugfix
```

### Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code plugin](https://worktrunk.dev/claude-code/) sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

### Storage

Stored in git config as `worktrunk.state.<branch>.marker`. Set directly with:

```bash
git config worktrunk.state.feature.marker '{"marker":"🚧","set_at":0}'
```

Without a subcommand, runs `get` for the current branch. For `--branch`, use `get --branch=NAME`.

### Command reference

wt config state marker - Branch markers

Usage: <b><span class=c>wt config state marker</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get marker for a branch
  <b><span class=c>set</span></b>    Set marker for a branch
  <b><span class=c>clear</span></b>  Clear marker for a branch

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)

## wt config state logs

Background operation logs.

View and manage logs from background operations.

### What's logged

| Operation | Log file |
|-----------|----------|
| post-start hooks | `{branch}-{source}-post-start-{name}.log` |
| Background removal | `{branch}-remove.log` |

Source is `user` or `project` depending on where the hook is defined.

### Location

All logs are stored in `.git/wt-logs/` (in the main worktree's git directory).

### Behavior

- **Overwrites** — Same operation on same branch overwrites previous log
- **Persists** — Logs from deleted branches remain until manually cleared
- **Shared** — All worktrees write to the same log directory

### Examples

List all log files:
```bash
wt config state logs get
```

View a specific log:
```bash
cat "$(git rev-parse --git-dir)/wt-logs/feature-project-post-start-build.log"
```

Clear all logs:
```bash
wt config state logs clear
```

### Command reference

wt config state logs - Background operation logs

Usage: <b><span class=c>wt config state logs</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get log file paths
  <b><span class=c>clear</span></b>  Clear background operation logs

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
