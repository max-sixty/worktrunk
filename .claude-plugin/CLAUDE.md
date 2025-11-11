# Claude Code Plugin Guidelines

## Skills Directory Location

**Current approach**: Testing whether `source: "./.claude-plugin"` allows skills to remain in `.claude-plugin/skills/`

Configuration:
- `source: "./.claude-plugin"`
- `skills: ["./skills/worktrunk"]`
- Combined path: `./.claude-plugin/skills/worktrunk`
- Actual location: `.claude-plugin/skills/worktrunk/` ✅

This approach keeps skills organized with hooks in `.claude-plugin/` and avoids root directory clutter.

**Alternative**: If this doesn't work, Claude Code documentation suggests skills must be at plugin root: "All other directories (commands/, agents/, skills/, hooks/) must be at the plugin root, not inside `.claude-plugin/`"

See ../ISSUE.md for full investigation details.
