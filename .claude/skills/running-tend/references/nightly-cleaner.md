# Nightly Sweep — Worktrunk Specifics

## Survey Checklist

For each `.rs` file in the survey, also check:

- **System docstring** — modules with cached state, coordination logic, or non-obvious invariants need a spec docstring (see CLAUDE.md "System Docstrings"). Flag if missing or stale.

## Branch Naming

`nightly/clean-$GITHUB_RUN_ID`

## Session Budget — Don't Chain a Second CI-Fix PR

The nightly session has a hard ~60-minute cap: the harness kills it at 3600s with exit 143, reporting the run as `failure` and `$0.00` / 0 tokens (the kill pre-empts the result event). Ship the sweep PR first, then monitor only *its* CI per the `running-in-ci` CI-monitoring loop.

When monitoring your nightly PR's CI surfaces a failure in an *unrelated, repo-wide* job — e.g. a `code-coverage` / apt-install infra break that fails identically on `main` and every PR — that is **`tend-ci-fix`'s** domain, not the nightly's. `tend-ci-fix` fires on every `ci`-workflow `failure` on `main`, and a failed `code-coverage` job is enough to trigger it (non-required ≠ out of scope). Record the breakage in your summary and finish; do **not** open a second investigate-and-fix PR in the same session.

Run [28357689944](https://github.com/max-sixty/worktrunk/actions/runs/28357689944) did exactly that: it shipped #3318 (merged), then — while polling #3318's CI — diagnosed an upstream `cache-apt-pkgs-action` regression and chained a full SHA-pin fix across four `.github/` sites. It hit the 3600s timeout mid-commit, stranding the branch `fix/ci-cache-apt-pkgs-pin-sha` (pushed, no PR). The hour and the diagnosis were spent on a second deliverable the session couldn't finish.
