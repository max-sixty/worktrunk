#!/usr/bin/env bash
# Mock gh CLI for demo

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
    alpha)
      echo '[{"state":"OPEN","headRefOid":"abc123","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"url":"https://github.com/acme/demo/pull/1"}]'
      ;;
    beta)
      echo '[{"state":"OPEN","headRefOid":"def456","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"IN_PROGRESS","conclusion":null}],"url":"https://github.com/acme/demo/pull/2"}]'
      ;;
    hooks)
      echo '[{"state":"OPEN","headRefOid":"ghi789","mergeStateStatus":"CLEAN","statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}],"url":"https://github.com/acme/demo/pull/3"}]'
      ;;
    *)
      echo '[]'
      ;;
  esac
  exit 0
fi

if [[ "$1" == "run" && "$2" == "list" ]]; then
  branch=""
  for arg in "$@"; do
    if [[ "$prev" == "--branch" ]]; then
      branch="$arg"
    fi
    prev="$arg"
  done

  case "$branch" in
    main)
      echo '[{"status":"completed","conclusion":"success","headSha":"abc123"}]'
      ;;
    *)
      echo '[]'
      ;;
  esac
  exit 0
fi

exit 1
