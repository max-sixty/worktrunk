#!/usr/bin/env bash
# Lists today's slice of repo files for the nightly rolling survey.
#
# Every tracked code/docs file is assigned to one of 28 buckets via a
# deterministic hash of its path. Each day gets one bucket (unix day mod 28),
# so the full repo is surveyed every 28 days.
#
# Output: one file path per line.

set -euo pipefail

CYCLE_LENGTH=28
TODAY_BUCKET=$(( $(date +%s) / 86400 % CYCLE_LENGTH ))

git ls-files -- '*.rs' '*.md' '*.toml' '*.yaml' '*.yml' '*.sh' | while read -r f; do
  hash=$(echo -n "$f" | cksum | awk '{print $1}')
  bucket=$(( hash % CYCLE_LENGTH ))
  if [ "$bucket" -eq "$TODAY_BUCKET" ]; then
    echo "$f"
  fi
done
