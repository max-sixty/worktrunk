---
title: "Shell integration"
description: "How Worktrunk's shell wrapper changes the parent shell's directory, which files each shell gets, and how to debug integration that isn't working."
sidebar:
  order: 25
---
[`wt config shell`](/config/#wt-config-shell) covers setup, activation, and current-shell diagnostics. This guide covers installed files, warning messages, and wrapper behavior.

## Why shell integration exists

The [shell integration overview](/config/#wt-config-shell) explains why directory switching needs a shell function. [How the shell wrapper works](#how-the-shell-wrapper-works) describes the directive mechanism.

## Installation

See [`wt config shell install`](/config/#wt-config-shell-install) for automatic installation and [`wt config shell init`](/config/#wt-config-shell-init) for manual setup in each supported shell.

## Files created

[`wt config shell install`](/config/#wt-config-shell-install) uses these locations:

- **Bash**: `~/.bashrc`
- **Zsh**: `~/.zshrc` (or `$ZDOTDIR/.zshrc`)
- **Fish**: `~/.config/fish/functions/wt.fish` and `~/.config/fish/completions/wt.fish` (under `$XDG_CONFIG_HOME/fish` when that is set)
- **Nushell** <span class="badge-experimental"></span>: `wt.nu` in Nushell's user vendor-autoload directory — the last entry of `$nu.vendor-autoload-dirs`, under `$nu.data-dir` (typically `~/.local/share/nushell/vendor/autoload` on Linux, `~/Library/Application Support/nushell/vendor/autoload` on macOS)
- **PowerShell** (Windows): both profile files:
  - `Documents/PowerShell/Microsoft.PowerShell_profile.ps1` (PowerShell 7+)
  - `Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1` (Windows PowerShell 5.1)

**PowerShell detection on Windows:** When running from cmd.exe or PowerShell, both PowerShell profile files are created automatically. When running from Git Bash or MSYS2, PowerShell is skipped (use `wt config shell install powershell` to create the profiles explicitly).

See [`wt config shell uninstall`](/config/#wt-config-shell-uninstall) for removal.

## Checking status

The [current-shell checks](/config/#wt-config-shell) explain how to inspect installation and activation with `wt config show` and the shell's command lookup.

## Warning messages

When shell integration isn't working, `wt switch` shows a warning explaining why.

### "shell wrapper is out of date"

**Meaning**: The active shell has a wrapper from an older Worktrunk version loaded, which can't follow a directory change.

**Fix**: Run `wt config shell install`, then restart the shell (or reload its config) to activate the current wrapper.

### "shell integration not installed"

**Meaning**: The current shell has no installed Worktrunk initialization line or wrapper file. The current shell is detected from the process tree (falling back to `$SHELL`), so this refers to the shell `wt` was actually invoked from, not necessarily the login shell.

**Fix**: Run `wt config shell install` or follow the [manual setup](/config/#wt-config-shell-init).

### "shell integration installed but not active"

**Meaning**: Shell integration is configured for the current shell, but the shell function isn't loaded in this session — usually because the session was started before installation.

**Fix**: Start a new terminal or run `source ~/.bashrc` (or equivalent). If the message persists after a restart, `wt config show` reports the detected shell, `$SHELL`, and per-shell integration status.

### "ran ./path/to/wt; shell integration wraps wt"

**Meaning**: The binary was invoked with an explicit path (like `./target/debug/wt` or `/usr/local/bin/wt`) instead of just `wt`. The shell wrapper only intercepts the bare command `wt`.

**Fix**: Use `wt` without a path. For testing dev builds, set `WORKTRUNK_BIN`:

```bash
export WORKTRUNK_BIN=./target/debug/wt
wt switch feature  # Now uses the dev build with shell integration
```

### "ran git wt; running through git prevents cd"

**Meaning**: `git wt` (git alias) was used instead of `wt`. Git runs worktrunk as a subprocess, bypassing the shell wrapper.

**Fix**: Use `wt` directly instead of `git wt` when directory switching is needed.

### "Alias bypasses shell integration"

**Meaning**: An alias like `alias gwt="/usr/bin/wt"` or `alias gwt="wt.exe"` points directly to the binary instead of the shell function.

When shell integration is installed, it creates a shell function named `wt` (or `git-wt`). If the alias points to the binary path, it bypasses this function and shell integration won't work.

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

`wt config show` detects these problematic aliases and shows a warning with the suggested fix.

## How the shell wrapper works

The shell wrapper (installed by `wt config shell install`) defines a shell function that:

1. Creates a temp file
2. Sets `WORKTRUNK_DIRECTIVE_CD_FILE`
3. Runs the real `wt` binary
4. Reads the CD file with `cd -- "$(< file)"` (raw path, no shell parsing)
5. Cleans up the temp file

Simplified example (the actual wrapper also handles completions and edge cases):

```bash
wt() {
    local cd_file exit_code=0
    cd_file="$(mktemp)"

    WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" \
        command wt "$@" || exit_code=$?

    if [[ -s "$cd_file" ]]; then
        cd -- "$(<"$cd_file")"
    fi
    rm -f "$cd_file"
    return "$exit_code"
}
```

## Debugging checklist

### 1. Check whether the wrapper is loaded

```bash
# Should show a shell function, not a binary path
type wt

# Expected output (bash/zsh):
# wt is a function
# wt () { ... }

# If it shows a path like /usr/local/bin/wt, the wrapper isn't loaded
```

### 2. Check whether the wrapper is loaded (PowerShell)

```powershell
# PowerShell: should show Function, not just Application
Get-Command wt -All

# Expected output when the wrapper is loaded:
# CommandType  Name  Source
# -----------  ----  ------
# Function     wt
# Application  wt    C:\Users\...\wt.exe

# If only Application appears, the wrapper isn't loaded (restart the shell)
# If Function appears but integration is still "not active", check the body:
(Get-Command wt -CommandType Function).ScriptBlock | Select-String WORKTRUNK
```

### 3. Check the shell config file

```bash
# bash
grep -n "wt config shell init" ~/.bashrc

# zsh
grep -n "wt config shell init" ~/.zshrc
```

These commands show the initialization line and its line number. For fish and Nushell, check the wrapper files listed under [Files created](#files-created).

### 4. Check whether directive files are set

```bash
# After running any wt command, this should be unset (the temp file is deleted)
echo $WORKTRUNK_DIRECTIVE_CD_FILE

# During wt execution, this is set to a temp file path
```

### 5. Test directive files manually

```bash
# Create the temp file and test
export WORKTRUNK_DIRECTIVE_CD_FILE=$(mktemp)
command wt switch feature
cat $WORKTRUNK_DIRECTIVE_CD_FILE     # Should contain: /path/to/worktree (raw path)
cd -- "$(<$WORKTRUNK_DIRECTIVE_CD_FILE)"  # Should cd you there
rm -f $WORKTRUNK_DIRECTIVE_CD_FILE
```

## Common issues

### Shell integration works in the terminal but not in an IDE terminal

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

If you see path issues, make sure you're on a recent Git for Windows version.

## Environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_DIRECTIVE_CD_FILE` | Set by the shell wrapper; `wt` writes a raw path, the wrapper `cd`s to it |
| `WORKTRUNK_BIN` | Override the binary path (for testing dev builds) |
| `WORKTRUNK_COMPLETE_NAME` | Set by the bash, zsh, and PowerShell wrappers when they load completions; names the command the registration binds to, so `--cmd` integrations complete |

## See also

- [`wt config shell`](/config/#wt-config-shell) — setup and current-shell diagnostics
- [FAQ: What files does Worktrunk create?](/faq/#what-files-does-worktrunk-create)
