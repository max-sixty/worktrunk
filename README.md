# Worktrunk

Git worktrees solve a real problem: working on multiple branches without stashing or switching contexts. But the vanilla `git worktree` command is cumbersome. Worktrunk makes worktrees fast and seamless.

## What It Does

Worktrunk wraps git worktrees with shell integration that makes them feel native. Switching to a worktree automatically changes the shell directory. The `wt remove` command returns to the original location.

```bash
# Create and switch to a new worktree in one command
$ wt switch --create fix-auth-bug
✅ Created fix-auth-bug
  Path: /Users/you/projects/myapp.fix-auth-bug

# Your shell is already in the new worktree
$ pwd
/Users/you/projects/myapp.fix-auth-bug

# When done, remove the worktree and return to primary
$ wt remove
✅ Removed worktree: fix-auth-bug
  Returned to: /Users/you/projects/myapp
```

## Why Worktrees Matter

Traditional git workflows present painful tradeoffs:
- Stash work and switch branches (lose environment state)
- Make hasty commits just to check something else
- Clone the repo multiple times (waste disk space, create sync issues)

Worktrees enable multiple branches checked out simultaneously. Each worktree is an independent working directory sharing the same git history. Git's native interface for managing them is verbose and requires manual directory navigation.

Worktrunk provides shell integration that makes `wt switch` actually change directories. No manual `cd` commands or path tracking required.

## Installation

```bash
cargo build --release
# Copy target/release/wt to somewhere in your PATH
```

Shell integration requires adding one line to your shell config:

**Bash** (`~/.bashrc`):
```bash
eval "$(wt init bash)"
```

**Fish** (`~/.config/fish/config.fish`):
```fish
wt init fish | source
```

**Zsh** (`~/.zshrc`):
```bash
eval "$(wt init zsh)"
```

**Nushell** (`~/.config/nushell/env.nu`):
```nu
wt init nushell | save -f ~/.cache/wt-init.nu
```

Then add to `~/.config/nushell/config.nu`:
```nu
source ~/.cache/wt-init.nu
```

**PowerShell** (profile):
```powershell
wt init powershell | Out-String | Invoke-Expression
```

**Elvish** (`~/.config/elvish/rc.elv`):
```elvish
eval (wt init elvish | slurp)
```

**Xonsh** (`~/.xonshrc`):
```python
execx($(wt init xonsh))
```

**Oil Shell** (`~/.config/oil/oshrc`):
```bash
eval "$(wt init oil)"
```

## LLM-Powered Commit Messages

Worktrunk can generate commit messages using an LLM during merge operations. The LLM analyzes the staged diff and recent commit history to write messages matching the project's style.

```bash
# Merge with LLM-generated commit message
$ wt merge main --squash

# Provide custom guidance
$ wt merge main --squash -m "Focus on the authentication changes"
```

Configure the LLM command in `~/.config/worktrunk/config.toml`:

```toml
[llm]
command = "llm"  # or "claude", "gpt", etc.
args = ["-m", "claude-3-7-sonnet-20250219"]
```

The LLM receives the staged diff and recent commit messages, then generates a message following project conventions. If the LLM is unavailable or fails, worktrunk falls back to a deterministic message.

## Project Automation

Projects can define commands that run automatically when creating or switching to worktrees. Create `.config/wt.toml` in the repository root:

```toml
# Run sequentially after worktree creation (blocking)
[post-create-command]
"npm install" = "npm install --frozen-lockfile"
"build" = "npm run build"

# Run in parallel after switching (non-blocking)
[post-start-command]
"dev server" = "npm run dev"
"type check" = "npm run type-check -- --watch"

# Validation before merging (blocking, fail-fast)
[pre-merge-check]
"tests" = "npm test"
"lint" = "npm run lint"

# Run after successful merge in main worktree (blocking)
[post-merge-command]
"install" = "cargo install --path ."
"deploy" = "scripts/deploy.sh"
```

**Hook Types:**

- **`post-create-command`**: Runs sequentially after creating a worktree. Use for setup tasks like installing dependencies.
- **`post-start-command`**: Runs in parallel as background processes after switching to a worktree. Use for dev servers and watchers.
- **`pre-merge-check`**: Runs sequentially before merging. All commands must succeed for the merge to proceed. Use for validation (tests, lints).
- **`post-merge-command`**: Runs sequentially in the main worktree after a successful merge and push. Use for deployment, notifications, or updating global state.

