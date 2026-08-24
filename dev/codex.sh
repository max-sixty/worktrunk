#!/usr/bin/env bash
# Supported forwarding entry point for Codex Cloud settings that use this path.

exec "$(dirname "${BASH_SOURCE[0]}")/../.codex/cloud.sh" "$@"
