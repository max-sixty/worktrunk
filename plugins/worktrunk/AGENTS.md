# Worktrunk plugin

The repository root is the marketplace for Claude Code, Codex, and Cursor and the Gemini extension root. Claude, Codex, and Cursor all install plugins/worktrunk; Gemini reads the root manifest. Keep each manifest at the path its loader requires.

## Plugin layout

- Repo-local maintainer skills live in .claude/skills/; .agents/skills points there. This symlink does not work on Windows checkouts with core.symlinks=false.
- Claude discovers plugins/worktrunk/hooks/hooks.json by convention. The Codex manifest defines hooks inline so Codex does not load Claude events from that same file. Gemini has its own root hooks/hooks.json. The Cursor manifest points hooks at hooks/cursor-hooks.json, which replaces Cursor's conventional hooks/hooks.json discovery so Cursor never parses Claude's file.
- Every integration calls plugins/worktrunk/hooks/wt.sh. On Windows, Codex commandWindows calls hooks/wt.cmd to find Git Bash before running wt.sh. The command must parse in cmd.exe, Windows PowerShell, and pwsh; test it in all three shells with a plugin path containing spaces. Marker failures must not raise hook errors.
- The Codex manifest uses the native PLUGIN_ROOT variable, braced as `${PLUGIN_ROOT}` in Windows commands for Codex's textual substitution. SessionEnd has a three-second timeout, Codex's event ceiling.
- The shared skills mirror exposes wt-switch-create to Codex, Cursor, and Gemini, even though only Claude can switch the current session directory. Keep one authored skill tree.

test_plugin_layout_is_consolidated checks loader paths and layout. test_docs_are_in_sync checks the skills mirror.

### Plugin skills are a generated mirror

Repo-root skills/ is the authored set. plugins/worktrunk/skills/ is a generated, real-file mirror; edit the root and run test_docs_are_in_sync. Codex drops symlinks while installing a plugin.

## Activity markers

Claude hooks mark working on UserPromptSubmit, waiting on Notification, AskUserQuestion, PermissionRequest, and Stop, and clear on SessionEnd. A user interrupt has no hook, so the working marker can persist. Claude resolves markers from CLAUDE_PROJECT_DIR, which also leaves EnterWorktree sessions marked on their launch worktree.

Codex hooks mark working on UserPromptSubmit, waiting on PermissionRequest and Stop, and clear on SessionEnd. Cursor hooks mark working on beforeSubmitPrompt, waiting on stop, and clear on sessionEnd. They resolve markers from CURSOR_PROJECT_DIR through ${CURSOR_PLUGIN_ROOT}/hooks/wt.sh. Cloud Agents have no editor-lifetime sessionEnd and run against a remote checkout, so markers cover local IDE and CLI sessions only.

Codex and Gemini currently resolve the worktree from the hook process directory. When a harness exposes a stable session project directory, pass it with Worktrunk's global -C.