Template variables expand at runtime:
- `{repo}` - Repository name
- `{branch}` - Current branch
- `{worktree}` - Absolute path to worktree
- `{repo_root}` - Absolute path to repository root
- `{target}` - Target branch (pre-merge-check and post-merge-command only)

### Available Hooks

Worktrunk provides three lifecycle hooks for project automation:

| Hook | When It Runs | Execution | Failure Behavior |
|------|--------------|-----------|------------------|
| **post-create-command** | After `git worktree add` completes | Sequential, blocking | Logs warning, continues with remaining commands |
| **post-start-command** | After post-create completes | Parallel, non-blocking (background processes) | Logs warning, doesn't affect switch result |
| **pre-merge-check** | Before committing/rebasing during `wt merge` | Sequential, blocking, fail-fast | Terminates merge immediately |

**Skipping hooks:**
- Use `--no-hooks` to skip all project hooks
- `wt switch --no-hooks` skips post-create and post-start
- `wt merge --no-hooks` skips pre-merge-check

**Security:**
Commands require approval on first run. Approved commands are saved globally per project. Use `--force` to bypass approval prompts.

## Customization

### Worktree Paths

By default, worktrees live as siblings to the main repo:

```
myapp/               # primary worktree
myapp.feature-x/     # secondary worktree
myapp.bugfix-y/      # secondary worktree
```

Customize the pattern in `~/.config/worktrunk/config.toml`:

```toml
# Inside the repo (keeps everything contained)
worktree-path = ".worktrees/{branch}"

# Shared directory with multiple repos
worktree-path = "../worktrees/{main-worktree}/{branch}"
```

### Fast Branch Switching

Push changes from the current worktree directly to another branch without committing or merging. Useful for moving work-in-progress code.

```bash
# Push current changes to another branch
$ wt push feature-experiment
```

Worktrunk stages the changes, creates a commit, and pushes it to the target branch's worktree if it exists.

## How Shell Integration Works

Worktrunk uses a directive protocol. Running `wt switch --internal my-branch` outputs:

```
__WORKTRUNK_CD__/path/to/worktree
Switched to worktree: my-branch
```

The shell wrapper parses this output. Lines starting with `__WORKTRUNK_CD__` trigger directory changes. Other lines print normally. This separation keeps the Rust binary focused on git logic while the shell handles environment changes.

This pattern is proven by tools like zoxide, starship, and direnv. The `--internal` flag is hidden from help output—end users never interact with it directly.

## Commands

**List worktrees:**
```bash
wt list
wt list --branches  # also show branches without worktrees
```

**Switch or create:**
```bash
wt switch feature-branch
wt switch --create new-feature
wt switch --create new-feature --base develop
wt switch feature-branch --no-hooks  # skip post-create and post-start hooks
```

**Run command after switching:**
```bash
wt switch feature-x --execute "npm test" --force
```

**Remove current worktree:**
```bash
wt remove
```

**Push changes between worktrees:**
```bash
wt push target-branch
```

**Merge into another branch:**
```bash
wt merge main                # merge commits as-is
wt merge main --squash       # squash all commits
wt merge main --keep         # keep worktree after merging
wt merge main -m "Custom message instruction"
wt merge main --no-hooks     # skip pre-merge-check hook
```

## Configuration

Global config at `~/.config/worktrunk/config.toml`:

```toml
worktree-path = "../{main-worktree}.{branch}"

[llm]
command = "llm"
args = ["-m", "claude-3-7-sonnet-20250219"]
```

Project config at `.config/wt.toml` in the repository root (see Project Automation above).

## Design Principles

**Progressive Enhancement**: Works without shell integration. Better with it.

**One Canonical Path**: No configuration flags for behavior that should just work. When there's a better way to do something, worktrunk does it that way by default.

**Fast**: Shell integration overhead is minimal. The binary shells out to git but adds negligible latency.

**Stateless**: The binary maintains no state between invocations. Shell and git are the source of truth.

## Development Status

This project is pre-release. Breaking changes are expected and acceptable. The best technical solution wins over backward compatibility.

## License

MIT
