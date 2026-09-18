#!/usr/bin/env bash
# Mock LLM CLI for demos — the `[commit.generation] command` every demo config
# points at.
#
# wt calls the same command for two jobs and tells them apart only by the
# prompt it pipes in, so this does the same: a summary prompt says "summary",
# a commit-message prompt doesn't. Both answers are picked from the paths in
# the diff, which is what makes each branch's row and each commit its own
# sentence rather than one canned line repeated down the column.
#
# Summaries answer instantly. The forge mock's per-branch delays already carry
# the "results stream in" story in the CI column, and `wt list` can't finish
# until every task returns — a slow answer here would just make the demo wait.

set -u
input=$(cat)

# Matches a path in the diff. The paths come from three places, all of which a
# branch can be defined in: the shared fixtures in shared/lib.py,
# `PICKER_EXTRA_BRANCHES`, and `prepare_zellij_omnibus` — which builds its own
# `api` out of `src/health.rs` where the picker's writes `src/api.rs`. A branch
# added to any of them wants a line here, or its row falls through to the
# default and claims to be a README change.
mentions() { echo "$input" | grep -q "$1"; }

if echo "$input" | grep -qi "summary"; then
    if mentions "utils\.rs"; then
        echo "Add utility functions module with string and math helpers"
    elif mentions "notes\.txt"; then
        echo "Add TODO notes for caching improvements"
    elif mentions "api\.rs\|health\.rs"; then
        echo "Add /health for load-balancer polling"
    elif mentions "auth\.rs"; then
        echo "Reissue tokens when a role changes"
    elif mentions "billing\.rs"; then
        echo "Format invoice totals per locale"
    elif mentions "cache\.rs"; then
        echo "Cache the parsed config between runs"
    elif mentions "collect\.rs"; then
        echo "Collect revisions in one batched call"
    elif mentions "docs/config\.md"; then
        echo "Document every key in the config file"
    elif mentions "CHANGELOG\.md"; then
        echo "Open the 0.2.0 changelog section"
    elif mentions "metrics\.rs"; then
        echo "Emit timings for every request"
    elif mentions "search\.rs"; then
        echo "Rank search results by recency"
    elif mentions "theme\.rs"; then
        echo "Soften the dark palette"
    elif mentions "tests/retry\.rs"; then
        echo "Retry the flaky network test"
    elif mentions "Cargo\.toml"; then
        echo "Bump tokio to the current minor"
    elif mentions "multiply\|subtract\|math"; then
        echo "Add math operations and consolidate tests"
    elif mentions "User settings"; then
        echo "Add user settings module placeholder"
    else
        echo "Expand README with contributing and license sections"
    fi
else
    # Commit message generation. The pause stands in for the model call the
    # demo is showing off, so the spinner is on screen long enough to read.
    sleep 0.5
    if mentions "test_add"; then
        echo "test: expand add coverage"
        echo ""
        echo "Add another test case for the add function."
    else
        echo "feat: add user settings module"
        echo ""
        echo "Add placeholder module for user profile settings."
    fi
fi
