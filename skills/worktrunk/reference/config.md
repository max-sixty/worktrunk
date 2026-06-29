# wt config

Manage user & project configs. Includes shell integration, hooks, and saved state.

## Examples

Install shell integration (required for directory switching):

```bash
$ wt config shell install
```

Create user config file with documented examples:

```bash
$ wt config create
```

Create project config file (`.config/wt.toml`) for hooks:

```bash
$ wt config create --project
```

Show current configuration and file locations:

```bash
$ wt config show
```

## Configuration files

| File | Location | Contains | Committed & shared |
|------|----------|----------|--------------------|
| **User config** | `~/.config/worktrunk/config.toml` | Worktree path template, LLM commit configs, etc | ✗ |
| **Project config** | `.config/wt.toml` | Project hooks, dev server URL | ✓ |

Organizations can deploy a system-wide config file for shared defaults — run `wt config show` for the platform-specific location.

**User config** — personal preferences:

```toml
# ~/.config/worktrunk/config.toml
worktree-path = ".worktrees/{{ branch | sanitize }}"

[commit.generation]
command = "MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --safe-mode --setting-sources='user' --system-prompt=''"
```

**Project config** — shared team settings:

```toml
# .config/wt.toml
[pre-start]
deps = "npm ci"

[pre-merge]
test = "npm test"
```

<!-- USER_CONFIG_START -->
# User Configuration

Create with `wt config create`. Values shown are defaults unless noted otherwise.

Location:

- macOS/Linux: `~/.config/worktrunk/config.toml` (or `$XDG_CONFIG_HOME` if set)
- Windows: `%APPDATA%\worktrunk\config.toml`

## Worktree path template

Controls where new worktrees are created.

**Available template variables:**

- `{{ repo_path }}` — absolute path to the repository root (e.g., `/Users/me/code/myproject`. Or for bare repos, the bare directory itself)
- `{{ repo }}` — repository directory name (e.g., `myproject`)
- `{{ owner }}` — primary remote owner path (may include subgroups like `group/subgroup`)
- `{{ branch }}` — raw branch name (e.g., `feature/auth`)
- `{{ branch | sanitize }}` — filesystem-safe: `/` and `\` become `-` (e.g., `feature-auth`)
- `{{ branch | sanitize_db }}` — database-safe: lowercase, underscores, hash suffix (e.g., `feature_auth_x7k`)
- `{{ branch | codename(2) }}` — deterministic friendly name from a ~1.26M-combo pool (e.g., `malleable-opah`)

This is a smaller set than [the variables hooks and aliases get](https://worktrunk.dev/hook/#template-variables).

**Examples** for repo at `~/code/myproject`, branch `feature/auth`:

Default — sibling directory (`~/code/myproject.feature-auth`):

```toml
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
```

Inside the repository (`~/code/myproject/.worktrees/feature-auth`):

```toml
worktree-path = "{{ repo_path }}/.worktrees/{{ branch | sanitize }}"
```

Friendly branch-derived names (`~/code/myproject.malleable-opah`):

```toml
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | codename(2) }}"
```

Friendly names with branch identity in a parent directory (`~/code/worktrees/feature-auth/malleable-opah`):

```toml
worktree-path = "{{ repo_path }}/../worktrees/{{ branch | sanitize }}/{{ branch | codename(2) }}"
```

Centralized worktrees directory (`~/worktrees/myproject/feature-auth`):

```toml
worktree-path = "~/worktrees/{{ repo }}/{{ branch | sanitize }}"
```

By remote owner path (`~/development/max-sixty/myproject/feature/auth`):

```toml
worktree-path = "~/development/{{ owner }}/{{ repo }}/{{ branch }}"
```

Bare repository (`~/code/myproject/feature-auth`):

```toml
worktree-path = "{{ repo_path }}/../{{ branch | sanitize }}"
```

`~` expands to the home directory. Relative paths resolve from `repo_path`.

## LLM commit messages

Generate commit messages automatically during merge. Requires an external CLI tool.

### Claude Code

```toml
[commit.generation]
command = "MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --safe-mode --setting-sources='user' --system-prompt=''"
```

### Codex

```toml
[commit.generation]
command = "codex exec -m gpt-5.4-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
```

### OpenCode

```toml
[commit.generation]
command = "opencode run -m anthropic/claude-haiku-4.5 --variant fast"
```

### llm

```toml
[commit.generation]
command = "llm -m claude-haiku-4.5"
```

### aichat

```toml
[commit.generation]
command = "aichat -m claude:claude-haiku-4.5"
```

See [LLM commits docs](https://worktrunk.dev/llm-commits/) for setup and [Custom prompt templates](#custom-prompt-templates) for template customization.

## Command config

### List

Persistent flag values for `wt list`. Override on command line as needed.

```toml
[list]
summary = false    # Enable LLM branch summaries (requires [commit.generation])

