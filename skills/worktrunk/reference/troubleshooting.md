# Troubleshooting

Claude-specific troubleshooting guidance for common worktrunk issues.

## Commit Message Generation

### Command not found

Check if the configured tool is installed:

```bash
wt config show  # shows the configured command
which claude    # or: which codex, which llm, which aichat
```

If empty, install one of the supported tools. See [LLM commits docs](https://worktrunk.dev/llm-commits/) for setup instructions.

### Command returns an error

Test the configured command directly by piping a prompt to it. See `reference/llm-commits.md` for the exact command syntax for each tool.

```bash
echo "say hello" | <your-configured-command>
```

Common issues:
- **API key not set**: Each tool has its own auth mechanism
- **Model not available**: Check model name with the tool's help
- **Network issues**: Check internet connectivity

### Config not loading

1. View config path: `wt config show` shows location
2. Verify file exists: `ls -la ~/.config/worktrunk/config.toml`
3. Check TOML syntax: `cat ~/.config/worktrunk/config.toml`
4. Look for validation errors (path must be relative, not absolute)

### Template conflicts

Check for mutually exclusive options:
- `template` and `template-file` cannot both be set
- `squash-template` and `squash-template-file` cannot both be set

If a template file is used, verify it exists at the specified path.

## Hooks

### Hook not running

Check sequence:
1. Verify `.config/wt.toml` exists: `ls -la .config/wt.toml`
2. Check TOML syntax (use `wt hook show` to see parsed config)
3. Verify hook type spelling matches one of the ten types
4. Test command manually in the worktree

### Hook failing

Debug steps:
1. Run the command manually in the worktree to see errors
2. Check for missing dependencies (npm packages, system tools)
3. Verify template variables expand correctly with `wt hook show --expanded` (shows each command with its variables substituted)
4. For background hooks, check `.git/wt/logs/` for output

### Slow blocking hooks

Move long-running commands to background:

```toml
# Before — blocks for minutes
pre-start = "npm run build"

# After — fast setup, build in background
pre-start = "npm install"
post-start = "npm run build"
```

## Aliases

### Inspecting an alias

- `wt config alias show <name>` prints the raw template.
- `wt config alias dry-run <name> [-- args...]` prints the rendered command without running it.

### A `for-each` or `--execute` alias uses the same value in every worktree

The alias body renders once at dispatch, in the invoking worktree, so a per-worktree variable like `{{ branch }}` is baked before the nested `wt` command iterates. If `wt config alias dry-run <name>` shows a single substituted value (e.g. `… echo branch=main`), it was baked at that first pass. Defer it with `{% raw %}{{ branch }}{% endraw %}`, and for `for-each` keep it inside a quoted `sh -c '...'` so the alias's shell doesn't word-split it. Repo-level variables like `{{ default_branch }}` are unaffected — they are identical in every worktree. See `reference/extending.md#deferring-expansion-to-a-nested-wt-command`.

## List

### `wt list` times out after 120s

The timeout warning names the tasks that didn't finish:

```
wt list timed out after 120s (170 results received); blocked tasks:
  <branch>: working-tree-diff, working-tree-conflicts
```

Both tasks run `git status --porcelain` first. When the named worktree has `core.fsmonitor=true` and its `git fsmonitor--daemon` is wedged, `git status` blocks until the IPC attempt fails (several minutes), and the 120s drain deadline fires first.

Confirm by running `git status` in the affected worktree:

```bash
cd <worktree>
time git --no-optional-locks status --porcelain
# error: could not read IPC response   → hung daemon
```

List running daemons with their IPC socket (identifies which worktree each serves):

```bash
for pid in $(pgrep -f 'git fsmonitor--daemon'); do
  sock=$(lsof -p $pid 2>/dev/null | grep 'fsmonitor--daemon.ipc' | awk '{print $NF}' | head -1)
  printf "%6d  %s\n" "$pid" "$sock"
done
```

Sockets listed as bare `fsmonitor--daemon.ipc` (no resolved path) belong to deleted worktrees. `wt remove` reaps these as part of its internal sweep: after the primary removal output, it sends `SIGTERM` (then `SIGKILL` after a short bounded wait) to every `git fsmonitor--daemon` whose socket no longer resolves to a live worktree. That covers daemons orphaned by `wt remove` itself and by paths that bypass it (plain `git worktree remove`, manual `rm -rf`, or a crashed `wt`).

`wt remove` also force-terminates the removed worktree's own daemon as part of the synchronous teardown — it sends `git fsmonitor--daemon stop`, then resolves the daemon's PID from its IPC socket and SIGTERM/SIGKILLs it if it didn't exit. So removing a worktree never leaves a daemon behind, even one that has stopped answering its IPC.

The residual case both paths deliberately leave is a wedged daemon on a *live* worktree that is never removed: `git status` in that worktree blocks on the unresponsive IPC, but the daemon still serves a real worktree, so reaping it implicitly is out of scope. Terminate it manually: kill the daemon whose socket path matches the worktree, or `pkill -9 -f 'git fsmonitor--daemon'` and let the next `wt list` respawn the live ones. Disabling fsmonitor globally (`git config --global core.fsmonitor false`) avoids the class of problem entirely at the cost of some `git status` speed on large repos.

## PowerShell on Windows

### PowerShell profiles not created

On Windows, `wt config shell install` creates PowerShell profiles automatically when running from cmd.exe or PowerShell. It creates both:
- `Documents/PowerShell/Microsoft.PowerShell_profile.ps1` (PowerShell 7+/pwsh)
- `Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1` (Windows PowerShell 5.1)

**If running from Git Bash or MSYS2**, PowerShell is skipped because the `SHELL` environment variable is set. To create PowerShell profiles explicitly:

```bash
wt config shell install powershell
```

### Wrong PowerShell variant configured

Both profile files are created when installing from a Windows-native shell. This ensures shell integration works regardless of which PowerShell variant the user opens later. The profile files are small and harmless if unused.

### Shell integration configured but not active

When `wt config show` shows the profile line is configured but shell integration
is "not active", ask the user to run these diagnostics in the same PowerShell
session:

1. `Get-Command git-wt -All` — shows whether the wrapper Function is loaded
   alongside the Application (exe). If only Application appears, the profile
   didn't define the function (restart shell, or profile load failed).

2. `(Get-Command git-wt -CommandType Function).ScriptBlock | Select-String
   WORKTRUNK` — verifies the wrapper function body sets
   `WORKTRUNK_DIRECTIVE_CD_FILE`. If this doesn't appear, the function is
   incomplete or corrupted.

3. `Get-Command git-wt -CommandType Application | Select-Object Source` — shows
   what the wrapper resolves as `$wtBin`. If empty, the wrapper can't find the
   binary and will fail silently.

### Detection logic

Worktrunk detects Windows-native shells (cmd/PowerShell) by checking if the `SHELL` environment variable is **not** set:
- `SHELL` not set → Windows-native shell → create both PowerShell profiles
- `SHELL` set (e.g., `/usr/bin/bash`) → Git Bash/MSYS2 → skip PowerShell
