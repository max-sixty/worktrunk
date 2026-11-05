# Worktrunk plugin

The repository root is the marketplace for Claude Code and Codex and the Gemini extension root. Claude and Codex both install plugins/worktrunk; Codex also installs plugins/worktrunk-markers; Gemini reads the root manifest. Keep each manifest at the path its loader requires.

## Plugin layout

- Repo-local maintainer skills live in .claude/skills/; .agents/skills points there. This symlink does not work on Windows checkouts with core.symlinks=false.
- Claude discovers plugins/worktrunk/hooks/hooks.json by convention. The worktrunk Codex manifest declares an empty inline `hooks` object so Codex does not load Claude events from that same file; an absent key makes Codex read it and warn at startup. Gemini has its own root hooks/hooks.json.
- Codex hooks live in plugins/worktrunk-markers, a hooks-only plugin. Codex binds hook approval to a fingerprint of every file in the plugin directory, so hooks beside the skills would need review after each skill or docs change. Keep that directory to its manifest and shims: any change there, including a manifest `version` bump, makes every Codex user approve the hooks again. Its hooks/wt.sh and hooks/wt.cmd are real copies of the plugins/worktrunk shims, since Codex's installer drops symlinks; test_codex_markers_shims_match keeps them identical.
- Claude hooks run `wt config plugins claude hook`, which dispatches on the payload event. Keep commands literal for Anthropic's plugin directory validator, and `$CLAUDE_PLUGIN_ROOT` unbraced for fish.
- Claude and Gemini call plugins/worktrunk/hooks/wt.sh; Codex calls its copy. On Windows, Codex commandWindows calls hooks/wt.cmd to find Git Bash before running wt.sh. The command must parse in cmd.exe, Windows PowerShell, and pwsh; test it in all three shells with a plugin path containing spaces. Marker failures must not raise hook errors.
- The Codex manifest uses the native PLUGIN_ROOT variable, braced as `${PLUGIN_ROOT}` in Windows commands for Codex's textual substitution. SessionEnd has a three-second timeout, Codex's event ceiling.
- The shared skills mirror exposes wt-switch-create to Codex and Gemini, even though only Claude can switch the current session directory. Keep one authored skill tree.
- hooks/register.tsx is the plugin's one Claude Code hooks module (the early-access function hooks API), named under `modules` in hooks/hooks.json; it serves `/wt-diff`. types/index.d.ts is its state contract and tests/ its suite. Claude Code releases without modules drop the key and still load `hooks`, and Codex and Gemini never read it. Work on it with `claude --plugin-dir plugins/worktrunk`, which overrides an installed copy, and check it with `claude plugin validate plugins/worktrunk` and `claude plugin test plugins/worktrunk`.

test_plugin_layout_is_consolidated checks loader paths and layout. test_docs_are_in_sync checks the skills mirror.

### Plugin skills are a generated mirror

Repo-root skills/ is the authored set. plugins/worktrunk/skills/ is a generated, real-file mirror; edit the root and run test_docs_are_in_sync. Codex drops symlinks while installing a plugin.

## Activity markers

Claude hooks mark working on UserPromptSubmit, waiting on Notification, AskUserQuestion, PermissionRequest, and Stop, and clear on SessionEnd. A user interrupt has no hook, so the working marker can persist. Claude resolves markers from CLAUDE_PROJECT_DIR, which also leaves EnterWorktree sessions marked on their launch worktree.

Codex hooks mark working on UserPromptSubmit, waiting on PermissionRequest and Stop, and clear on SessionEnd. Codex and Gemini currently resolve the worktree from the hook process directory. When a harness exposes a stable session project directory, pass it with Worktrunk's global -C.
