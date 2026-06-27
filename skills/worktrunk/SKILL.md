---
name: worktrunk
description: Guidance for Worktrunk (the `wt` CLI) — git worktree management, hooks, and config. Load when editing .config/wt.toml or ~/.config/worktrunk/config.toml; adding, modifying, or debugging hooks (post-merge, post-start, pre-commit, pre-merge, post-switch, etc.); configuring commit message generation or command aliases; or troubleshooting wt behavior. Also answers general worktrunk/wt questions.
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev)
---

# Worktrunk

Help users work with Worktrunk, a CLI tool for managing git worktrees.

## Available Documentation

Reference files are synced from [worktrunk.dev](https://worktrunk.dev) documentation:

- **reference/config.md**: User and project configuration (LLM, hooks, command defaults)
- **reference/hook.md**: Hook types, timing, and execution order
- **reference/switch.md**, **merge.md**, **list.md**, etc.: Command documentation
- **reference/extending.md**: Aliases, multi-step pipelines, custom subcommands, and template-expansion gotchas (two-pass `{% raw %}` deferral, for-each recipes)
- **reference/llm-commits.md**: LLM commit message generation
- **reference/tips-patterns.md**: Practical recipes — aliases, per-branch variables, dev server per worktree, parallel agent patterns
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
- **Contains**: Hooks for worktree lifecycle (pre-start, pre-merge, etc.)
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

**Both configs may be needed**: For example, setting up commit message generation requires user config, but automating quality checks requires project config.

## Core Workflows

### Setting Up Commit Message Generation (User Config)

Most common request. See `reference/llm-commits.md` for supported tools and exact command syntax.

1. **Detect available tools**
   ```bash
   which claude codex llm aichat 2>/dev/null
   ```

2. **If none installed, recommend Claude Code** (already available in Claude Code sessions)

3. **Propose config change** — Get the exact command from `reference/llm-commits.md`
   ```toml
   [commit.generation]
   command = "..."  # see reference/llm-commits.md for tool-specific commands
   ```
   Ask: "Should I add this to your config?"

4. **After approval, apply**
   - Check if config exists: `wt config show`
   - If not, guide through `wt config create`
   - Read, modify, write preserving structure

5. **Suggest testing**
   ```bash
   wt step commit --show-prompt | head  # verify prompt builds
   wt merge  # in a repo with uncommitted changes
   ```

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

3. **Design appropriate hooks** (10 hook types: 5 events × pre/post — see `reference/hook.md`)
   - Dependencies (fast, must complete) → `pre-start`
   - Tests/linting (must pass) → `pre-commit` or `pre-merge`
   - Long builds, dev servers → `post-start`
   - Setup before branch resolution → `pre-switch`
   - Terminal/IDE updates → `post-switch`
   - After-commit triggers (CI, notifications) → `post-commit`
   - Deployment → `post-merge`
   - Cleanup before removal → `pre-remove`
   - Cleanup after removal (stop servers, remove containers) → `post-remove`

4. **Validate commands work**
   ```bash
   npm run lint  # verify exists
   which cargo   # verify tool exists
   ```

5. **Create `.config/wt.toml`**
   ```toml
   # Install dependencies when creating worktrees
   pre-start = "npm install"

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

2. **Determine hook type** - When should this run? (10 types: 5 events × pre/post)
   - Creating worktree (blocking) → `pre-start`
   - Creating worktree (background) → `post-start`
   - Before/after a switch → `pre-switch` / `post-switch`
   - Before committing → `pre-commit`
   - After committing (CI, notifications) → `post-commit`
   - Before merging → `pre-merge`
   - After merging → `post-merge`
   - Before/after removal → `pre-remove` / `post-remove`

3. **Handle format conversion if needed**

   Single command to a pipeline of dependent steps:
   ```toml
   # Before
   pre-start = "npm install"

   # After (adding db:migrate, which needs install to finish first)
   [[pre-start]]
   install = "npm install"

   [[pre-start]]
   migrate = "npm run db:migrate"
   ```

   For independent commands, a named table runs them concurrently:
   ```toml
   [pre-start]
   install = "npm install"
   env = "cp .env.example .env"
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
- Set up commit message generation → `reference/llm-commits.md`
- Customize worktree paths → `reference/config.md#worktree-path-template`
- Custom commit templates → `reference/llm-commits.md#prompt-templates`
- Configure command defaults → `reference/config.md#command-config`
- Set up personal hooks → `reference/config.md#hooks`

### Project Config Tasks
- Set up hooks for new project → `reference/hook.md`
- Add hook to existing config → `reference/hook.md#hook-forms`
- Use template variables → `reference/hook.md#template-variables`
- Add dev server URL to list → `reference/config.md#dev-server-url`

### Aliases & Multi-Worktree Tasks
- Create a `wt` alias → `reference/extending.md#aliases`
- Run a command in every worktree → `reference/step.md#wt-step-for-each`
- Rebase every worktree (up-style) → `reference/extending.md#recipe-rebase-every-worktree-onto-its-upstream`
- Defer a template variable to a nested `wt` command → `reference/extending.md#deferring-expansion-to-a-nested-wt-command`

## Key Commands

```bash
# View all configuration
wt config show

# Create initial user config (LLM/commit setup: see reference/llm-commits.md)
wt config create

# Full config reference (subcommands, templates, env vars)
wt config --help
```

## Loading Additional Documentation

Load **reference files** for detailed configuration, hook specifications, and troubleshooting.

Find specific sections with grep:
```bash
grep -A 20 "## Setup" reference/llm-commits.md
grep -A 30 "### pre-start" reference/hook.md
grep -A 20 "## Warning Messages" reference/shell-integration.md
```

## Hook Approvals in Non-Interactive Sessions

Worktrunk never runs a project's hooks or aliases until the user has explicitly approved them. The commands in `.config/wt.toml` are arbitrary shell code shipped in a repository the user may have just cloned, so on first run Worktrunk shows each command and waits for the user to approve it — an untrusted `.config/wt.toml` cannot silently execute anything. Approvals are stored per-project in `~/.config/worktrunk/approvals.toml` and re-prompted whenever a command template changes, so a hook can't be swapped for a different command after it was approved.

Agents running `wt merge`, `wt switch`, or other commands that trigger hooks will hit an error like:

```
▲ cargo-difftest needs approval to execute 1 command:
○ post-merge install:
  cargo install --path .
✗ Cannot prompt for approval in non-interactive environment
↳ To skip prompts in CI/CD, add --yes; to pre-approve commands, run wt config approvals add
```

The resolution is for the user to make the trust decision themselves:

- **`wt config approvals add`** — interactive prompt where the user reviews each command before it is stored to `~/.config/worktrunk/approvals.toml`. Run once per project; the approval persists across invocations until the command template changes or the project moves. This is the path to recommend — the user reviews and consents to exactly the commands that will run.

**When invoked as an agent, stop and escalate to the user.** Approving a project's hooks is a security decision about whether this repository should be trusted to run arbitrary commands on the user's machine — that decision belongs to the user, not the agent. Tell the user to run `wt config approvals add` and let them review the commands. Do not run `--yes` on the user's behalf: it skips the approval gate for that invocation, so reaching for it to unblock a command defeats the protection. `--yes` exists for CI/CD pipelines that already control their own hook contents; it is not a shortcut for an interactive agent to silence an approval prompt.

## Advanced: Agent Handoffs

When the user requests spawning a worktree with an agent in a background session ("spawn a worktree for...", "hand off to another agent"), use the appropriate pattern for their terminal multiplexer. Substitute `<agent-cli>` with the CLI you are running as: `claude` for Claude Code, `'opencode run'` for OpenCode.

**tmux** (check `$TMUX` env var):
```bash
tmux new-session -d -s <branch-name> "wt switch --create <branch-name> -x <agent-cli> -- '<task description>'"
```

**Zellij** (check `$ZELLIJ` env var):
```bash
zellij run -- wt switch --create <branch-name> -x <agent-cli> -- '<task description>'
```

**Requirements** (all must be true):
- User explicitly requests spawning/handoff
- User is in a supported multiplexer (tmux or Zellij)
- The user's project instructions (`CLAUDE.md` or `AGENTS.md`) or an explicit prompt authorize this pattern

**Do not use this pattern** for normal worktree operations.

Example (tmux, Claude Code):
```bash
tmux new-session -d -s fix-auth-bug "wt switch --create fix-auth-bug -x claude -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'"
```

Example (Zellij, OpenCode):
```bash
zellij run -- wt switch --create fix-auth-bug -x 'opencode run' -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'
```

### Parallel sub-Agents (single Claude Code session)

To spawn multiple sub-Agents that each work in their own worktree from one Claude Code session — no terminal multiplexer, no human in the other pane — pre-start each worktree from the parent and pass the path into the sub-Agent prompt:

```bash
wt switch --create <branch> --no-cd --no-hooks
```

Then call the `Agent` tool **without** `isolation: "worktree"`, naming the path in the prompt:

```
You are working in `/abs/path/to/worktrunk.<branch>` on branch `<branch>`.
All edits must stay in that worktree.
```

`--no-cd` skips the shell-integration cd script the parent can't consume; `--no-hooks` is appropriate when each sub-Agent will run its own build/test step (e.g. `cargo run -- hook pre-merge --yes`) and you don't need post-start setup repeated per worktree.

**Do not** use `Agent { isolation: "worktree" }` for this. Claude Code passes its internal agent ID as `name` to the `WorktreeCreate` hook, so `wt` creates the worktree as `worktrunk.agent-<id>` on a throwaway branch. If the sub-Agent then creates a feature branch on top, you end up with non-canonical paths, orphan branches, and post-start hooks fired against the wrong branch. Pre-creating with `wt switch --create` keeps path, branch, and hook target aligned.
