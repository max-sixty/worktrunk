# User Config Reference

Detailed guidance for configuring personal Worktrunk settings at `~/.config/worktrunk/config.toml`.

## Guiding Principle: Propose, Never Impose

Never edit user config files without explicit consent. Always:
1. Show the proposed change
2. Explain what it does
3. Wait for approval
4. Then apply

Never install tools (llm, aichat) automatically. Provide installation commands for users to run themselves.

## LLM Setup

Most users want this for LLM commit messages. Follow this sequence:

### Step 1: Check if LLM Tool Exists

```bash
which llm
# or
which aichat
```

### Step 2: Guide Installation (Don't Run It)

<example type="llm-install-guide">

For `llm` (Python-based, recommended):
```bash
uv tool install -U llm
```

For `aichat` (Rust-based, 20+ providers):
```bash
# See: https://github.com/sigoden/aichat
```

</example>

### Step 3: Guide API Key Setup (Don't Run It)

<example type="api-key-setup">

For Claude (via llm):
```bash
llm install llm-anthropic
llm keys set anthropic
# User pastes API key from: https://console.anthropic.com/settings/keys
llm models default claude-haiku-4.5
```

For OpenAI (via llm):
```bash
llm keys set openai
# User pastes API key from: https://platform.openai.com/api-keys
```

</example>

### Step 4: Propose Config Change

Show what will be added:

<example type="config-proposal">

```toml
[commit-generation]
command = "llm"
```

Ask: "Should I add this to your config at `~/.config/worktrunk/config.toml`?"

</example>

### Step 5: After Approval, Check if Config Exists

```bash
wt config show
```

If not: guide through `wt config create` first.

### Step 6: Apply the Change

Read existing config, add the `[commit-generation]` section, preserve existing structure and comments.

### Step 7: Suggest Testing

```bash
# Test LLM works
llm "say hello"

# Test with worktrunk (in a repo with uncommitted changes)
wt merge
```

## Worktree Paths

Users may want different worktree organization patterns.

### Common Patterns

<example type="worktree-patterns">

Default (parent siblings):
```toml
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
```
Result: `~/code/myproject` → `~/code/myproject.feature-auth`

Inside repo:
```toml
worktree-path = ".worktrees/{{ branch | sanitize }}"
```
Result: `~/code/myproject/.worktrees/feature-auth`

</example>

### Workflow

1. Show current setting from `wt config show`
2. Explain the new pattern with concrete example
3. Warn: "Existing worktrees won't move automatically"
4. Propose change
5. After approval, update config

### Available Variables

- `{{ repo }}` - Repository directory name
- `{{ branch }}` - Raw branch name (e.g., `feature/auth`)

### Filters

- `{{ branch | sanitize }}` - Replace `/` and `\` with `-` (e.g., `feature-auth`)
- `{{ branch | hash_port }}` - Hash string to deterministic port (10000-19999)

### Validation Rules

- Path must be relative, not absolute
- Path cannot be empty

## Templates

Users may want to customize the prompt sent to their LLM.

### Two Options

<example type="template-options">

Inline template:
```toml
[commit-generation]
command = "llm"
template = """
Write a commit message <50 chars.
Focus on WHAT changed.

Changes:
{{ git_diff }}
"""
```

Template file:
```toml
[commit-generation]
command = "llm"
template-file = "~/.config/worktrunk/commit-template.txt"
```

</example>

### Available Variables

**Commit message templates**:
- `{{ git_diff }}` - Staged changes
- `{{ branch }}` - Current branch
- `{{ recent_commits }}` - Recent commit titles
- `{{ repo }}` - Repository directory name

**Squash commit templates**:
- `{{ commits }}` - List of commits being squashed
- `{{ target_branch }}` - Target branch for merge
- `{{ branch }}` - Current branch
- `{{ repo }}` - Repository directory name

### Validation Rules

- `template` and `template-file` are mutually exclusive
- `squash-template` and `squash-template-file` are mutually exclusive
- Template files support tilde expansion: `~/...`

### Workflow

1. Understand what the user wants different
2. Propose template (inline or file-based)
3. Show available variables
4. After approval, update config

## Configuration Structure

Complete reference:

```toml
# Worktree Path Template
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

# LLM Commit Generation (Optional)
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]

# Command Settings (configure default flags)
[list]
full = true       # --full
branches = true   # --branches
remotes = true    # --remotes

[commit]
stage = "all"     # "all", "tracked", or "none"

