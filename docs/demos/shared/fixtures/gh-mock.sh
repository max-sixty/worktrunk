#!/usr/bin/env bash
# Mock gh CLI for demos.
#
# Every answer is a file under $HOME/.local/share/gh-mock, written by
# `write_gh_mock_data` in shared/lib.py: the first line is how long to wait
# before answering, the rest is the JSON body. The wait is what makes the CI
# column stream — each branch's `gh pr list` answers at a different moment, so
# `wt list --full` and the switch picker fill their CI cells one at a time
# instead of all at once.
#
# A request with no file answers with an empty array, which wt reads as "no PR"
# for `pr list` and "no checks" for the check-runs API.

set -u
MOCK_DIR="${HOME}/.local/share/gh-mock"

# Wait out the response file's first line, then emit the rest.
respond() {
    local file="$MOCK_DIR/$1"
    if [[ ! -f "$file" ]]; then
        echo '[]'
        exit 0
    fi
    {
        read -r delay
        sleep "$delay"
        cat
    } <"$file"
    exit 0
}

# Branch names become filenames; `/` is the only character the demo's branches
# use that can't appear in one. `write_gh_mock_data` names the files with the
# same substitution — the two have to agree.
sanitize() { printf '%s' "${1//\//_}"; }

# wt gates every forge call on `gh --version` then `gh auth status`
# (`CiToolsStatus::detect`), so both have to succeed before the responses below
# are ever asked for.
if [[ "${1:-}" == "--version" ]]; then
    echo "gh version 2.63.2 (2024-12-05)"
    exit 0
fi

if [[ "${1:-}" == "auth" && "${2:-}" == "status" ]]; then
    exit 0
fi

if [[ "${1:-}" == "pr" && "${2:-}" == "list" ]]; then
    branch=""
    prev=""
    for arg in "$@"; do
        [[ "$prev" == "--head" ]] && branch="$arg"
        prev="$arg"
    done
    respond "head/$(sanitize "$branch")"
fi

# `gh pr view <n> --json comments` — the picker's `comments` tab when the CI
# call hasn't primed its cache yet.
if [[ "${1:-}" == "pr" && "${2:-}" == "view" ]]; then
    respond "view/${3:-}"
fi

# `gh api repos/<owner>/<repo>/commits/<sha>/check-runs` — branch CI for a
# commit with no PR. wt passes --jq, so the file holds the post-jq array.
if [[ "${1:-}" == "api" && "${2:-}" == */check-runs ]]; then
    sha="${2#*/commits/}"
    respond "sha/${sha%/check-runs}"
fi

exit 1
