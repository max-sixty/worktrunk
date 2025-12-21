#!/usr/bin/env bash
# Mock gh CLI for demos - returns varied CI status per branch

if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi

if [[ "$1" == "pr" && "$2" == "list" ]]; then
  branch=""
  for arg in "$@"; do
    if [[ "$prev" == "--head" ]]; then
      branch="$arg"
    fi
    prev="$arg"
  done

  case "$branch" in
    main)
      echo '[{"state":"MERGED","headRefOid":"main123","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"url":"https://github.com/acme/demo/pull/100"}]'
      ;;
    alpha)
      echo '[{"state":"OPEN","headRefOid":"abc123","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"url":"https://github.com/acme/demo/pull/1"}]'
      ;;
    beta)
      echo '[{"state":"OPEN","headRefOid":"def456","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"IN_PROGRESS","conclusion":null}],"url":"https://github.com/acme/demo/pull/2"}]'
      ;;
    hooks)
      # hooks has no remote, so no PR
      echo '[]'
      ;;
    billing|api)
      echo '[{"state":"OPEN","headRefOid":"jkl012","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"url":"https://github.com/acme/demo/pull/4"}]'
      ;;
    *)
      echo '[]'
      ;;
  esac
  exit 0
fi

# gh api for check-runs (branches without PRs, like main)
# wt calls: gh api repos/.../commits/.../check-runs --jq '.check_runs | map({status, conclusion})'
# We return the post-jq result directly
if [[ "$1" == "api" && "$2" == *"check-runs"* ]]; then
  echo '[{"status":"completed","conclusion":"success"}]'
  exit 0
fi

exit 1
