# Worktrunk Plugin Guidelines (Claude Code + Codex)

## Directory Layout

One plugin payload, two tools. The plugin lives entirely in this directory
(`plugins/worktrunk/`); only the two loader-mandated marketplace pointers stay
at the repo root, because Claude Code and Codex each hardcode their marketplace
path with no fallback.

```
worktrunk/                          ← repo root = marketplace root
├── .claude-plugin/marketplace.json ← Claude pointer  (source → ./plugins/worktrunk)
├── .agents/plugins/marketplace.json← Codex pointer   (source → ./plugins/worktrunk)
└── plugins/worktrunk/              ← plugin root (both tools resolve source here)
    ├── plugin.json                 ← Claude manifest (NO .claude-plugin/ wrapper —
    │                                  the wrapper is marketplace-root-only)
    ├── .codex-plugin/plugin.json   ← Codex manifest (Codex's required wrapper)
    ├── hooks/hooks.json            ← Claude activity + WorktreeCreate/Remove hooks
    ├── hooks/wt.sh                  ← hook helper (referenced via ${CLAUDE_PLUGIN_ROOT})
    ├── skills -> ../../skills       ← symlink; single-sources skills across both
    │                                  plugins and the docs auto-sync
    ├── CLAUDE.md / README.md
    └── (Codex ships no hooks — see repo CLAUDE.md → "Codex Plugin")
```

Path resolution differs by tool, both verified end-to-end against the real CLIs:

- **Claude**: `.claude-plugin/marketplace.json` `source: "./plugins/worktrunk"`.
  Claude reads `plugins/worktrunk/plugin.json` (at the plugin root, *not* a
  `.claude-plugin/` subdir). `hooks` and `skills` paths in `plugin.json` resolve
  from the plugin root, so `./skills/worktrunk` follows the `skills` symlink to
  the repo-root `skills/worktrunk`. `${CLAUDE_PLUGIN_ROOT}` is the plugin root.
- **Codex**: `.agents/plugins/marketplace.json` `source` object
  `{ "source": "local", "path": "./plugins/worktrunk" }`. Codex reads
  `plugins/worktrunk/.codex-plugin/plugin.json`. `skills: "./skills/"` resolves
  through the same symlink.

Each Claude skill directory must be listed in `plugin.json`'s `skills` array;
Codex picks up the whole `skills/` dir via the symlink (accepted tradeoff — see
repo CLAUDE.md → "Codex Plugin").

## Known Limitations

### Status persists after user interrupt (Claude)

The Claude hooks track activity via git config (`worktrunk.status.{branch}`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification` → 💬 (waiting for input)
- `SessionEnd` → clears status

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: [claude-code#9516](https://github.com/anthropics/claude-code/issues/9516)

### Codex ships no activity hooks

Codex-cli 0.130.0's hook event vocabulary has no `Stop`/turn-end event, so a 🤖 marker could never return to 💬. The Codex manifest deliberately carries no `hooks` key. See repo CLAUDE.md → "Codex Plugin" for the re-enablement conditions.
