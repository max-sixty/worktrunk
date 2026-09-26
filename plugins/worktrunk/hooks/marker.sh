#!/usr/bin/env bash
# Claude Code activity-marker hook: `marker.sh set <marker>` or `marker.sh clear`.
#
# Resolves against CLAUDE_PROJECT_DIR, the directory the session launched in,
# so a shell `cd` during a turn can't retarget the marker to another
# repository (#3921). A marker failure must never surface as a hook error.
bash "$CLAUDE_PLUGIN_ROOT/hooks/wt.sh" -C "$CLAUDE_PROJECT_DIR" config state marker "$@" || true
