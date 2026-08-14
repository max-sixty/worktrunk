# Nightly Sweep — Worktrunk Specifics

## Survey Checklist

For each `.rs` file in the survey, also check:

- **System docstring** — modules with cached state, coordination logic, or non-obvious invariants need a spec docstring (see CLAUDE.md "System Docstrings"). Flag if missing or stale.

## Branch Naming

`nightly/clean-$GITHUB_RUN_ID`

## Repo-Wide CI Breakage

A failure that reproduces identically on `main` and every PR (a broken system-package install in `code-coverage`, say) belongs to `tend-ci-fix`, which fires on any watched-workflow `failure` on `main` — the list lives in `.config/tend.yaml` under `ci-fix.watched_workflows`; a non-required job failing is enough to trigger it. It does not fire on runs that end `cancelled`, so record the breakage in the summary rather than assuming the handoff lands.