full = false       # Show CI status and LLM summaries (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote-only branches (--remotes)

columns = ["branch", "status", "ci", "path"]   # Columns to show, in order — built-ins or custom headers (omit for the default set)

task-timeout-ms = 0   # Kill individual git commands after N ms; 0 disables
timeout-ms = 0        # Wall-clock budget for the entire collect phase; 0 disables
```

`columns` selects and orders the columns to render; omit it for the default set.
It is meant to drive a per-invocation [alias](https://worktrunk.dev/extending/#aliases)
(`wt --config-set 'list.columns=[…]' list`), giving a named view without
disturbing the default `wt list`. A static setting works but pins one layout
over a table that otherwise adapts to `--full` and terminal width.

Valid built-in names:

- `branch` — The branch name
- `status` — Git status symbols, plus any user-defined status
- `working-diff` — Uncommitted line changes against `HEAD` (header `HEAD±`)
- `ahead-behind` — Commits ahead of and behind the default branch (header `main↕`)
- `branch-diff` — Line changes against the default branch (header `main…±`)
- `summary` — An LLM-generated summary of the branch
- `upstream` — Commits ahead of and behind the upstream tracking branch (header `Remote⇅`)
- `ci` — CI status of the head commit
- `path` — The worktree's path
- `url` — Dev-server URL from the `[list] url` template
- `commit` — The head commit's short hash
- `age` — Time since the last commit
- `message` — The head commit's subject

A selection mixes built-ins with [custom columns](#custom-columns), each named
by its `[list.custom-columns]` header (`columns = ["branch", "Ticket", "ci"]`),
and is exhaustive: only the listed columns render. Omit `columns` to keep the
default set, where custom columns append automatically. A built-in name wins a
header collision; the gutter type indicator always shows.

Listing a column forces it on, space permitting: `ci` shows without `--full`,
since `--full` only bundles columns into the default table rather than gating a
named one. A column whose data source is missing still stays hidden — `summary`
needs an LLM command (`[commit.generation]`), `url` needs a `[list] url`
template — since listing can't supply the data.

The selection drives the table and the `wt switch` picker; `wt list --format
json` ignores it and emits every field.

#### Custom columns [experimental]

Custom columns add per-branch context to the `wt list` table. Each
`[list.custom-columns]` entry is a column: the key is the header, the template
renders each row's cell.

```toml
[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"   # Required; the result is the cell text
width = 20                       # Optional max display width (default: 40)
priority = 9                     # Optional drop order when the terminal narrows;
                                 # lower = kept longer (default: 9, the URL band)
```

Templates may reference `{{ branch }}`, `{{ worktree_path }}`,
`{{ worktree_name }}` (empty for branch-only rows), and two per-branch
namespaces:

- `{{ vars.* }}` — values stored with
  [`wt config state vars set`](https://worktrunk.dev/config/#wt-config-state-vars).
- `{{ git.branch.* }}` — the branch's own git config under `branch.<name>.*`,
  read straight from `git config` (e.g. `{{ git.branch.jira }}` for a key you
  set yourself, or the git-native `description`). Git lowercases config variable
  names, so `branch.<name>.nvciShelf` reads as `{{ git.branch.nvcishelf }}`.

All standard filters work (`sanitize`, `hash_port`, `codename`, …). A row
where the template renders empty (e.g. a branch without the key) shows an
empty cell; a column that is empty for every row is dropped from the table.
`wt list --format json` includes the rendered values under `columns`.

A `Jira` column reading a key kept in git config, and a `Summary` column
showing just the first line of the git-native branch description:

```toml
[list.custom-columns.Jira]
template = "{{ git.branch.jira }}"

[list.custom-columns.Summary]
template = "{{ git.branch.description | lines | first }}"
```

### Commit

Shared by `wt step commit`, `wt step squash`, and `wt merge`.

```toml
[commit]
stage = "all"      # What to stage before commit: "all", "tracked", or "none"
```

### Merge

Most flags are on by default. Set to false to change default behavior.

```toml
[merge]
squash = true      # Squash commits into one (--no-squash to preserve history)
commit = true      # Commit uncommitted changes first (--no-commit to skip)
rebase = true      # Rebase onto target before merge (--no-rebase to skip)
remove = true      # Remove worktree after merge (--no-remove to keep)
verify = true      # Run project hooks (--no-hooks to skip)
ff = true          # Fast-forward merge (--no-ff to create a merge commit instead)
```

### Remove

Persistent flag values for `wt remove`. Override on command line as needed.

```toml
[remove]
delete-branch = true   # Delete branch after removal (--no-delete-branch to keep)
```

### Switch

```toml
[switch]
cd = true          # Change directory after switching (--no-cd to skip)

[switch.picker]
pager = "delta --paging=never"   # Example: override git's core.pager for diff preview
```

### Step

```toml
[step.copy-ignored]
exclude = []   # Additional excludes (e.g., [".cache/", ".turbo/"])
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.worktrees/`). User config and project config exclusions are combined.

### Aliases

Command templates that run as `wt <name>`. See the [Extending Worktrunk guide](https://worktrunk.dev/extending/#aliases) for usage and flags.

```toml
[aliases]
greet = "echo Hello from {{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here apply to all projects. For project-specific aliases, use the [project config](https://worktrunk.dev/config/#project-configuration) `[aliases]` section instead.

### User project-specific settings

User config can include a `[projects]` table for project-specific settings — worktree layout, setting overrides, anything else — separate from the [project config](https://worktrunk.dev/config/#project-configuration) shared with teammates.

Entries are keyed by project identifier — `<host>/<owner>/<repo>` derived from the primary remote URL (no `.git` suffix), or the canonical repo path when there is no remote. Run `wt config show` inside the repo to see the identifier for the current project; it appears in the `PROJECT CONFIG` section as `Identifier: …`.

Scalar values (like `worktree-path`) replace the global value; everything else (hooks, aliases, etc.) appends, global first.

```toml
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
list.full = true
merge.squash = false
remove.delete-branch = false
pre-start.env = "cp .env.example .env"
step.copy-ignored.exclude = [".repo-local-cache/"]
aliases.deploy = "make deploy BRANCH={{ branch }}"
```

Hooks support all three [hook forms](https://worktrunk.dev/hook/#hook-forms). A table runs multiple commands concurrently; an array-of-tables pipeline runs steps in sequence. The dotted-key examples below are equivalent to the table forms — TOML treats `projects."github.com/user/repo".post-start.server = "..."` and a `[projects."github.com/user/repo".post-start]` table the same way:

```toml
# Single command
[projects."github.com/user/repo"]
post-start = "mise trust"

# Multiple commands, running concurrently
[projects."github.com/user/repo".post-start]
mise = "mise trust"
server = "npm run dev"

# Pipeline: steps run in sequence
[[projects."github.com/user/repo".post-start]]
install = "npm ci"

[[projects."github.com/user/repo".post-start]]
build = "npm run build"
server = "npm run dev"
```

### Custom prompt templates

Templates use [minijinja](https://docs.rs/minijinja/) syntax.

#### Commit template

Available variables:

- `{{ git_diff }}`, `{{ git_diff_stat }}` — diff content
- `{{ branch }}`, `{{ repo }}` — context
- `{{ recent_commits }}` — recent commit messages
- `{{ user_guidance }}`, `{{ project_guidance }}` — rendered append fragments (see [Appending to the prompt](https://worktrunk.dev/config/#appending-to-the-prompt))

Default template:

<!-- DEFAULT_TEMPLATE_START -->
```toml
[commit.generation]
template = """
<task>Write a commit message for the staged changes below.</task>

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
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
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

- `{{ commit_details }}` — list of commits being squashed; each renders as its subject and exposes `.subject` / `.body`
- `{{ target_branch }}` — merge target branch

Default template:

<!-- DEFAULT_SQUASH_TEMPLATE_START -->
```toml
[commit.generation]
squash-template = """
<task>Write a commit message for the combined effect of these commits.</task>

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
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for detail in commit_details %}- {{ detail.subject }}
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

#### Appending to the prompt [experimental]

`template-append` adds to the prompt instead of replacing it. The value is rendered as its own minijinja template (same variables) and injected into the default templates' `{{ user_guidance }}` slot — a `<user-guidance>` block right after `<style>`. It applies to both commit and squash. Use it for personal preferences without restating the whole template:

```toml
[commit.generation]
template-append = """
- Explain the rationale in the body, not just the change
"""
```

The [project config](https://worktrunk.dev/config/#project-configuration) has a `template-append` of its own; it renders into a separate `<project-guidance>` block right after `<user-guidance>`.

## Hooks

See [`wt hook`](https://worktrunk.dev/hook/) for hook types, execution order, template variables, and examples. User hooks apply to all projects; [project hooks](https://worktrunk.dev/config/#project-configuration) apply only to that repository.
<!-- USER_CONFIG_END -->
<!-- PROJECT_CONFIG_START -->
# Project Configuration

Project configuration lets teams share repository-specific settings — hooks, dev server URLs, and other defaults. The file lives in `.config/wt.toml` and is typically checked into version control.

To create a starter file with commented-out examples, run `wt config create --project`.

## Hooks

Project hooks apply to this repository only. See [`wt hook`](https://worktrunk.dev/hook/) for hook types, execution order, and examples.

```toml
pre-start = "npm ci"
post-start = "npm run dev"
pre-merge = "npm test"
```

## Dev server URL

URL column in `wt list` (dimmed when port not listening):

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

## Forge platform

Name the forge explicitly for SSH aliases or self-hosted instances, where it can't be detected from the remote URL:

```toml
[forge]
platform = "github"  # or "gitlab", "gitea" (experimental), "azure-devops" (experimental)
hostname = "github.example.com"  # Example: API host (GHE / self-hosted GitLab)
```

## Commit-message append [experimental]

Project-wide commit-message conventions appended to the LLM commit and squash prompts inside a `<project-guidance>` block, after the main template's `<style>` section (and after any user `<user-guidance>`). Rendered as a [minijinja](https://docs.rs/minijinja/) template with the same variables as the main commit template (`{{ branch }}`, `{{ git_diff }}`, etc.), so it can reference them directly. The first time the fragment changes, `wt` prompts the user to approve it — the same one-shot gate as project-defined hooks.

```toml
[commit.generation]
template-append = """
- Use conventional commits (feat:, fix:, docs:, …)
- Reference the relevant issue ID in the body
"""
```

Only `template-append` is honored from the project file. The LLM command and the main prompt template stay in [user config](https://worktrunk.dev/config/) — they describe per-developer environment (which CLI is installed, which agent the developer prefers). User config has a `[commit.generation] template-append` of its own; it renders into a separate `<user-guidance>` block immediately before this one.

## Copy-ignored excludes

Additional excludes for `wt step copy-ignored`:

```toml
[step.copy-ignored]
exclude = [".cache/", ".turbo/"]
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.worktrees/`). User config and project config exclusions are combined.

## Aliases

Command templates that run as `wt <name>`. See the [Extending Worktrunk guide](https://worktrunk.dev/extending/#aliases) for usage and flags.

```toml
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here are shared with teammates. For personal aliases, use the [user config](https://worktrunk.dev/config/#aliases) `[aliases]` section instead.
<!-- PROJECT_CONFIG_END -->

# Shell Integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

```bash
$ wt config shell install
```

For manual setup, see `wt config shell init --help`.

Without shell integration, `wt switch` prints the target directory but cannot `cd` into it.

### First-run prompts

On first run without shell integration, Worktrunk offers to install it. On first commit without LLM configuration, it offers to configure a detected tool (`claude`, `codex`). Declining sets `skip-shell-integration-prompt` or `skip-commit-generation-prompt` automatically.

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

### Example: CI/testing override

Override the LLM command in CI to use a mock:

```bash
$ WORKTRUNK_COMMIT__GENERATION__COMMAND="echo 'test: automated commit'" wt merge
```

### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers; useful for testing dev builds |
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_SYSTEM_CONFIG_PATH` | Override system config file location |
| `WORKTRUNK_PROJECT_CONFIG_PATH` | Override project config file location (defaults to `.config/wt.toml`) |
| `XDG_CONFIG_DIRS` | Colon-separated system config directories (default: `/etc/xdg`) |
| `WORKTRUNK_DIRECTIVE_CD_FILE` | Internal: set by shell wrappers. wt writes a raw path; the wrapper `cd`s to it |
| `WORKTRUNK_DIRECTIVE_EXEC_FILE` | Internal: set by shell wrappers. wt writes shell commands; the wrapper sources the file |
| `WORKTRUNK_SHELL` | Internal: set by shell wrappers to indicate shell type (e.g., `powershell`) |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting file descriptor limits. |
| `WORKTRUNK_VERBOSE` | Verbosity level (`0`/`1`/`2`), like `-v`/`-vv` but applied everywhere — including shell completion, which no flag can reach |
| `RUST_LOG` | Logging directive (e.g. `worktrunk=debug`); overrides the verbosity baseline for what reaches stderr |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

## Inline config overrides (`--config-set`)

`--config-set <toml>` overrides any user config key for a single invocation, with higher priority than both config files and `WORKTRUNK_` env vars. The value is a TOML fragment, so arrays and tables work directly; the flag is global (works before or after the subcommand), repeatable, and a later `--config-set` replaces an earlier one for the same key.

```bash
$ wt --config-set list.full=true list
$ wt step copy-ignored --config-set 'step.copy-ignored.exclude=["target", "dist"]'
```

This composes with aliases — an alias body can invoke `wt --config-set … <command>` to render a named view without changing the saved config.

## Command reference

```
wt config - Manage user & project configs

Includes shell integration, hooks, and saved state.

Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create configuration file
  show       Show configuration files & locations
  update     Update deprecated config settings
  approvals  Manage command approvals
  alias      Inspect and preview aliases
  plugins    Plugin management
  state      Manage internal data and cache

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

# Subcommands

## wt config show

Show configuration files & locations.

Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`). Also shows system config if present.

If a config file doesn't exist, shows defaults that would be used.

### Full diagnostics

Use `--full` to run diagnostic checks:

```bash
$ wt config show --full
```

This tests:
- **CI tool status** — Whether `gh` (GitHub) or `glab` (GitLab) is installed and authenticated
- **Commit generation** — Whether the LLM command can generate commit messages
- **Version check** — Whether a newer version is available on GitHub

### Command reference

```
wt config show - Show configuration files & locations

Usage: wt config show [OPTIONS]

Options:
      --full
          Run diagnostic checks (CI tools, commit generation, version)

  -h, --help
          Print help (see a summary with '-h')

Output:
      --format <FORMAT>
          Output format

          [default: text]
          [possible values: text, json]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config approvals

Manage command approvals.

Project hooks and project aliases prompt for approval on first run to prevent untrusted projects from running arbitrary commands. Approvals from both flows are stored together.

### Examples

Pre-approve all hook and alias commands for current project:
```bash
$ wt config approvals add
```

Clear approvals for current project:
```bash
$ wt config approvals clear
```

Clear global approvals:
```bash
$ wt config approvals clear --global
```

### How approvals work

Approved commands are saved to `~/.config/worktrunk/approvals.toml`. Re-approval is required when the command template changes or the project moves. Use `--yes` to bypass prompts in CI.

### Command reference

```
wt config approvals - Manage command approvals

Usage: wt config approvals [OPTIONS] <COMMAND>

Commands:
  add    Store approvals in approvals.toml
  clear  Clear approved commands from approvals.toml

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config alias

Inspect and preview aliases.

Aliases are command templates configured in user (`~/.config/worktrunk/config.toml`) or project (`.config/wt.toml`) config and run as `wt <name>`. See the [Extending Worktrunk guide](https://worktrunk.dev/extending/#aliases) for the configuration format.

### Examples

Show every configured alias's template:
```bash
$ wt config alias show
```

Show the template for `deploy`:
```bash
$ wt config alias show deploy
```

Preview an invocation without running it:
```bash
$ wt config alias dry-run deploy
$ wt config alias dry-run deploy -- --env=staging
```

### Command reference

```
wt config alias - Inspect and preview aliases

Usage: wt config alias [OPTIONS] <COMMAND>

Commands:
  show     Show an alias's template, or all aliases' templates
  dry-run  Preview an alias invocation with template expansion

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state

Manage internal data and cache.

State is stored in `.git/` (config entries and log files), separate from configuration files.

### Keys

- **cache**: [Regenerable caches — CI status, summaries, git commands, hints, and the `wt switch -` target](https://worktrunk.dev/config/#wt-config-state-cache)
- **default-branch**: [The repository's default branch (`main`, `master`, etc.)](https://worktrunk.dev/config/#wt-config-state-default-branch)
- **marker**: [Custom status marker for a branch (shown in `wt list`)](https://worktrunk.dev/config/#wt-config-state-marker)
- **vars**: [experimental] [Custom variables per branch](https://worktrunk.dev/config/#wt-config-state-vars)
- **logs**: [Operation and debug logs](https://worktrunk.dev/config/#wt-config-state-logs)

### Examples

Get the default branch:
```bash
$ wt config state default-branch
```

Set the default branch manually:
```bash
$ wt config state default-branch set main
```

Set a marker for current branch:
```bash
$ wt config state marker set 🚧
```

Store arbitrary data:
```bash
$ wt config state vars set env=staging
```

Drop the regenerable caches:
```bash
$ wt config state cache clear
```

Show all stored state:
```bash
$ wt config state get
```

Clear all stored state:
```bash
$ wt config state clear
```

### Command reference

```
wt config state - Manage internal data and cache

Usage: wt config state [OPTIONS] <COMMAND>

Commands:
  get             Get all stored state
  clear           Clear all stored state
  cache           Regenerable caches
  default-branch  Default branch detection and override
  logs            Operation and debug logs
  marker          Branch markers
  vars            [experimental] Custom variables per branch

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state cache

Regenerable caches.

View or drop worktrunk's regenerable caches in one place. Everything here is rebuilt on demand — clearing only forces recomputation, never data loss.

### What's cached

- **CI status** — GitHub/GitLab CI per branch (30–60s TTL), shown in [`wt list`](https://worktrunk.dev/list/#ci-status), plus the largest PR/MR number seen (sizes the CI column)
- **Summaries** — LLM-generated branch summaries (`wt list --full`, `wt switch` preview)
- **Git commands** — SHA-keyed disk caches: merge-tree, ancestry, diff-stats, and `wt switch` preview renders
- **Hints** — one-time hints already shown in this repo
- **Previous branch** — the `wt switch -` target, re-recorded on the next switch

`cache clear` drops all of the above with no prompt. It re-shows one-time hints and forgets the `wt switch -` target until the next switch — both repopulate on their own.

Without a subcommand, runs `get`.

### Examples

Show cache contents:
```bash
$ wt config state cache
```

Drop all caches:
```bash
$ wt config state cache clear
```

### Command reference

```
wt config state cache - Regenerable caches

Usage: wt config state cache [OPTIONS] [COMMAND]

Commands:
  get    Show cache contents
  clear  Drop all caches

Options:
  -h, --help
          Print help (see a summary with '-h')

Output:
      --format <FORMAT>
          Output format (text, json) [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state default-branch

Default branch detection and override.

Useful in scripts to avoid hardcoding `main` or `master`:

```bash
$ git rebase $(wt config state default-branch)
```

In a hook or alias template, prefer the `{{ default_branch }}` [template variable](https://worktrunk.dev/hook/#template-variables); `$(wt config state default-branch)` is for plain shell scripts.

Without a subcommand, runs `get`. Use `set` to override, or `clear` then `get` to re-detect.

`default-branch get` resolves the value and caches it on a miss; the aggregate `wt config state get` only reports the cache (read-only), so it can show `(none)` until something populates it.

### Detection

Worktrunk detects the default branch automatically:

1. **Worktrunk cache** — Checks `git config worktrunk.default-branch`
2. **Git cache** — Detects primary remote and checks its HEAD (e.g., `origin/HEAD`)
3. **Remote query** — If not cached, queries `git ls-remote` — typically 100ms–2s
4. **Local inference** — If no remote, infers from local branches

Once detected, the result is cached in `worktrunk.default-branch` for fast access.

The local inference fallback uses these heuristics in order:
- If only one local branch exists, uses it
- For bare repos or empty repos, checks `symbolic-ref HEAD`
- Checks `git config init.defaultBranch`
- Looks for common names: `main`, `master`, `develop`, `trunk`

If none of these match, detection fails; set it explicitly with `wt config state default-branch set BRANCH`.

### Command reference

```
wt config state default-branch - Default branch detection and override

Usage: wt config state default-branch [OPTIONS] [COMMAND]

Commands:
  get    Get the default branch
  set    Set the default branch
  clear  Clear the default branch cache

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state logs

Operation and debug logs.

View and manage log files — hook output, command audit trail, and debug diagnostics.

### What's logged

Three kinds of logs live in `.git/wt/logs/`:

#### Command log (`commands.jsonl`)

All hook executions and LLM commands are recorded automatically — one JSON object per line. Rotates to `commands.jsonl.old` at 1MB (~2MB total). Fields:

| Field | Description |
|-------|-------------|
| `ts` | ISO 8601 timestamp |
| `wt` | The `wt` command that triggered this (e.g., `wt hook pre-merge --yes`) |
| `label` | What ran (e.g., `pre-merge user:lint`, `commit.generation`) |
| `cmd` | Shell command executed |
| `exit` | Exit code (`null` for background commands) |
| `dur_ms` | Duration in milliseconds (`null` for background commands) |

The command log appends entries and is not branch-specific — it records all activity across all worktrees.

#### Hook output logs

Hook output lives in per-branch subtrees under `.git/wt/logs/{branch}/`:

| Operation | Log path |
|-----------|----------|
| Background hooks | `{branch}/{source}/{hook-type}/{name}.log` |
| Background removal | `{branch}/internal/remove.log` |

All `post-*` hooks (post-start, post-switch, post-commit, post-merge) run in the background and produce log files. Source is `user` or `project`. Branch and hook names are sanitized for filesystem safety (invalid characters → `-`; short collision-avoidance hash appended). Same operation on same branch overwrites the previous log. Removing a branch clears its subtree; orphans from deleted branches can be swept with `wt config state logs clear`.

#### Diagnostic files

| File | Created when |
|------|-------------|
| `trace.log` | Running with `-vv` |
| `trace.jsonl` | Running with `-vv` |
| `subprocess.log` | Running with `-vv` |
| `profile.txt` | Running with `-vv` |
| `diagnostic.md` | Running with `-vv` |

`trace.log` is the human-readable trace at `-vv` — each command's start (`$ …`) and completion (`✓`/`✗ … 12.3ms`), in-process spans, milestones, and bounded subprocess previews. `trace.jsonl` is the same event stream as one JSON object per line, for machines (`jq`, chrome://tracing); `wt config state logs profile` reads it to summarize a performance report (where time went, parallelism, redundant commands). `subprocess.log` holds the raw uncapped subprocess stdout/stderr bodies. `profile.txt` is that performance report saved standalone — the same rendering `wt config state logs profile` produces, ready to `cat`. `diagnostic.md` is a markdown bug-report bundle that inlines `trace.log` and the same profile; `wt` prints a `gh gist create` command pointing at it. All five are overwritten on each `-vv` run.

### Location

All logs are stored in `.git/wt/logs/` (in the main worktree's git directory). All worktrees write to the same directory. Top-level files are shared logs (command audit + diagnostics); top-level directories are per-branch log trees.

### Structured output

`wt config state logs --format=json` emits three arrays — `command_log`, `hook_output`, `diagnostic`. Each entry carries a `file` (relative), `path` (absolute), `size`, and `modified_at` (unix seconds). Hook-output entries additionally expose `branch`, `source` (`user` / `project` / `internal`), `hook_type` (the `post-*` kind, or `null` for internal ops), and `name`. Filter with `jq` to pick out a specific entry.

### Examples

List all log files:
```bash
$ wt config state logs
```

Query the command log:
```bash
$ tail -5 .git/wt/logs/commands.jsonl | jq .
```

Path to one hook log (e.g. the `post-start` `server` hook for the current branch):
```bash
$ wt config state logs --format=json | jq -r '.hook_output[] | select(.source == "user" and .hook_type == "post-start" and (.name | startswith("server"))) | .path'
```

Logs for a specific branch:
```bash
$ wt config state logs --format=json | jq '.hook_output[] | select(.branch | startswith("feature"))'
```

Clear all logs:
```bash
$ wt config state logs clear
```

### Command reference

```
wt config state logs - Operation and debug logs

Usage: wt config state logs [OPTIONS] [COMMAND]

Commands:
  get      List all log file paths
  profile  Performance profile from a trace
  clear    Clear all log files

Options:
  -h, --help
          Print help (see a summary with '-h')

Output:
      --format <FORMAT>
          Output format (text, json) [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state ci-status

CI status cache.

**Deprecated** — the CI status cache is now part of [`wt config state cache`](https://worktrunk.dev/config/#wt-config-state-cache). This subcommand still works but prints a deprecation notice.

Caches GitHub/GitLab CI status for display in [`wt list`](https://worktrunk.dev/list/#ci-status).

Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated. Platform auto-detects from the remote URL; set `forge.platform = "github"` (or `"gitlab"`) in `.config/wt.toml` for SSH host aliases or self-hosted instances. For GitHub Enterprise or self-hosted GitLab, also set `forge.hostname`.

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

```
wt config state ci-status - CI status cache

Usage: wt config state ci-status [OPTIONS] [COMMAND]

Commands:
  get    Get CI status for a branch
  clear  Clear CI status cache

Options:
  -h, --help
          Print help (see a summary with '-h')

Output:
      --format <FORMAT>
          Output format (text, json) [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state marker

Branch markers.

Custom status text or emoji shown in the `wt list` Status column.

### Display

Markers appear at the end of the Status column, after git symbols:

```
$ wt list
  Branch       Status        HEAD±    main↕     main…±  Remote⇅  Commit    Age   Message
@ main             ^⇡                                    ⇡1      33323bc1  1d    Initial commit
+ feature-api      ↑ 🤖              ↑1        +1                70343f03  1d    Add REST API end…
+ review-ui      ? ↑ 💬              ↑1        +1                a585d6ed  1d    Add dashboard co…
+ wip-docs       ? –                                             33323bc1  1d    Initial commit

○ Showing 4 worktrees, 2 with changes, 2 ahead, 1 column hidden
```

### Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code](https://worktrunk.dev/claude-code/) plugin sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

### Storage

Stored in git config as `worktrunk.state.<branch>.marker`. Set directly with:

```bash
$ git config worktrunk.state.feature.marker '{"marker":"🚧","set_at":0}'
```

Without a subcommand, runs `get` for the current branch. For `--branch`, use `get --branch=NAME`.

### Command reference

```
wt config state marker - Branch markers

Usage: wt config state marker [OPTIONS] [COMMAND]

Commands:
  get    Get marker for a branch
  set    Set marker for a branch
  clear  Clear marker for a branch

Options:
  -h, --help
          Print help (see a summary with '-h')

Output:
      --format <FORMAT>
          Output format (text, json) [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

## wt config state vars

[experimental]

Custom variables per branch.

Store custom variables per branch. Values are stored as-is — plain strings or JSON.

### Examples

Set and get values:
```bash
$ wt config state vars set env=staging
$ wt config state vars get env
```

Store JSON:
```bash
$ wt config state vars set config='{"port": 3000, "debug": true}'
```

List all keys:
```bash
$ wt config state vars list
```

Operate on a different branch:
```bash
$ wt config state vars set env=production --branch=main
```

### Template access

Variables are available in [hook templates](https://worktrunk.dev/hook/#template-variables) as `{{ vars.<key> }}`. Use the `default` filter for keys that may not be set:

```toml
[post-start]
dev = "ENV={{ vars.env | default('development') }} npm start -- --port {{ vars.port | default('3000') }}"
```

JSON object and array values support dot access:

```bash
$ wt config state vars set config='{"port": 3000, "debug": true}'
```
```toml
[post-start]
dev = "npm start -- --port {{ vars.config.port }}"
```

### Storage format

Stored in git config as `worktrunk.state.<branch>.vars.<key>`. Keys must contain only letters, digits and hyphens — dots conflict with git config's section separator, underscores with its variable name format.

### Command reference

```
wt config state vars - [experimental] Custom variables per branch

Usage: wt config state vars [OPTIONS] <COMMAND>

Commands:
  get    Get a value
  list   List all keys
  set    Set a value
  clear  Clear a key or all keys

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```
