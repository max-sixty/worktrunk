---
name: triage-issue
description: Triages new GitHub issues — classifies, reproduces bugs, attempts conservative fixes, and comments. Use when a new issue is opened and needs automated triage.
argument-hint: "[issue number]"
metadata:
  internal: true
---

# Worktrunk Issue Triage

Load `/cd-triage` for the generic triage workflow (classification, duplicate
search, investigation, reproduction, fix, comment templates). This file adds
worktrunk-specific commands and conventions.

Triage a newly opened GitHub issue on worktrunk, a Rust CLI tool for managing
git worktrees.

**Issue to triage:** $ARGUMENTS

## Test Commands

```bash
# Unit tests
cargo test --lib --bins -- test_name
# Integration tests
cargo test --test integration -- test_name
# Full test suite + lints
cargo run -- hook pre-merge --yes
```

## CI Monitoring

After creating a PR, monitor CI using `/running-in-ci` (includes codecov/patch
polling).

## Version Check

When a bug may already be fixed, ask the reporter: `wt --version`

## Labels

Use `automated-fix` label on fix PRs.
