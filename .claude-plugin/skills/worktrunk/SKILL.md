---
name: worktrunk-config
description: This skill should be used when helping users configure Worktrunk, either personal settings (user config at ~/.config/worktrunk/config.toml) or project-specific hooks (project config at .config/wt.toml). Use when users ask to "set up LLM", "configure hooks", "automate npm install", or similar configuration tasks.
---

# Worktrunk Configuration

This skill guides configuration of Worktrunk for both personal settings and project-specific automation.

## Two Types of Configuration

Worktrunk uses two separate config files with different scopes and behaviors:

### User Config (`~/.config/worktrunk/config.toml`)
- **Scope**: Personal preferences for the individual developer
- **Location**: `~/.config/worktrunk/config.toml` (never checked into git)
- **Contains**: LLM integration, worktree path templates, approved commands
- **Permission model**: Always propose changes and get consent before editing
- **See**: `references/user-config.md` for detailed guidance

### Project Config (`.config/wt.toml`)
- **Scope**: Team-wide automation shared by all developers
- **Location**: `<repo>/.config/wt.toml` (checked into git)
- **Contains**: Hooks for worktree lifecycle (post-create, pre-merge, etc.)
- **Permission model**: Proactive (create directly, changes are reversible via git)
- **See**: `references/project-config.md` for detailed guidance

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
   llm models default claude-3.5-sonnet
   ```

4. **Propose config change**
   ```toml
   [commit-generation]
   command = "llm"
   ```
   Ask: "Should I add this to your config?"

5. **After approval, apply**
   - Check if config exists: `wt config list`
   - If not, guide through `wt config init`
   - Read, modify, write preserving structure

6. **Suggest testing**
   ```bash
   llm "say hello"
   wt merge  # in a repo with uncommitted changes
   ```

**See `references/user-config.md` for complete details.**

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

3. **Design appropriate hooks**
   - Dependencies (fast, must complete) → `post-create-command`
   - Tests/linting (must pass) → `pre-commit-command` or `pre-merge-command`
   - Long builds → `post-start-command`

4. **Validate commands work**
   ```bash
   npm run lint  # verify exists
   which cargo   # verify tool exists
   ```

5. **Create `.config/wt.toml`**
   ```toml
   # Install dependencies when creating worktrees
   post-create-command = "npm install"

   # Validate code quality before committing
   pre-commit-command = ["npm run lint", "npm run typecheck"]

   # Run tests before merging
   pre-merge-command = "npm test"
   ```

6. **Add comments explaining choices**

7. **Suggest testing**
   ```bash
   wt switch --create test-hooks
   ```

**See `references/project-config.md` for complete details.**

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
- Set up LLM integration → `references/user-config.md#llm-setup`
- Customize worktree paths → `references/user-config.md#worktree-paths`
- Custom commit templates → `references/user-config.md#templates`
- Troubleshoot LLM issues → `references/user-config.md#troubleshooting`

### Project Config Tasks
- Set up hooks for new project → `references/project-config.md#new-project`
- Add hook to existing config → `references/project-config.md#add-hook`
- Use template variables → `references/project-config.md#variables`
- Convert command formats → `references/project-config.md#formats`
- Troubleshoot hook execution → `references/project-config.md#troubleshooting`

## Key Commands

```bash
# View all configuration
wt config list

# Create initial user config
wt config init

# LLM setup guide
wt config --help
```

## When to Load Reference Files

Reference files contain detailed procedures. Load them when:
- User asks about specific configuration aspects
- Troubleshooting configuration issues
- Need detailed examples or validation rules
- Want comprehensive hook type reference

Use grep patterns to find specific sections:
```bash
# Find LLM setup details
grep -A 20 "## LLM Setup" references/user-config.md

# Find hook type details
grep -A 30 "### post-create-command" references/project-config.md
```
