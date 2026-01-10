---
name: worktrunk
description: Guidance for Worktrunk, a CLI tool for managing git worktrees. Covers configuration (user config at ~/.config/worktrunk/config.toml and project hooks at .config/wt.toml), usage, and troubleshooting. Use for "setting up LLM", "configuring hooks", "automating tasks", or general worktrunk questions.
---

<!-- worktrunk-skill-version: 0.9.3 -->

# Worktrunk

Help users work with Worktrunk, a CLI tool for managing git worktrees.

## Available Documentation

Reference files are synced from [worktrunk.dev](https://worktrunk.dev) documentation:

- **reference/config.md**: User and project configuration (LLM, hooks, command defaults)
- **reference/hook.md**: Hook types, timing, and execution order
- **reference/switch.md**, **merge.md**, **list.md**, etc.: Command documentation
- **reference/llm-commits.md**: LLM commit message generation
- **reference/tips-patterns.md**: Language-specific tips and patterns
- **reference/shell-integration.md**: Shell integration debugging
- **reference/troubleshooting.md**: Troubleshooting for LLM and hooks (Claude-specific)

For command-specific options, run `wt <command> --help`. For configuration, follow the workflows below.

## Two Types of Configuration

Worktrunk uses two separate config files with different scopes and behaviors:

### User Config (`~/.config/worktrunk/config.toml`)
- **Scope**: Personal preferences for the individual developer
- **Location**: `~/.config/worktrunk/config.toml` (never checked into git)
- **Contains**: LLM integration, worktree path templates, command settings, user hooks, approved commands
- **Permission model**: Always propose changes and get consent before editing
- **See**: `reference/config.md` for detailed guidance

### Project Config (`.config/wt.toml`)
- **Scope**: Team-wide automation shared by all developers
- **Location**: `<repo>/.config/wt.toml` (checked into git)
- **Contains**: Hooks for worktree lifecycle (post-create, pre-merge, etc.)
- **Permission model**: Proactive (create directly, changes are reversible via git)
- **See**: `reference/hook.md` for detailed guidance

## Determining Which Config to Use

When a user asks for configuration help, determine which type based on:

**User config indicators**:
- "set up LLM" or "configure commit generation"
- "change where worktrees are created"
- "customize commit message templates"
- Affects only their environment

**Project config indicators**:
- "set up hooks for this project"
- "automate npm install"
- "run tests before merge"
- Affects the entire team

**Both configs may be needed**: For example, setting up LLM integration requires user config, but automating quality checks requires project config.

## Core Workflows

### Setting Up LLM Integration (User Config)

Most common request. Follow this sequence:

1. **Check if LLM tool exists**
   ```bash
   which llm  # or: which aichat
   ```

2. **If not installed, guide installation (don't run it)**
   ```bash
   uv tool install -U llm
   ```

3. **Guide API key setup (don't run it)**
   ```bash
   llm install llm-anthropic
   llm keys set anthropic
   llm models default claude-haiku-4.5
   ```

4. **Propose config change**
   ```toml
   [commit-generation]
   command = "llm"
   ```
   Ask: "Should I add this to your config?"

5. **After approval, apply**
   - Check if config exists: `wt config show`
   - If not, guide through `wt config create`
   - Read, modify, write preserving structure

6. **Suggest testing**
   ```bash
   llm "say hello"
   wt merge  # in a repo with uncommitted changes
   ```

**See `reference/config.md` and `reference/llm-commits.md` for complete details.**

### Setting Up Project Hooks (Project Config)

Common request for workflow automation. Follow discovery process:

1. **Detect project type**
   ```bash
   ls package.json Cargo.toml pyproject.toml
   ```

2. **Identify available commands**
   - For npm: Read `package.json` scripts
   - For Rust: Common cargo commands
   - For Python: Check pyproject.toml

3. **Design appropriate hooks** (7 hook types available)
   - Dependencies (fast, must complete) → `post-create`
   - Tests/linting (must pass) → `pre-commit` or `pre-merge`
   - Long builds, dev servers → `post-start`
   - Terminal/IDE updates → `post-switch`
   - Deployment → `post-merge`
   - Cleanup tasks → `pre-remove`

4. **Validate commands work**
   ```bash
   npm run lint  # verify exists
   which cargo   # verify tool exists
   ```

5. **Create `.config/wt.toml`**
   ```toml
   # Install dependencies when creating worktrees
   post-create = "npm install"

   # Validate code quality before committing
   [pre-commit]
   lint = "npm run lint"
   typecheck = "npm run typecheck"

   # Run tests before merging
   pre-merge = "npm test"
   ```

6. **Add comments explaining choices**

7. **Suggest testing**
   ```bash
   wt switch --create test-hooks
   ```

**See `reference/hook.md` for complete details.**

### Adding Hooks to Existing Config

When users want to add automation to an existing project:

1. **Read existing config**: `cat .config/wt.toml`

2. **Determine hook type** - When should this run?
   - Creating worktree (blocking) → `post-create`
   - Creating worktree (background) → `post-start`
   - Every switch → `post-switch`
   - Before committing → `pre-commit`
   - Before merging → `pre-merge`
   - After merging → `post-merge`
   - Before removal → `pre-remove`

3. **Handle format conversion if needed**

   Single command to named table:
   ```toml
   # Before
   post-create = "npm install"

   # After (adding db:migrate)
   [post-create]
   install = "npm install"
   migrate = "npm run db:migrate"
   ```

4. **Preserve existing structure and comments**

### Validation Before Adding Commands

Before adding hooks, validate:

```bash
# Verify command exists
which npm
which cargo

# For npm, verify script exists
npm run lint --dry-run

# For shell commands, check syntax
bash -n -c "if [ true ]; then echo ok; fi"
```

**Dangerous patterns** — Warn users before creating hooks with:
- Destructive commands: `rm -rf`, `DROP TABLE`
- External dependencies: `curl http://...`
- Privilege escalation: `sudo`

## Permission Models

### User Config: Conservative
- **Never edit without consent** - Always show proposed change and wait for approval
- **Never install tools** - Provide commands for users to run themselves
- **Preserve structure** - Keep existing comments and organization
- **Validate first** - Ensure TOML is valid before writing

### Project Config: Proactive
- **Create directly** - Changes are versioned, easily reversible
- **Validate commands** - Check commands exist before adding
- **Explain choices** - Add comments documenting why hooks exist
- **Warn on danger** - Flag destructive operations before adding

## Common Tasks Reference

### User Config Tasks
- Set up LLM integration → `reference/llm-commits.md`
- Customize worktree paths → `reference/config.md#worktree-path-template`
- Custom commit templates → `reference/llm-commits.md#templates`
- Configure command defaults → `reference/config.md#command-settings`
- Set up personal hooks → `reference/config.md#user-hooks`

### Project Config Tasks
- Set up hooks for new project → `reference/hook.md`
- Add hook to existing config → `reference/hook.md#configuration`
- Use template variables → `reference/hook.md#template-variables`
- Add dev server URL to list → `reference/config.md#dev-server-url`

## Key Commands

```bash
# View all configuration
wt config show

# Create initial user config
wt config create

# LLM setup guide
wt config --help
```

## Loading Additional Documentation

Load **reference files** for detailed configuration, hook specifications, and troubleshooting.

Find specific sections with grep:
```bash
grep -A 20 "## Setup" reference/llm-commits.md
grep -A 30 "### post-create" reference/hook.md
grep -A 20 "## Warning Messages" reference/shell-integration.md
```

## Advanced: Agent Handoffs

When the user requests spawning a worktree with Claude in a background session ("spawn a worktree for...", "hand off to another agent"), use the appropriate pattern for their terminal multiplexer:

**tmux** (check `$TMUX` env var):
```bash
tmux new-session -d -s <branch-name> "wt switch --create <branch-name> -x claude -- '<task description>'"
```

**Zellij** (check `$ZELLIJ` env var):
```bash
zellij run -- wt switch --create <branch-name> -x claude -- '<task description>'
```

**Requirements** (all must be true):
- User explicitly requests spawning/handoff
- User is in a supported multiplexer (tmux or Zellij)
- User's CLAUDE.md or explicit instruction authorizes this pattern

**Do not use this pattern** for normal worktree operations.

Example (tmux):
```bash
tmux new-session -d -s fix-auth-bug "wt switch --create fix-auth-bug -x claude -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'"
```

Example (Zellij):
```bash
zellij run -- wt switch --create fix-auth-bug -x claude -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'
```
