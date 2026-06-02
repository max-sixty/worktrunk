# Worktrunk Plugin Guidelines (Claude Code + Codex)

## Directory Layout

This directory (`plugins/worktrunk/`) is the Claude Code + Codex payload. Each
tool hardcodes its loader path with no fallback, so the repo root carries one
pointer per tool: Claude's and Codex's both `source → ./plugins/worktrunk`,
while Gemini resolves its extension at the repo root itself; Gemini's hooks
call the canonical `hooks/wt.sh` below.

```
worktrunk/                          ← repo root = marketplace root
├── .claude-plugin/marketplace.json ← Claude pointer  (source → ./plugins/worktrunk)
├── .agents/plugins/marketplace.json← Codex pointer   (source → ./plugins/worktrunk)
├── gemini-extension.json           ← Gemini manifest (extensionPath = repo root)
├── hooks/hooks.json                ← Gemini activity hooks (call the wt.sh below)
├── skills -> (this dir)            ← Gemini reads ${extensionPath}/skills = repo-root skills/
└── plugins/worktrunk/              ← plugin root (Claude + Codex resolve source here)
    ├── plugin.json                 ← Claude manifest (NO .claude-plugin/ wrapper —
    │                                  the wrapper is marketplace-root-only)
    ├── .codex-plugin/plugin.json   ← Codex manifest (Codex's required wrapper)
    ├── hooks/hooks.json            ← Claude activity + WorktreeCreate/Remove hooks
    ├── hooks/wt.sh                 ← canonical hook shim; Claude/Codex reach it via
    │                                  $CLAUDE_PLUGIN_ROOT, Gemini via
    │                                  ${extensionPath}/plugins/worktrunk/hooks/wt.sh
    ├── skills -> ../../skills       ← symlink; single-sources skills across all
    │                                  tools and the docs auto-sync
    ├── CLAUDE.md / README.md
    └── (Codex ships no hooks — see Known Limitations below)
```

Path resolution differs by tool, all verified end-to-end against the real CLIs:

- **Claude**: `.claude-plugin/marketplace.json` `source: "./plugins/worktrunk"`.
  Claude reads `plugins/worktrunk/plugin.json` (at the plugin root, *not* a
  `.claude-plugin/` subdir). `hooks` and `skills` paths in `plugin.json` resolve
  from the plugin root, so `./skills/worktrunk` follows the `skills` symlink to
  the repo-root `skills/worktrunk`. `$CLAUDE_PLUGIN_ROOT` is the plugin root.
- **Codex**: `.agents/plugins/marketplace.json` `source` object
  `{ "source": "local", "path": "./plugins/worktrunk" }`. Codex reads
  `plugins/worktrunk/.codex-plugin/plugin.json`. `skills: "./skills/"` resolves
  through the same symlink.
- **Gemini**: `gemini-extension.json` at the repo root; `${extensionPath}` is
  the repo root, so `${extensionPath}/skills/` is the repo-root `skills/`
  directly and `hooks/hooks.json` (repo root) calls the canonical shim at
  `${extensionPath}/plugins/worktrunk/hooks/wt.sh`. No symlink or copy.

Each Claude skill directory must be listed in `plugin.json`'s `skills` array
(Claude has no auto-discovery — `test_plugin_layout_is_consolidated` enforces
that every repo-root skill is listed); Codex and Gemini pick up the whole
`skills/` dir (accepted tradeoff — see Known Limitations below).

## Known Limitations

### Status persists after user interrupt (Claude)

The Claude hooks track activity via git config (`worktrunk.status.{branch}`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification` → 💬 (waiting for input)
- `SessionEnd` → clears status

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: [claude-code#9516](https://github.com/anthropics/claude-code/issues/9516)

### Codex ships no activity hooks

The Claude manifest carries `hooks: "./hooks/hooks.json"`; the Codex manifest has no `hooks` key and Codex ships no hooks. Codex's `HookEventNameWire` vocabulary (codex-cli 0.130.0: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `UserPromptSubmit`) has no `Stop`/turn-end event, so a 🤖 marker set on `UserPromptSubmit` could never return to 💬 — it would stick at "working" indefinitely.

Re-add a Codex `hooks.json`, the `hooks` manifest key, the install hints in `src/commands/config/codex.rs`, and the docs (`docs/content/claude-code.md` "Activity tracking", `src/cli/config.rs` plugin list) once Codex exposes a turn-end hook event.

### Accepted tradeoff: shared `skills/` exposes `wt-switch-create`

Codex's `"skills": "./skills/"` and Gemini's `${extensionPath}/skills/` both resolve the entire repo-root `skills/`, including `wt-switch-create`, which depends on Claude session-cwd switching and the `WorktreeCreate` hook neither provides. Accepted: a tool loading a skill it can't act on is harmless, and a single repo-root `skills/` keeps the `worktrunk` skill single-source across all three tools and the docs sync. Don't add per-tool skills subtrees to exclude it.
