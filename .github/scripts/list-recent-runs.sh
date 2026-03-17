#!/usr/bin/env bash
# Lists recently completed Claude CI runs.
#
# Fetches runs started in the lookback window, then filters to only those that
# are completed and whose updatedAt is within the window. This two-step
# approach is needed because `gh run list --created` filters by *start* time,
# not *end* time — a run started hours ago may have just finished, and a run
# started recently may still be running. See #1301 for details.
#
# Usage: list-recent-runs.sh [HOURS]
#   HOURS: lookback window in hours (default: 25, covers a full day plus buffer)
#
# Output: JSON array of {databaseId, conclusion, createdAt, updatedAt} objects.

set -euo pipefail

# Prevent gh from emitting ANSI color codes in non-TTY contexts.
export NO_COLOR=1

# Dynamically discover all claude-* workflows instead of maintaining a hardcoded list.
mapfile -t WORKFLOWS < <(gh workflow list --json name --jq '.[].name | select(startswith("claude-"))')

LOOKBACK_HOURS="${1:-25}"
CREATED_SINCE=$(date -d "${LOOKBACK_HOURS} hours ago" +%Y-%m-%dT%H:%M:%S)
COMPLETED_AFTER=$(date -d "${LOOKBACK_HOURS} hours ago" +%s)

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