[merge]
squash = true     # Squash commits (default: true)
commit = true     # Commit uncommitted changes (default: true)
rebase = true     # Rebase onto target (default: true)
remove = true     # Remove worktree after merge (default: true)
verify = true     # Run hooks (default: true)

# User Hooks (run for all repositories, no approval required)
[post-create]
setup = "echo 'Setting up worktree...'"

[pre-merge]
notify = "notify-send 'Merging {{ branch }}'"

# Optional: Custom prompt template (inline, Jinja2 syntax)
template = "..."

# Optional: Load template from file (mutually exclusive with 'template')
template-file = "~/.config/worktrunk/commit-template.txt"

# Optional: Custom squash commit template
squash-template = "..."

# Optional: Load squash template from file
squash-template-file = "~/.config/worktrunk/squash-template.txt"

# Approved Commands (auto-populated by wt switch --execute --yes)
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
```

## Troubleshooting

### LLM Integration Not Working

**Check sequence:**
1. Verify command exists: `which llm`
2. Test command directly: `llm "test"`
3. View config: `wt config show`
4. Check for template conflicts (both `template` and `template-file` set)
5. If template file is used, verify it exists

### Config Not Loading

**Check sequence:**
1. View config path: `wt config show` shows location
2. Verify file exists: `ls -la ~/.config/worktrunk/config.toml`
3. Check TOML syntax: `cat ~/.config/worktrunk/config.toml`
4. Look for validation errors (path must be relative, not absolute)

### Approved Commands Not Persisting

- Approved commands are auto-populated when using `wt switch --execute "cmd" --yes`
- Manual editing is possible but discouraged (use the tool's approval system)

## User Hooks

Personal hooks that run for all repositories. These don't require approval and run before project hooks.

```toml
[post-create]
setup = "echo 'Setting up worktree...'"

[pre-merge]
notify = "notify-send 'Merging {{ branch }}'"
```

User hooks support the same hook types and template variables as project hooks. Use `--no-verify` to skip.

**Key differences from project hooks:**

| Aspect | User hooks | Project hooks |
|--------|------------|--------------|
| Location | `~/.config/worktrunk/config.toml` | `.config/wt.toml` |
| Scope | All repositories | Single repository |
| Approval | Not required | Required |
| Execution order | Before project hooks | After user hooks |

**Filtering by repository:**

User hooks receive JSON context on stdin, enabling repository-specific behavior:

```toml
[post-switch]
notify = """
python3 -c '
import json, sys
ctx = json.load(sys.stdin)
if "work-project" in ctx.get("repo", ""):
    print(f"Switched to work project: {ctx[\"branch\"]}")
'
"""
```

## Command Settings

Configure default flag values for commands. These apply unless explicitly overridden.

### `[list]` section

```toml
[list]
full = false       # Show CI and main-diffstat columns (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote branches (--remotes)
```

### `[commit]` section

Shared by `wt step commit`, `wt step squash`, and `wt merge`:

```toml
[commit]
stage = "all"      # What to stage: "all", "tracked", or "none"
```

### `[merge]` section

```toml
[merge]
squash = true      # Squash commits when merging
commit = true      # Commit uncommitted changes (disables squash when false)
rebase = true      # Rebase onto target before merging
remove = true      # Remove worktree after merge
verify = true      # Run project hooks
```

## Key Commands

```bash
wt config show        # View current config
wt config create      # Create initial config file
wt config --help      # Show LLM setup guide
```

## Config File Location

- **macOS/Linux**: `~/.config/worktrunk/config.toml` (or `$XDG_CONFIG_HOME/worktrunk/config.toml`)
- **Windows**: `%APPDATA%\worktrunk\config.toml`

## Environment Variable Overrides

All config options can be overridden via environment variables with the `WORKTRUNK_` prefix:

- Single underscore after prefix: `WORKTRUNK_WORKTREE_PATH`
- Double underscore for nested keys: `WORKTRUNK_COMMIT_GENERATION__COMMAND`
- Snake_case env vars convert to kebab-case config keys automatically

```bash
# Override LLM in CI/testing
WORKTRUNK_COMMIT_GENERATION__COMMAND=echo \
WORKTRUNK_COMMIT_GENERATION__ARGS="test: mock commit" \
  wt merge
```

Array values like `args` can be specified as single strings in env vars.

## Example Config

See `dev/config.example.toml` in the worktrunk repository for a complete annotated example.
