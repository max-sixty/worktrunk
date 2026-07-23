# Nightly Sweep — Worktrunk Specifics

## Survey Checklist

For each `.rs` file in the survey, also check:

- **System docstring** — modules with cached state, coordination logic, or non-obvious invariants need a spec docstring (see CLAUDE.md "System Docstrings"). Flag if missing or stale.

## Branch Naming

`nightly/clean-$GITHUB_RUN_ID`

## Session Budget — One Deliverable

The session runs under a harness timeout. The kill pre-empts the result event, so an overrun reports as `failure` with `$0.00` and 0 tokens and loses its summary. Ship the sweep PR, then monitor only *its* CI per the `running-in-ci` CI-monitoring loop.

A failure in an unrelated, repo-wide job (one that fails identically on `main` and every PR, such as a broken system-package install in `code-coverage`) belongs to `tend-ci-fix`, which fires on every `ci`-workflow `failure` on `main`; a non-required job failing is enough to trigger it. Record the breakage in the summary and finish rather than opening a second investigate-and-fix PR the session has no budget to land.
