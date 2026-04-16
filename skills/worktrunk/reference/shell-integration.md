# Shell Integration Reference

How Worktrunk's shell integration works and how to debug issues.

## Why Shell Integration Exists

Subprocesses cannot change the parent shell's current directory. When
`wt switch feature` runs, the `wt` binary runs as a child process and cannot
`cd` the terminal.

Worktrunk solves this with **split directive file passing**:

1. Shell wrapper creates two temp files via `mktemp` (one for cd, one for exec)
2. Shell sets `WORKTRUNK_DIRECTIVE_CD_FILE` and `WORKTRUNK_DIRECTIVE_EXEC_FILE`
3. `wt` binary writes a raw path to the CD file (no shell escaping needed)
4. `wt` writes shell commands to the EXEC file (only for `--execute`)
5. Shell reads the CD file with `cd -- "$(< file)"` — no shell parsing
6. Shell sources the EXEC file if non-empty
7. Shell removes both temp files

The split design eliminates shell injection from cd directives — the CD file
holds a raw path that is never parsed as shell.

## Installation

```bash
# Auto-install for all shells (bash, zsh, fish, nushell (experimental), PowerShell)
wt config shell install

# Or manual installation - add to the shell config:
# bash (~/.bashrc):
eval "$(wt config shell init bash)"

# zsh (~/.zshrc):
eval "$(wt config shell init zsh)"

# fish (~/.config/fish/config.fish):
wt config shell init fish | source

# nushell (experimental) — save to vendor autoload directory:
wt config shell init nu | save -f ($nu.default-config-dir | path join vendor/autoload/wt.nu)

# PowerShell ($PROFILE):
Invoke-Expression (& wt config shell init powershell | Out-String)
```

## Checking Status

```bash
# Show shell integration status
wt config show
```

The RUNTIME section shows whether shell integration is active for the current
session.

## Warning Messages

When shell integration isn't working, `wt switch` shows warnings explaining why.

### "shell integration not installed"

**Meaning**: The shell config file doesn't have the `eval "$(wt config shell init ...)"` line.

**Fix**: Run `wt config shell install` or add the line manually.

### "shell requires restart"

**Meaning**: Shell integration is configured, but the current shell session was
started before installation. The shell function isn't loaded yet.

**Fix**: Start a new terminal or run `source ~/.bashrc` (or equivalent).

### "ran ./path/to/wt; shell integration wraps wt"

**Meaning**: The binary was invoked with an explicit path (like `./target/debug/wt`
or `/usr/local/bin/wt`) instead of just `wt`. The shell wrapper only intercepts
the bare command `wt`.

**Fix**: Use `wt` without a path. For testing dev builds, set `WORKTRUNK_BIN`:
```bash
export WORKTRUNK_BIN=./target/debug/wt
wt switch feature  # Now uses the dev build with shell integration
```

### "ran git wt; running through git prevents cd"

**Meaning**: `git wt` (git alias) was used instead of `wt`. Git runs worktrunk as
a subprocess, bypassing the shell wrapper.

**Fix**: Use `wt` directly instead of `git wt` when directory switching is needed.

### "Alias bypasses shell integration"

**Meaning**: An alias like `alias gwt="/usr/bin/wt"` or `alias gwt="wt.exe"`
points directly to the binary instead of the shell function.

When shell integration is installed, it creates a shell function named `wt` (or
`git-wt`). If the alias points to the binary path, it bypasses this function
and shell integration won't work.

