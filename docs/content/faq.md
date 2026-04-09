+++
title = "FAQ"
description = "Common questions about Worktrunk: comparison to git worktree and branch switching, bare repos, TUI support, and more."
weight = 25

[extra]
group = "Reference"
+++

## How does Worktrunk compare to alternatives?

### vs. branch switching

Branch switching uses one directory: uncommitted changes from one agent get mixed with the next agent's work, or block switching entirely. Worktrees give each agent its own directory with independent files and index.

### vs. Plain `git worktree`

Git's built-in worktree commands work but require manual lifecycle management:

{% terminal() %}
# Plain git worktree workflow
<span class="cmd">git worktree add -b feature-branch ../myapp-feature main</span>
<span class="cmd">cd ../myapp-feature</span>
# ...work, commit, push...
<span class="cmd">cd ../myapp</span>
<span class="cmd">git merge feature-branch</span>
<span class="cmd">git worktree remove ../myapp-feature</span>
<span class="cmd">git branch -d feature-branch</span>
{% end %}

Worktrunk automates the full lifecycle:

{% terminal() %}
<span class="cmd">wt switch --create feature-branch  # Creates worktree, runs setup hooks</span>
# ...work...
<span class="cmd">wt merge                            # Merges into default branch, cleans up</span>
{% end %}

No cd back to main — `wt merge` runs from the feature worktree and merges into the target, like GitHub's merge button.

What `git worktree` doesn't provide:

- Consistent directory naming and cleanup validation
- Project-specific automation (install dependencies, start services)
- Unified status across all worktrees (commits, CI, conflicts, changes)

### vs. git-machete / git-town

Different scopes:

- **git-machete**: Branch stack management in a single directory
- **git-town**: Git workflow automation in a single directory
- **worktrunk**: Multi-worktree management with hooks and status aggregation

These tools can be used together—run git-machete or git-town inside individual worktrees.

### vs. Git TUIs (lazygit, gh-dash, etc.)

Git TUIs operate on a single repository. Worktrunk manages multiple worktrees, runs automation hooks, and aggregates status across branches. TUIs work inside each worktree directory.

## There's an issue with my shell setup

If shell integration isn't working (auto-cd not happening, completions missing, `wt` not found as a function), the fastest path to a fix is using Claude Code with the Worktrunk plugin:

1. Install the [Worktrunk plugin](@/claude-code.md) in Claude Code
2. Ask Claude to debug the Worktrunk shell integration

Claude will run `wt config show`, inspect the shell config files, and identify the issue.

If Claude can't fix it, please [open an issue](https://github.com/max-sixty/worktrunk/issues/new?title=Shell%20setup%20issue&body=%23%23%20Shell%20and%20OS%0A%0A-%20Shell%3A%20%0A-%20OS%3A%20%0A%0A%23%23%20Output%20of%20%60wt%20config%20show%60%0A%0A%60%60%60%0A%0A%60%60%60%0A%0A%23%23%20What%20Claude%20found%20%28if%20available%29%0A%0A) with the output of `wt config show`, the shell (bash/zsh/fish), and OS. (And even if it fixes the problem, feel free to open an issue: non-standard success cases are useful for ensuring Worktrunk is easy to set up for others.)

## What files does Worktrunk create?

Worktrunk creates files in four categories.

### 1. Worktree directories

Created by `wt switch <branch>` when switching to a branch that doesn't have a worktree. Use `wt switch --create <branch>` to create a new branch. Default location is `../<repo>.<branch>` (sibling to main repo), configurable via `worktree-path` in user config.

**To remove:** `wt remove <branch>` removes the worktree directory and deletes the branch.

### 2. Config files

| File | Created by | Purpose |
|------|------------|---------|
| `~/.config/worktrunk/config.toml` | `wt config create` | User preferences |
| `~/.config/worktrunk/approvals.toml` | Approving project commands | Approved hook commands |
| `.config/wt.toml` | `wt config create --project` | Project hooks (checked into repo) |

