# Claude Code Plugin Guidelines

## Skills Directory Location

**Working solution**: Using `source: "./.claude-plugin"` in `marketplace.json` allows skills to remain in `.claude-plugin/skills/` ✅

Configuration in `marketplace.json`:
```json
{
  "source": "./.claude-plugin",
  "skills": ["./skills/worktrunk"]
}
```

Configuration in `plugin.json`:
```json
{
  "hooks": "./hooks/hooks.json",
  "skills": ["./skills/worktrunk"]
}
```

**Path resolution**:
- Source base: `./.claude-plugin`
- Skills: `./.claude-plugin + ./skills/worktrunk = ./.claude-plugin/skills/worktrunk` ✅
- Hooks: `./.claude-plugin + ./hooks/hooks.json = ./.claude-plugin/hooks/hooks.json` ✅

This approach keeps all Claude Code components organized together in `.claude-plugin/` and avoids root directory clutter.

**Note**: The official Claude Code documentation states "All other directories (commands/, agents/, skills/, hooks/) must be at the plugin root" but using the `source` field to point to `./.claude-plugin` makes paths relative to that directory, allowing this organizational structure.

**Why this works**: The `source` field in `marketplace.json` changes the base directory for path resolution. When `source: "./"` (the default), skills paths are resolved from the plugin root. When `source: "./.claude-plugin"`, skills paths are resolved from `.claude-plugin/`, allowing the entire plugin to be self-contained in one directory.

## Known Limitations

### Status persists after user interrupt

The hooks track Claude Code activity via git config (`worktrunk.status.{branch}`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification` → 💬 (waiting for input)
- `SessionEnd` → clears status

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: [claude-code#9516](https://github.com/anthropics/claude-code/issues/9516)
