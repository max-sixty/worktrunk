# Nightly Sweep — Worktrunk Specifics

## Survey Checklist

For each `.rs` file in the survey, also check:

- **System docstring** — modules with cached state, coordination logic, or non-obvious invariants need a spec docstring (see CLAUDE.md "System Docstrings"). Flag if missing or stale.

## Branch Naming

`nightly/clean-$GITHUB_RUN_ID`