User config location: `$XDG_CONFIG_HOME/worktrunk/` (or `~/.config/worktrunk/`) on Linux/macOS, `%APPDATA%\worktrunk\` on Windows.

**To remove:** Delete directly. User config: `rm ~/.config/worktrunk/config.toml`. Project config: `rm .config/wt.toml` (and commit).

### 3. Shell integration

Created by `wt config shell install`:

- **Bash**: adds line to `~/.bashrc`
- **Zsh**: adds line to `~/.zshrc` (or `$ZDOTDIR/.zshrc`)
- **Fish**: creates `~/.config/fish/functions/wt.fish` and `~/.config/fish/completions/wt.fish`
- **Nushell** <span class="badge-experimental"></span>: creates `$nu.default-config-dir/vendor/autoload/wt.nu` (typically `~/.config/nushell` on Linux, `~/Library/Application Support/nushell` on macOS)
- **PowerShell** (Windows): creates both profile files if they don't exist:
  - `Documents/PowerShell/Microsoft.PowerShell_profile.ps1` (PowerShell 7+)
  - `Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1` (Windows PowerShell 5.1)

**PowerShell detection on Windows:** When running from cmd.exe or PowerShell, both PowerShell profile files are created automatically. When running from Git Bash or MSYS2, PowerShell is skipped (use `wt config shell install powershell` to create the profiles explicitly).

**To remove:** `wt config shell uninstall`.

### 4. Metadata in `.git/` (automatic)

Worktrunk stores small amounts of cache and log data in the repository's `.git/` directory:

| Location | Purpose | Created by |
|----------|---------|------------|
| `.git/config` keys under `worktrunk.*` | Cached default branch, switch history, branch markers, custom variables | Various commands |
| `.git/wt/cache/ci-status/*.json` | CI status cache (~1KB each) | `wt list` when `gh` or `glab` CLI is installed |
| `.git/wt/logs/{branch}-*.log` | Background hook output | Hooks, background `wt remove` |
| `.git/wt/logs/commands.jsonl` | Command audit log (~2MB max) | Hooks, LLM commands |
| `.git/wt/logs/verbose.log` | Debug log for issue reporting | Running with `-vv` |
| `.git/wt/logs/diagnostic.md` | Diagnostic report for issue reporting | Running with `-vv` when warnings occur |
| `.git/wt/trash/<name>-<timestamp>` | Staged worktree contents pending background deletion | `wt remove` |

None of this is tracked by git or pushed to remotes.

**To remove:** `wt config state clear` removes all worktrunk data — config keys, CI cache, markers, hints, variables, logs, and stale trash.

### What Worktrunk does NOT create

- No files outside `.git/`, config directories, or worktree directories
- No global git hooks
- No modifications to `~/.gitconfig`
- No background processes or daemons

## What can Worktrunk delete?

Worktrunk can delete **worktrees** and **branches**. Both have safeguards.

### Worktree removal

`wt remove` mirrors `git worktree remove`: it refuses to remove worktrees with uncommitted changes (staged, modified, or untracked files). The `--force` flag overrides the untracked-files check for build artifacts that weren't cleaned up.

For worktrees containing precious ignored data (databases, caches, large assets), use `git worktree lock`:

{{ terminal(cmd="git worktree lock ../myproject.feature --reason \"Contains local database\"") }}

Locked worktrees show `⊞` in `wt list`. Neither `git worktree remove` nor `wt remove` (even with `--force`) will delete them. Unlock with `git worktree unlock`.

### Branch deletion

By default, `wt remove` only deletes branches whose content is already in the default branch. Branches showing `_` (same commit) or `⊂` (integrated) in `wt list` are safe to delete.

For the full algorithm, see [Branch cleanup](@/remove.md#branch-cleanup) — it handles squash-merge and rebase workflows where commit history differs but file changes match.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

### Other cleanup

- `wt remove` — in addition to the target worktree, sweeps `.git/wt/trash/` entries older than 24 hours in the background (eventual cleanup for directories orphaned when a previous background removal was interrupted)
- `wt config state clear` — removes all worktrunk data from `.git/` (config keys, CI cache, markers, hints, variables, logs, stale trash)
- `wt config shell uninstall` — removes shell integration from rc files

See [What files does Worktrunk create?](#what-files-does-worktrunk-create) for details.

## What commands does Worktrunk execute?

Worktrunk runs `git` commands internally and optionally runs `gh` (GitHub) or `glab` (GitLab) for CI status. Beyond that, user-defined commands execute in four contexts:

1. **User hooks** (`~/.config/worktrunk/config.toml`) — Personal automation for all repositories
2. **Project hooks** (`.config/wt.toml`) — Repository-specific automation
3. **LLM commands** (`~/.config/worktrunk/config.toml`) — Commit message generation and [branch summaries](@/llm-commits.md#branch-summaries)
4. **--execute flag** — Explicitly provided commands

User hooks don't require approval (you defined them). Commands from project hooks require approval on first run. Approved commands are saved to the approvals file (`approvals.toml`). If a command changes, Worktrunk requires new approval.

### Example approval prompt

{% terminal() %}
<span class="y">▲ <b>repo</b> needs approval to execute <b>3</b> commands:</span>

<span class="d">○</span> pre-start <b>install</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">npm</span> ci</span>
<span class="d">○</span> pre-start <b>build</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">cargo</span> build <span class="c">--release</span></span>
<span class="d">○</span> pre-start <b>env</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">echo</span> <span class="g">'PORT={{ branch | hash_port }}'</span> <span class="c">></span> .env.local</span>

<span class="c">❯</span> Allow and remember? <b>[y/N]</b>
{% end %}

Use `--yes` to bypass prompts (useful for CI/automation).

### Command log

All hook executions and LLM commands are recorded in `.git/wt/logs/commands.jsonl` — one JSON object per line. Fields: `ts` (timestamp), `wt` (the wt command that triggered it), `label` (what ran, e.g., `pre-merge user:lint`), `cmd` (shell command), `exit` (exit code, `null` for background), `dur_ms` (duration, `null` for background). The file rotates to `commands.jsonl.old` at 1MB, bounding storage to ~2MB.

View the log with `wt config state logs get`, or query directly:

{% terminal() %}
# Recent commands
<span class="cmd">tail -5 .git/wt/logs/commands.jsonl | jq .</span>

# Failed commands
<span class="cmd">jq 'select(.exit != 0 and .exit != null)' .git/wt/logs/commands.jsonl</span>
{% end %}

Clear with `wt config state logs clear`.

## Does Worktrunk work on Windows?

Yes. Core commands, shell integration, and tab completion work in both Git Bash and PowerShell. See [installation](@/worktrunk.md#install) for setup details, including avoiding the Windows Terminal `wt` conflict.

**Git for Windows required** — Hooks use bash syntax and execute via Git Bash, so [Git for Windows](https://gitforwindows.org/) must be installed even when PowerShell is the interactive shell.

**`wt switch` interactive picker unavailable** — Uses [skim](https://github.com/skim-rs/skim), which doesn't support Windows. Use `wt list` and `wt switch <branch>` instead.

## How does Worktrunk determine the default branch?

Worktrunk checks the local git cache first, queries the remote if needed, and falls back to local inference when no remote exists. The result is cached for fast subsequent lookups.

If the remote's default branch has changed (e.g., renamed from master to main), clear the cache with `wt config state default-branch clear`.

For full details on the detection mechanism, see `wt config state default-branch --help`.

## Installation fails with C compilation errors

Errors related to tree-sitter or C compilation (C99 mode, `le16toh` undefined) can be avoided by installing without syntax highlighting:

{{ terminal(cmd="cargo install worktrunk --no-default-features") }}

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.

## Running tests (for contributors)

### Quick tests

{{ terminal(cmd="cargo test") }}

### Full integration tests

Shell integration tests require bash, zsh, fish, and nushell:

{{ terminal(cmd="cargo test --test integration --features shell-integration-tests") }}

## How can I contribute?

- Star the repo
- Try it out and [open an issue](https://github.com/max-sixty/worktrunk/issues) with feedback — even small annoyances
- What worktree friction does Worktrunk not yet solve? [Tell us](https://github.com/max-sixty/worktrunk/issues)
- Send to a friend
- Post about it on [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fworktrunk.dev), [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fworktrunk.dev&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management), or [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fworktrunk.dev)
