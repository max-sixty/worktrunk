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

## Does Worktrunk support stacked branches?

Not natively — stacked-branch workflows are a large design space, so Worktrunk treats them as an extension rather than a built-in. [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync) is a community tool that auto-detects the branch dependency tree from git history and rebases each branch onto its parent in topological order. Install with `cargo install worktrunk-sync` and run as `wt sync` (via [custom subcommands](@/extending.md#custom-subcommands)).

## How do I move uncommitted changes to a new worktree?

Stash the changes, create the worktree, then pop:

{% terminal() %}
<span class="cmd">git stash push -u           # -u also stashes untracked files</span>
<span class="cmd">wt switch --create feature  # new branch off the default branch</span>
<span class="cmd">git stash pop               # changes reappear in the new worktree</span>
{% end %}

The stash lives in the shared `.git` directory, so it's reachable from the new worktree. The original branch is left clean.

`wt switch --create` bases the new branch on the default branch. To base it on the current commit instead, pass `--base=@` (needed when the current branch has commits beyond the default branch).

## There's an issue with my shell setup

If shell integration isn't working (auto-cd not happening, completions missing, `wt` not found as a function), the fastest path to a fix is using Claude Code with the Worktrunk plugin:

1. Install the [Worktrunk plugin](@/claude-code.md) in Claude Code
2. Ask Claude to debug the Worktrunk shell integration

Claude will run `wt config show`, inspect the shell config files, and identify the issue.

If Claude can't fix it, please [open an issue](https://github.com/max-sixty/worktrunk/issues/new?title=Shell%20setup%20issue&body=%23%23%20Shell%20and%20OS%0A%0A-%20Shell%3A%20%0A-%20OS%3A%20%0A%0A%23%23%20Output%20of%20%60wt%20config%20show%60%0A%0A%60%60%60%0A%0A%60%60%60%0A%0A%23%23%20What%20Claude%20found%20%28if%20available%29%0A%0A) with the output of `wt config show`, the shell (bash/zsh/fish), and OS. (And even if it fixes the problem, feel free to open an issue: non-standard success cases are useful for ensuring Worktrunk is easy to set up for others.)

## What does `-v` / `-vv` do?

Three verbosity levels. Each is a superset of the previous one.

| Level | Stderr | Files (`.git/wt/logs/`) | Use case |
|-------|--------|-------------------------|----------|
| (none) | Warnings only | — | Normal use |
| `-v` | + Info: hook output, alias template variable resolution | — | Debugging hooks/aliases |
| `-vv` | Same as `-v` | + `trace.log`, `trace.jsonl`, `subprocess.log`, `diagnostic.md` | Filing a bug |

At `-vv`, debug-level records (command lines, in-process spans, bounded subprocess preview) route to `trace.log` instead of stderr — so the terminal stays readable while the deep trace lands on disk. A one-line pointer on stderr shows where the files went.

The `-vv` files have distinct audiences:

- **`trace.log`** — the human trace (bounded ~1K lines, gistable): each command's start (`$ git status`) and finish (`✓ git status [wt]  12.3ms`, `✗` on failure), spans, and milestones.
- **`trace.jsonl`** — the same records as one JSON object per line, for machines (`jq`, chrome://tracing). `wt config state logs profile` reads it.
- **`subprocess.log`** — raw uncapped stdout/stderr of every subprocess `wt` spawns (multi-MB possible, e.g. full `git log -p` output). The deep-dive escape hatch.
- **`diagnostic.md`** — markdown bug-report bundle that leads with the performance report (subprocess time by command type, slowest calls, repeated calls — the same rendering `wt config state logs profile` produces from `trace.jsonl`) and inlines `trace.log`. `wt` prints a `gh gist create` command pointing at it.

`RUST_LOG` overrides the flag baseline when set (`RUST_LOG=debug wt -v` lifts `-v` to debug-on-stderr).

The flags only reach a command you type; shell completion runs as its own process with nowhere to pass one. Set `WORKTRUNK_VERBOSE=0|1|2` to apply the level to *every* invocation, completion included — it's the env-var equivalent of `-v`/`-vv`, so level 2 writes the same `trace.log`/`trace.jsonl`/`subprocess.log`/`diagnostic.md` files. An explicit `-v`/`-vv` on a command raises the level further but never lowers this baseline. To profile a slow tab-completion, run it the way your shell does — e.g. `WORKTRUNK_VERBOSE=2 COMPLETE=fish wt -- wt switch ''` — then render the result with `wt config state logs profile`.

## What files does Worktrunk create?

### 1. Worktree directories

Created by `wt switch <branch>` when switching to a branch that doesn't have a worktree. Use `wt switch --create <branch>` to create a new branch. Default location is `../<repo>.<branch>` (sibling to main repo), configurable via `worktree-path` in user config.

**To remove:** `wt remove <branch>` removes the worktree directory and deletes the branch.

### 2. Config files

| File | Created by | Purpose |
|------|------------|---------|
| `~/.config/worktrunk/config.toml` | `wt config create` | User preferences |
| `~/.config/worktrunk/approvals.toml` | Approving project commands | Approved hook and alias commands |
| `.config/wt.toml` | `wt config create --project` | Project hooks (checked into repo) |

User config location: `$XDG_CONFIG_HOME/worktrunk/` (or `~/.config/worktrunk/`) on Linux/macOS, `%APPDATA%\worktrunk\` on Windows.

**To remove:** Delete directly. User config: `rm ~/.config/worktrunk/config.toml`. Project config: `rm .config/wt.toml` (and commit).

### 3. Shell integration

Created by `wt config shell install`:

- **Bash**: adds line to `~/.bashrc`
- **Zsh**: adds line to `~/.zshrc` (or `$ZDOTDIR/.zshrc`)
- **Fish**: creates `~/.config/fish/functions/wt.fish` and `~/.config/fish/completions/wt.fish`
- **Nushell** <span class="badge-experimental"></span>: creates `wt.nu` in Nushell's user vendor-autoload directory — the last entry of `$nu.vendor-autoload-dirs`, under `$nu.data-dir` (typically `~/.local/share/nushell/vendor/autoload` on Linux, `~/Library/Application Support/nushell/vendor/autoload` on macOS)
- **PowerShell** (Windows): creates both profile files if they don't exist:
  - `Documents/PowerShell/Microsoft.PowerShell_profile.ps1` (PowerShell 7+)
  - `Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1` (Windows PowerShell 5.1)

**PowerShell detection on Windows:** When running from cmd.exe or PowerShell, both PowerShell profile files are created automatically. When running from Git Bash or MSYS2, PowerShell is skipped (use `wt config shell install powershell` to create the profiles explicitly).

**To remove:** `wt config shell uninstall`.

### 4. Metadata in `.git/` (automatic)

Worktrunk stores small amounts of cache and log data in the repository's `.git/` directory:

| Location | Purpose | Created by |
|----------|---------|------------|
| `git config worktrunk.*` | Cached default branch, switch history, branch markers, custom variables | Various commands |
| `.git/wt/cache/{kind}/*.json` | Cached CI status, the largest PR/MR number seen (sizes the `wt list` CI column), and git command results (merge-tree, integration probes, diff stats, ancestry checks, ahead/behind counts) | `wt list`, `wt merge`, `wt remove` |
| `.git/wt/cache/summary/{branch}/{hash}.json` | Cached LLM branch summaries, content-addressed by diff hash | `wt list --full`, `wt switch` (when `[list] summary = true`) |
| `.git/wt/logs/{branch}/**/*.log` | Background hook output (nested per branch) | Hooks, background `wt remove` |
| `.git/wt/logs/commands.jsonl` | Command audit log (~2MB max) | Hooks, LLM commands |
| `.git/wt/logs/trace.log` | Human debug trace for issue reporting | Running with `-vv` |
| `.git/wt/logs/trace.jsonl` | Machine trace (one JSON object per record) | Running with `-vv` |
| `.git/wt/logs/subprocess.log` | Raw uncapped subprocess stdout/stderr (may be multi-MB) | Running with `-vv` |
| `.git/wt/logs/diagnostic.md` | Diagnostic report for issue reporting (leads with the performance profile) | Running with `-vv` |
| `.git/wt/trash/<name>-<timestamp>` | Staged worktree contents pending background deletion | `wt remove` |

None of this is tracked by git or pushed to remotes.

**To remove:** `wt config state clear` removes all worktrunk data — config keys, caches, markers, hints, variables, logs, and stale trash.

### What Worktrunk does NOT create

- No files outside `.git/`, config directories, or worktree directories
- No global git hooks
- No modifications to `~/.gitconfig`
- No long-running background processes or daemons

## What can Worktrunk delete?

Worktrunk can delete **worktrees** and **branches**. Both have safeguards.

### Worktree removal

`wt remove` mirrors `git worktree remove`: it refuses to remove worktrees with uncommitted changes (staged, modified, or untracked files). The `--force` flag removes the worktree anyway, discarding all of those changes.

For worktrees containing precious ignored data (databases, caches, large assets), use `git worktree lock`:

{{ terminal(cmd="git worktree lock ../myproject.feature --reason __WT_QUOT__Contains local database__WT_QUOT__") }}

Locked worktrees show `⊞` in `wt list`. Neither `git worktree remove` nor `wt remove` (even with `--force`) will delete them. Unlock with `git worktree unlock`.

### Branch deletion

By default, `wt remove` only deletes branches whose content is already in the default branch. Branches showing `_` (same commit) or `⊂` (integrated) in `wt list` are safe to delete.

For the full algorithm, see [Branch cleanup](@/remove.md#branch-cleanup) — it handles squash-merge and rebase workflows where commit history differs but file changes match.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

### Other cleanup

- `wt remove` — in addition to the target worktree, runs a background internal sweep: deletes `.git/wt/trash/` entries older than 24 hours (eventual cleanup for directories orphaned when a previous background removal was interrupted), and terminates (`SIGTERM` then `SIGKILL`) `git fsmonitor--daemon` processes whose worktree no longer exists
- `wt remove` — terminates the removed worktree's `git fsmonitor--daemon` process. Git starts this per-worktree filesystem-watch daemon when `core.fsmonitor=true`; once its worktree is gone it would leak. Removal sends `git fsmonitor--daemon stop`, then resolves that daemon's PID from its IPC socket and force-terminates it (SIGTERM, then SIGKILL) if it didn't exit. The signal only ever targets the daemon whose socket resolves to the worktree being removed.
- `wt config state clear` — removes all worktrunk data from `.git/` (config keys, caches, markers, hints, variables, logs, stale trash)
- `wt config shell install` — when migrating an integration to a new location, removes the worktrunk-managed file left at the old one: fish `conf.d/wt.fish` (now `functions/wt.fish`) and nushell wrappers stranded under `<config-dir>/vendor/autoload` (now `<data-dir>/vendor/autoload`)
- `wt config shell uninstall` — removes shell integration from rc files

See [What files does Worktrunk create?](#what-files-does-worktrunk-create) for details.

## What commands does Worktrunk execute?

Worktrunk runs `git` commands internally and optionally runs `gh` (GitHub) or `glab` (GitLab) for CI status. Beyond that, user-defined commands execute in four contexts:

1. **User hooks** (`~/.config/worktrunk/config.toml`) — Personal automation for all repositories
2. **Project hooks** (`.config/wt.toml`) — Repository-specific automation
3. **LLM commands** (`~/.config/worktrunk/config.toml`) — Commit message generation and [branch summaries](@/llm-commits.md#branch-summaries)
4. **--execute flag** — Explicitly provided commands

User hooks and user aliases don't require approval (you defined them). Commands from project hooks and project aliases require approval on first run. Approved commands are saved to the approvals file (`approvals.toml`). If a command changes, Worktrunk requires new approval.

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

The `wt switch` interactive picker runs on Windows too, on [skim](https://github.com/skim-rs/skim)'s crossterm backend.

## How does Worktrunk determine the default branch?

Worktrunk checks the local git cache first, queries the remote if needed, and falls back to local inference when no remote exists.

If the remote's default branch has changed (e.g., renamed from master to main), clear the cache with `wt config state default-branch clear`.

For full details on the detection mechanism, see `wt config state default-branch --help`.

## My `for-each` or `--execute` alias prints the same value in every worktree

An alias body renders once at dispatch, in the invoking worktree's context, so a per-worktree variable like `{{ branch }}` is baked to that one worktree's value before the nested `wt` command iterates. Every worktree then sees the same value.

Confirm it with `wt config alias dry-run <name>`: if the value is already substituted (e.g. `… echo branch=main`), it was baked at dispatch.

To defer a variable to the nested command, wrap it as `{% raw %}{{ branch }}{% endraw %}`; for `wt step for-each`, also keep it inside a quoted `sh -c '…'` so the alias's shell doesn't word-split it. See [deferring expansion in an alias](@/extending.md#deferring-expansion-to-a-nested-wt-command). A repo-level variable like `{{ default_branch }}` is unaffected — it is identical in every worktree.

## Installation fails with C compilation errors

Errors related to tree-sitter or C compilation (C99 mode, `le16toh` undefined) can be avoided by installing without syntax highlighting:

{{ terminal(cmd="cargo install worktrunk --no-default-features --features cli") }}

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
