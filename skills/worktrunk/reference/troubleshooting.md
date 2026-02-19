# Troubleshooting

Claude-specific troubleshooting guidance for common worktrunk issues.

## Commit Message Generation

### Command not found

Check if the configured tool is installed:

```bash
$ wt config show  # shows the configured command
$ which claude    # or: which codex, which llm, which aichat
```

If empty, install one of the supported tools. See [LLM commits docs](https://worktrunk.dev/llm-commits/) for setup instructions.

### Command returns an error

Test the configured command directly by piping a prompt to it. See `reference/llm-commits.md` for the exact command syntax for each tool.

```bash
$ echo "say hello" | <your-configured-command>
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
3. Verify hook type spelling matches one of the seven types
4. Test command manually in the worktree

### Hook failing

Debug steps:
1. Run the command manually in the worktree to see errors
2. Check for missing dependencies (npm packages, system tools)
3. Verify template variables expand correctly (`wt hook show --verbose`)
4. For background hooks, check `.git/wt-logs/` for output

### Slow blocking hooks

Move long-running commands to background:

```toml
# Before — blocks for minutes
post-create = "npm run build"

# After — fast setup, build in background
post-create = "npm install"
post-start = "npm run build"
```

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
   `WORKTRUNK_DIRECTIVE_FILE`. If this doesn't appear, the function is
   incomplete or corrupted.

3. `Get-Command git-wt -CommandType Application | Select-Object Source` — shows
   what the wrapper resolves as `$wtBin`. If empty, the wrapper can't find the
   binary and will fail silently.

### Detection logic

Worktrunk detects Windows-native shells (cmd/PowerShell) by checking if the `SHELL` environment variable is **not** set:
- `SHELL` not set → Windows-native shell → create both PowerShell profiles
- `SHELL` set (e.g., `/usr/bin/bash`) → Git Bash/MSYS2 → skip PowerShell
