#!/usr/bin/env bash
# Compatibility entry point for existing Codex Cloud environment settings.

exec "$(dirname "${BASH_SOURCE[0]}")/../.codex/cloud.sh" "$@"