**Examples that bypass** (won't auto-cd):
```bash
alias gwt="/usr/bin/wt"
alias gwt="wt.exe"
alias wt="/path/to/wt"
```

**Fix**: Change the alias to point to the function name instead of the binary:
```bash
alias gwt="wt"       # Good - uses the shell function
alias gwt="git-wt"   # Good - uses the shell function
```

`wt config show` detects these problematic aliases and shows a warning with the
suggested fix.

## How the Shell Wrapper Works

The shell wrapper (installed by `wt config shell install`) defines a shell
function that:

1. Creates two temp files (cd and exec)
2. Sets `WORKTRUNK_DIRECTIVE_CD_FILE` and `WORKTRUNK_DIRECTIVE_EXEC_FILE`
3. Runs the real `wt` binary
4. Reads the CD file with `cd -- "$(< file)"` (raw path, no shell parsing)
5. Sources the EXEC file if non-empty (for `--execute` payloads)
6. Cleans up both temp files

Simplified example (actual wrapper handles completions and edge cases):
```bash
wt() {
    local cd_file exec_file exit_code=0
    cd_file="$(mktemp)"
    exec_file="$(mktemp)"

    WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" WORKTRUNK_DIRECTIVE_EXEC_FILE="$exec_file" \
        command wt "$@" || exit_code=$?

    if [[ -s "$cd_file" ]]; then
        cd -- "$(<"$cd_file")"
    fi
    if [[ -s "$exec_file" ]]; then
        source "$exec_file"
    fi

    rm -f "$cd_file" "$exec_file"
    return "$exit_code"
}
```

## Debugging Checklist

### 1. Check if wrapper is installed

```bash
# Should show shell function, not binary path
type wt

# Expected output (bash/zsh):
# wt is a function
# wt () { ... }

# If it shows a path like /usr/local/bin/wt, wrapper isn't loaded
```

### 1b. Check if wrapper is installed (PowerShell)

```powershell
# PowerShell: should show Function, not just Application
Get-Command wt -All

# Expected output when wrapper is loaded:
# CommandType  Name  Source
# -----------  ----  ------
# Function     wt
# Application  wt    C:\Users\...\wt.exe

# If only Application appears, wrapper isn't loaded (restart shell)
# If Function appears but integration is still "not active", check the body:
(Get-Command wt -CommandType Function).ScriptBlock | Select-String WORKTRUNK
```

### 2. Check shell config file

```bash
# bash
grep -n "wt config shell init" ~/.bashrc

# zsh
grep -n "wt config shell init" ~/.zshrc

# fish
grep -n "wt config shell init" ~/.config/fish/config.fish
```

Should show the `eval` line with line number.

### 3. Check if directive files are set

```bash
# After running any wt command, these should be unset (temp files deleted)
echo $WORKTRUNK_DIRECTIVE_CD_FILE
echo $WORKTRUNK_DIRECTIVE_EXEC_FILE

# During wt execution, these would be set to temp file paths
```

### 4. Test directive files manually

```bash
# Create temp files and test
export WORKTRUNK_DIRECTIVE_CD_FILE=$(mktemp)
export WORKTRUNK_DIRECTIVE_EXEC_FILE=$(mktemp)
command wt switch feature
cat $WORKTRUNK_DIRECTIVE_CD_FILE     # Should contain: /path/to/worktree (raw path)
cd -- "$(<$WORKTRUNK_DIRECTIVE_CD_FILE)"  # Should cd you there
rm -f $WORKTRUNK_DIRECTIVE_CD_FILE $WORKTRUNK_DIRECTIVE_EXEC_FILE
```

## Common Issues

### Shell integration works in terminal but not in IDE terminal

IDE terminals may use different shell configs. Check:
- VS Code: Settings → Terminal → Integrated → Shell Args
- The IDE terminal might source a different profile

### Completions not working

Completions are installed alongside shell integration. If they're missing:

```bash
# Reinstall (forces regeneration)
wt config shell install

# For zsh, you may need compinit before the wt line:
autoload -Uz compinit && compinit
eval "$(wt config shell init zsh)"
```

### Windows Git Bash issues

Git Bash uses MSYS2, which automatically converts POSIX paths in environment
variables. The directive file path is handled correctly without manual conversion.

If you see path issues, ensure you're using a recent Git for Windows version.

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_DIRECTIVE_CD_FILE` | Set by shell wrapper; wt writes a raw path, wrapper `cd`s to it |
| `WORKTRUNK_DIRECTIVE_EXEC_FILE` | Set by shell wrapper; wt writes shell commands, wrapper sources the file |
| `WORKTRUNK_BIN` | Override binary path (for testing dev builds) |
| `WORKTRUNK_SHELL` | Set by PowerShell wrapper to indicate shell type |

## See Also

- `wt config shell --help` — Shell integration commands
- `wt config show` — View current configuration and status
