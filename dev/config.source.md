# Worktrunk User Configuration

Location: `~/.config/worktrunk/config.toml`

Create with `wt config create`. Alternative locations follow XDG Base Directory
spec: `$XDG_CONFIG_HOME/worktrunk/config.toml` on macOS/Linux,
`%APPDATA%\worktrunk\config.toml` on Windows.

## Worktree Path Template

Controls where new worktrees are created. Paths are relative to the repository
root.

**Variables:**

- `{{ repo }}` — repository directory name
- `{{ branch }}` — raw branch name (e.g., `feature/auth`)
- `{{ branch | sanitize }}` — filesystem-safe: `/` and `\` become `-` (e.g., `feature-auth`)
- `{{ branch | sanitize_db }}` — database-safe: lowercase, underscores, hash suffix (e.g., `feature_auth_x7k`)

**Examples** for repo at `~/code/myproject`, branch `feature/auth`:

Siblings in parent directory (default) → `~/code/myproject.feature-auth`:

```toml
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
```

Inside the repository → `~/code/myproject/.worktrees/feature-auth`:

```toml
worktree-path = ".worktrees/{{ branch | sanitize }}"
```

## List Command Defaults

Persistent flag values for `wt list`. Override on command line as needed.

```toml
[list]
full = false       # Show CI status and main…± diffstat columns (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote-only branches (--remotes)
```

## Commit Defaults

Shared by `wt step commit`, `wt step squash`, and `wt merge`.

```toml
[commit]
stage = "all"      # What to stage before commit: "all", "tracked", or "none"
```

## Merge Command Defaults

All flags are on by default. Set to false to change default behavior.

```toml
[merge]
squash = true      # Squash commits into one (--no-squash to preserve history)
commit = true      # Commit uncommitted changes first (--no-commit to skip)
rebase = true      # Rebase onto target before merge (--no-rebase to skip)
remove = true      # Remove worktree after merge (--no-remove to keep)
verify = true      # Run project hooks (--no-verify to skip)
```

## Select Command Defaults

Pager behavior for `wt select` diff previews.

```toml
[select]
# Pager command with flags for diff preview (overrides git's core.pager)
# Use this to specify pager flags needed for non-TTY contexts
# Example: pager = "delta --paging=never"
```

## LLM Commit Messages

Generate commit messages automatically during merge. Requires an external CLI
tool. See <https://worktrunk.dev/llm-commits/> for setup.

Using [llm](https://github.com/simonw/llm) (install: `pip install llm llm-anthropic`):

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
```

Using [aichat](https://github.com/sigoden/aichat):

```toml
[commit-generation]
command = "aichat"
args = ["-m", "claude:claude-haiku-4.5"]
```

Load templates from external files (supports `~` expansion):

```toml
[commit-generation]
template-file = "~/.config/worktrunk/commit-template.txt"
squash-template-file = "~/.config/worktrunk/squash-template.txt"
```

See [Custom Prompt Templates](#custom-prompt-templates) for inline template options.

## Approved Commands

Commands approved for project hooks. Auto-populated when approving hooks on
first run, or via `wt hook approvals add`.

```toml
[projects."github.com/user/repo"]
approved-commands = ["npm ci", "npm test"]
```

For project-specific hooks (post-create, post-start, pre-merge, etc.), use a
separate project config at `<repo>/.config/wt.toml`. Run `wt config create --project`
to create one, or see <https://worktrunk.dev/hook/>.

## Custom Prompt Templates

These options belong under the `[commit-generation]` section. Uses
[minijinja](https://docs.rs/minijinja/) syntax.

### Commit Template

Available variables: `{{ git_diff }}`, `{{ git_diff_stat }}`, `{{ branch }}`,
`{{ recent_commits }}`, `{{ repo }}`

Default template:

<!-- DEFAULT_TEMPLATE_START -->
```toml
[commit-generation]
template = """
Write a commit message for the staged changes below.

<format>
- Subject under 50 chars, blank line, then optional body
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

### Squash Template

Available variables: `{{ git_diff }}`, `{{ git_diff_stat }}`, `{{ branch }}`,
`{{ recent_commits }}`, `{{ repo }}`, `{{ commits }}`, `{{ target_branch }}`

Default template:

<!-- DEFAULT_SQUASH_TEMPLATE_START -->
```toml
[commit-generation]
squash-template = """
Combine these commits into a single commit message.

<format>
- Subject under 50 chars, blank line, then optional body
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
