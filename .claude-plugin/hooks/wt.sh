#!/usr/bin/env bash
# Cross-platform wrapper for the worktrunk CLI.
# On Windows (MSYS/Cygwin), prefers git-wt.exe over wt and rejects wt if
# it resolves to Windows Terminal (WindowsApps/wt.exe).
# On other platforms, uses wt directly.
# Exits silently if git is unavailable or not inside a git repository.
# Usage: wt.sh [args...]

# bail early if git is not available or not inside a git repository
if ! command -v git >/dev/null 2>&1 || ! git rev-parse --git-dir >/dev/null 2>&1; then
    exit 0
fi

# check for bash on Windows (on Windows, Claude Code defaults to Git Bash)
if [[ "$(uname -o 2>/dev/null)" =~ ^(Msys|Cygwin)$ ]]; then
    if command -v git-wt.exe >/dev/null 2>&1; then
        # prefer git-wt over wt if  available
        WT=git-wt.exe
    elif command -v wt >/dev/null 2>&1; then
        # reject wt if it's the Windows Terminal alias
        if [[ "$(command -v wt)" == *WindowsApps* ]]; then
            echo "worktrunk: 'wt' resolves to Windows Terminal; install worktrunk as git-wt.exe or remove the Windows Terminal alias — see https://worktrunk.dev/worktrunk/#install" >&2
            exit 1
        fi

        WT=wt
    fi
else
    # non-Windows, always use wt
    WT=wt
fi

if [[ -z "$WT" ]] || ! command -v "$WT" >/dev/null 2>&1; then
    echo "worktrunk: could not find 'wt' in PATH" >&2
    exit 1
fi

"$WT" "$@"
