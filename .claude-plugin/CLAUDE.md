# Claude Code Plugin Guidelines

## Plugin Structure

This plugin uses `source: "./"` in `marketplace.json`, which means:
- `marketplace.json` and `plugin.json` live in `.claude-plugin/`
- `hooks/` and `skills/` directories are at the repository root

Configuration in `.claude-plugin/marketplace.json`:
```json
{
  "source": "./",
  "skills": ["./skills/worktrunk"]
}
```

Configuration in `.claude-plugin/plugin.json`:
```json
{
  "hooks": "./hooks/hooks.json",
  "skills": ["./skills/worktrunk"]
}
```

**Why this structure**: Using `source: "./.claude-plugin"` caused EXDEV (cross-device link) errors on Linux systems where `/tmp` is on a separate filesystem (Ubuntu 21.04+, Fedora, Arch). See https://github.com/anthropics/claude-code/issues/14799 for details. Once that upstream bug is fixed, we could consolidate back to `.claude-plugin/` if desired.

## Known Limitations

### Status persists after user interrupt

The hooks track Claude Code activity via git config (`worktrunk.status.{branch}`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification` → 💬 (waiting for input)
- `SessionEnd` → clears status

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: https://github.com/anthropics/claude-code/issues/9516
