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
    ├── hooks/wt.sh                 ← canonical hook shim; Claude reaches it via
    │                                  $CLAUDE_PLUGIN_ROOT, Codex via $PLUGIN_ROOT,
    │                                  Gemini via
    │                                  ${extensionPath}/plugins/worktrunk/hooks/wt.sh
    ├── skills -> ../../skills       ← symlink; single-sources skills across all
    │                                  tools and the docs auto-sync
    ├── CLAUDE.md / README.md
    └── (Codex activity hooks live *inline* in .codex-plugin/plugin.json's
        `hooks` key — see Known Limitations below)
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

The Claude hooks track activity via git config (`worktrunk.state.{branch}.marker`):
- `UserPromptSubmit` → 🤖 (working)
- `Notification`, `PreToolUse`(`AskUserQuestion`), `PermissionRequest`, `Stop` → 💬 (waiting for input)
- `SessionEnd` → clears status

The 💬 transitions overlap deliberately: `Notification` covers the documented permission/idle path, but on platforms where it doesn't fire (VS Code extension, Windows CLI) `PermissionRequest` and `Stop` still mark the wait; `PreToolUse`(`AskUserQuestion`) catches the built-in question picker, which fires no `Notification` on any platform ([claude-code#13024](https://github.com/anthropics/claude-code/issues/13024)). There is currently no transition back to 🤖 once a turn-end/permission marker is set except a fresh `UserPromptSubmit`, so 💬 can persist into resumed work after a permission grant (the original symptom in [#2916](https://github.com/max-sixty/worktrunk/issues/2916)).

**Problem**: If the user interrupts Claude Code (Escape/Ctrl+C), the 🤖 status persists because there's no `UserInterrupt` hook. The `Stop` hook explicitly does not fire on user interrupt.

**Tracking**: [claude-code#9516](https://github.com/anthropics/claude-code/issues/9516)

### Codex activity hooks (marker persists after session end)

The Claude manifest carries `hooks: "./hooks/hooks.json"` (a path); the Codex manifest carries `hooks` as an **inline object**, `{ "hooks": { … } }`, embedding a Codex-tailored hooks file directly. The distinction is deliberate:

- **Why inline, not a path or an absent key.** Claude and Codex share one payload dir. If the Codex manifest omitted `hooks`, Codex would auto-discover `hooks/hooks.json` from the plugin root by convention (`DEFAULT_HOOKS_CONFIG_FILE`, the `None` branch of `load_plugin_hooks`) and surface Worktrunk's *Claude* events in a Codex session ([#3362](https://github.com/max-sixty/worktrunk/issues/3362)). An inline object takes Codex's `Some(Inline)` branch (`resolve_manifest_hooks` in `codex-rs/core-plugins/src/manifest.rs`), which overrides discovery — so the shared Claude file is never loaded and only these Codex-native events fire. Inline keeps the definition self-contained and avoids a second file that could collide with `hooks/hooks.json`.
- **Why `$PLUGIN_ROOT`, not `$CLAUDE_PLUGIN_ROOT`.** Codex exports both to hook commands (`PLUGIN_ROOT` native, `CLAUDE_PLUGIN_ROOT` as an OOTB-compat alias — `codex-rs/hooks/src/engine/discovery.rs`). The Codex file uses the native `$PLUGIN_ROOT` so nothing Claude-branded appears in a Codex session.

The events (Codex's `HookEventsToml` vocabulary, verified against `codex-rs/config/src/hook_config.rs`):
- `UserPromptSubmit` → 🤖 (working)
- `PermissionRequest`, `Stop` → 💬 (waiting for input)

`Stop` fires at turn-end (Codex added it after codex-cli 0.130.0, which had no turn-end event), so 🤖 correctly returns to 💬 when a turn completes — the transition the earlier "no turn-end event" limitation lacked.

**Limitation — marker persists after the session ends.** Codex's `HookEventsToml` has **no `SessionEnd`/session-exit event**, so there is no hook to *clear* the marker when a Codex session exits. The resting state after a normal exit is 💬 (set by the last `Stop`), which reads as "waiting for input" and lingers until the next session or a manual `wt config state marker clear`. This is the same class of limitation already documented above for Claude ("Status persists after user interrupt") — an accepted tradeoff, not a regression. If Codex later adds a session-exit event, add a `marker clear` handler for it here.

### Accepted tradeoff: shared `skills/` exposes `wt-switch-create`

Codex's `"skills": "./skills/"` and Gemini's `${extensionPath}/skills/` both resolve the entire repo-root `skills/`, including `wt-switch-create`, which depends on Claude session-cwd switching (`EnterWorktree`) that neither provides. Accepted: a tool loading a skill it can't act on is harmless, and a single repo-root `skills/` keeps the `worktrunk` skill single-source across all three tools and the docs sync. Don't add per-tool skills subtrees to exclude it.
