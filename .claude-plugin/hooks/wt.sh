#!/usr/bin/env bash
# Cross-platform wrapper for the worktrunk CLI.
# On Windows (MSYS/Cygwin), prefers git-wt.exe if available, falls back to wt.
# On other platforms, uses wt.
# Usage: wt.sh [args...]

case "$(uname -o 2>/dev/null)" in
    Msys|Cygwin)
        if command -v git-wt.exe >/dev/null 2>&1; then
            WT=git-wt.exe
        else
            WT=wt
        fi
        ;;
    *) WT=wt ;;
esac

"$WT" "$@"
