# Nightly Sweep — Worktrunk Specifics

## Survey Checklist

For each `.rs` file in the survey, also check:

- **System docstring** — modules with cached state, coordination logic, or non-obvious invariants need a spec docstring (see AGENTS.md "System Docstrings"). Flag if missing or stale.

## Branch Naming

`nightly/clean-$GITHUB_RUN_ID`

## Repo-Wide CI Breakage

A failure that reproduces identically on `main` and every PR (a broken system-package install in `code-coverage`, say) is reported by `tend-ci-failures` and dispatched to triage — the watched workflows are listed under `ci-failures.watched_workflows` in `.config/tend.yaml`; a non-required job failing is enough to report it. A cancelled run does not trigger it, so record a breakage that shows up only as cancellations in the summary rather than assuming a handoff.
