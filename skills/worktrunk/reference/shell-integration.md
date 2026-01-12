# Shell Integration Reference

How Worktrunk's shell integration works and how to debug issues.

## Why Shell Integration Exists

Subprocesses cannot change the parent shell's current directory. When you run
`wt switch feature`, the `wt` binary runs as a child process and cannot `cd`
your terminal.

Worktrunk solves this with **directive file passing**:

1. Shell wrapper creates a temp file via `mktemp`
2. Shell sets `WORKTRUNK_DIRECTIVE_FILE` env var to the temp file path
3. `wt` binary writes shell commands (like `cd '/path'`) to that file
4. Shell sources the file after `wt` exits, executing the commands
5. Shell removes the temp file

This allows `wt switch` to change your terminal's directory.

## Installation

```bash
# Auto-install for all shells (bash, zsh, fish, PowerShell)
wt config shell install

# Or manual installation - add to your shell config:
# bash (~/.bashrc):
eval "$(wt config shell init bash)"

# zsh (~/.zshrc):
eval "$(wt config shell init zsh)"

# fish (~/.config/fish/config.fish):
wt config shell init fish | source

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

**Meaning**: You invoked the binary with an explicit path (like `./target/debug/wt`
or `/usr/local/bin/wt`) instead of just `wt`. The shell wrapper only intercepts
the bare command `wt`.

**Fix**: Use `wt` without a path. For testing dev builds, set `WORKTRUNK_BIN`:
```bash
export WORKTRUNK_BIN=./target/debug/wt
wt switch feature  # Now uses your dev build with shell integration
```

### "ran git wt; running through git prevents cd"

**Meaning**: You ran `git wt` (git alias) instead of `wt`. Git runs worktrunk as
a subprocess, bypassing the shell wrapper.

**Fix**: Use `wt` directly instead of `git wt` when you need directory switching.

### "Alias bypasses shell integration"

**Meaning**: You have an alias like `alias gwt="/usr/bin/wt"` or
`alias gwt="wt.exe"` that points directly to the binary instead of the shell
function.

When shell integration is installed, it creates a shell function named `wt` (or
`git-wt`). If your alias points to the binary path, it bypasses this function
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

1. Creates a temp file for directives
2. Sets `WORKTRUNK_DIRECTIVE_FILE` to that path
3. Runs the real `wt` binary
4. Sources the directive file (executes `cd` commands)
5. Cleans up the temp file

Simplified example (actual wrapper handles completions and edge cases):
```bash
wt() {
    local directive_file
    directive_file="$(mktemp)"

    WORKTRUNK_DIRECTIVE_FILE="$directive_file" command wt "$@" || exit_code=$?

    if [[ -s "$directive_file" ]]; then
        source "$directive_file"
    fi

    rm -f "$directive_file"
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

### 3. Check if directive file is set

```bash
# After running any wt command, this should be unset (temp file deleted)
echo $WORKTRUNK_DIRECTIVE_FILE

# During wt execution, this would be set to a temp file path
```

### 4. Test directive file manually

```bash
# Create a temp file and test
export WORKTRUNK_DIRECTIVE_FILE=$(mktemp)
command wt switch feature
cat $WORKTRUNK_DIRECTIVE_FILE  # Should contain: cd '/path/to/worktree'
source $WORKTRUNK_DIRECTIVE_FILE  # Should cd you there
rm $WORKTRUNK_DIRECTIVE_FILE
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
| `WORKTRUNK_DIRECTIVE_FILE` | Set by shell wrapper; path to temp file for directives |
| `WORKTRUNK_BIN` | Override binary path (for testing dev builds) |
| `WORKTRUNK_SHELL` | Set by PowerShell wrapper to indicate shell type |

## See Also

- `wt config shell --help` — Shell integration commands
- `wt config show` — View current configuration and status
