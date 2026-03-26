#!/usr/bin/env bash
# Cross-platform wrapper for the worktrunk CLI.
# Calls git-wt.exe on Windows, wt elsewhere.
# Usage: wt.sh [args...]

case "$(uname -o 2>/dev/null)" in
    Msys|Cygwin) WT=git-wt.exe ;;
    *)           WT=wt ;;
esac

"$WT" "$@"
