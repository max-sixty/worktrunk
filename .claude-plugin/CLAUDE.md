# Claude Code Plugin Guidelines

## Directory Layout

Skills are at the repo root (`skills/worktrunk/`) — the standard plugin location.
Hooks remain in `.claude-plugin/hooks/` for now.

```
worktrunk.skills/          ← plugin root
├── .claude-plugin/
│   ├── plugin.json        ← manifest
│   └── hooks/hooks.json   ← activity tracking hooks
└── skills/
    └── worktrunk/         ← main skill + reference docs
```

Paths in `plugin.json` and `marketplace.json` resolve from the plugin root (repo root).

## Known Limitations

### Status persists after user interrupt

The hooks track Claude Code activity via git config (`worktrunk.status.{branch}`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification` → 💬 (waiting for input)
- `SessionEnd` → clears status

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: [claude-code#9516](https://github.com/anthropics/claude-code/issues/9516)
