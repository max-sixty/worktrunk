---
name: worktrunk
description: Guidance for Worktrunk (the `wt` CLI) — git worktree management, hooks, and config. Load when editing .config/wt.toml or ~/.config/worktrunk/config.toml; adding, modifying, or debugging hooks (post-merge, post-start, pre-commit, pre-merge, post-switch, etc.); configuring commit message generation or command aliases; or troubleshooting wt behavior. Also answers general worktrunk/wt questions.
license: MIT OR Apache-2.0
compatibility: Requires the `wt` CLI (https://worktrunk.dev)
---

# Worktrunk

Help users work with Worktrunk, a CLI tool for managing git worktrees.

## Available documentation

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

## Two types of configuration

Worktrunk uses two config files with different scopes and permission models:

**User config** (`~/.config/worktrunk/config.toml`, never checked into git) holds personal preferences: LLM integration, worktree path templates, command settings, user hooks. Treat it conservatively — propose changes and get consent before editing, never install tools on the user's behalf, and preserve the file's existing structure and comments. See `reference/config.md`.

**Project config** (`<repo>/.config/wt.toml`, checked into git) holds team-wide automation: hooks for the worktree lifecycle (pre-start, pre-merge, etc.). Edit proactively — changes are versioned and reversible via git. Comment why each hook exists, and warn the user before adding destructive commands (`rm -rf`, `DROP TABLE`), network fetches piped to shells, or `sudo`. See `reference/hook.md`.

Some requests span both: commit-message generation is user config, while the team's quality checks are project config.

## Core workflows

### Setting up commit message generation (user config)

Detect which tools are installed (`which claude codex llm aichat`); if none, recommend Claude Code. Take the exact command for the chosen tool from `reference/llm-commits.md`, propose the `[commit.generation]` change, and apply it after approval (`wt config create` first if no config exists). To verify, `wt step commit --dry-run` renders the prompt, runs the LLM, and prints the message without committing.

### Configuring project hooks

Pick the hook type by when the command should run and whether it may block (10 types: 5 events × pre/post — full reference in `reference/hook.md`):

- Dependencies and env files a later step needs → `pre-start` (blocks creation)
- Dev servers, long builds, cache copying → `post-start` (background)
- Formatters, linters, type checks → `pre-commit`
- Tests that must pass before merging → `pre-merge`
- CI triggers, notifications → `post-commit`
- Deployment → `post-merge`
- Setup before branch resolution / terminal-IDE updates → `pre-switch` / `post-switch`
- Cleanup before/after removal (save artifacts; stop servers, remove containers) → `pre-remove` / `post-remove`

Derive the commands from the project itself (`package.json` scripts, `Cargo.toml`, `pyproject.toml`) and verify they run before adding them.

When a new hook must wait for an existing one, convert the entry to a pipeline; independent commands in a named table run concurrently:

```toml
# Pipeline: install completes before migrate starts
[[pre-start]]
install = "npm install"

[[pre-start]]
migrate = "npm run db:migrate"

# Concurrent: independent commands in one table
[pre-start]
install = "npm install"
env = "cp .env.example .env"
```

Test with `wt switch --create test-hooks`.

## Common tasks reference

### User config tasks
- Set up commit message generation → `reference/llm-commits.md`
- Customize worktree paths → `reference/config.md#worktree-path-template`
- Custom commit templates → `reference/llm-commits.md#prompt-templates`
- Configure command defaults → `reference/config.md#command-config`
- Set up personal hooks → `reference/config.md#hooks`

### Project config tasks
- Set up hooks for new project → `reference/hook.md`
- Add hook to existing config → `reference/hook.md#hook-forms`
- Use template variables → `reference/hook.md#template-variables`
- Add dev server URL to list → `reference/config.md#dev-server-url`

### Aliases & multi-worktree tasks
- Create a `wt` alias → `reference/extending.md#aliases`
- Run a command in every worktree → `reference/step.md#wt-step-for-each`
- Rebase every worktree (up-style) → `reference/extending.md#recipe-rebase-every-worktree-onto-its-upstream`
- Defer a template variable to a nested `wt` command → `reference/extending.md#deferring-expansion-to-a-nested-wt-command`

## Key commands

```bash
# View all configuration
wt config show

# Create initial user config (LLM/commit setup: see reference/llm-commits.md)
wt config create

# Full config reference (subcommands, templates, env vars)
wt config --help
```

## Hook approvals in non-interactive sessions

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

## Advanced: agent handoffs

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
