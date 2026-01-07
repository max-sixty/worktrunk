# Troubleshooting

Claude-specific troubleshooting guidance for common worktrunk issues.

## LLM Commit Messages

### LLM command not found

```bash
$ which llm
```

If empty, the tool isn't installed or not in PATH. Install with `uv tool install -U llm`.

### LLM returns an error

Test the command directly:

```bash
$ echo "say hello" | llm
```

Common issues:
- **API key not set**: Run `llm keys set anthropic` (or `openai`)
- **Model not available**: Check model name with `llm models`
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
