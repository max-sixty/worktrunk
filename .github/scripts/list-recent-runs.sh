#!/usr/bin/env bash
# Lists recently completed Claude CI runs.
#
# Fetches runs started in the past 3 hours, then filters to only those that
# are completed and whose updatedAt is within the past hour. This two-step
# approach is needed because `gh run list --created` filters by *start* time,
# not *end* time — a run started 2h ago may have just finished, and a run
# started 50min ago may still be running. See #1301 for details.
#
# Output: JSON array of {databaseId, conclusion, createdAt, updatedAt} objects.

set -euo pipefail

# Prevent gh from emitting ANSI color codes (even in non-TTY contexts).
# CLICOLOR_FORCE overrides NO_COLOR per the clicolors spec, and Claude Code
# sets CLICOLOR_FORCE=1 — so we must unset it for NO_COLOR to take effect.
unset CLICOLOR_FORCE
export NO_COLOR=1

# Dynamically discover all claude-* workflows instead of maintaining a hardcoded list.
mapfile -t WORKFLOWS < <(gh workflow list --json name --jq '.[].name | select(startswith("claude-"))')

CREATED_SINCE=$(date -d '3 hours ago' +%Y-%m-%dT%H:%M:%S)
COMPLETED_AFTER=$(date -d '1 hour ago' +%s)

all_runs="[]"

for wf in "${WORKFLOWS[@]}"; do
  runs=$(gh run list \
    --workflow "${wf}" \
    --created ">=${CREATED_SINCE}" \
    --json databaseId,conclusion,createdAt,updatedAt \
    --limit 50 2>/dev/null || echo "[]")
  all_runs=$(echo "$all_runs" "$runs" | jq -s 'add')
done

# Filter: drop in-progress (empty conclusion), keep only recently finished
echo "$all_runs" | jq --argjson cutoff "$COMPLETED_AFTER" '
  [ .[]
    | select(.conclusion != null and .conclusion != "")
    | select((.updatedAt | fromdateiso8601) >= $cutoff)
  ]
'
