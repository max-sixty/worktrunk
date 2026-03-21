---
name: fix-ci
description: Debug and fix failing CI on main. Use when CI or docs workflow fails on main branch.
argument-hint: "[run-id and context]"
metadata:
  internal: true
---

# Worktrunk CI Fix

Load `/cd-ci-fix` for the generic CI fix workflow (diagnose, fix, create PR,
monitor CI). This file adds worktrunk-specific configuration.

**Failed run:** $ARGUMENTS

## Test Commands

```bash
# Full test suite + lints
cargo run -- hook pre-merge --yes
# Unit tests only
cargo test --lib --bins
# Integration tests only
cargo test --test integration
```

## Labels

Use `automated-fix` label on fix PRs.

## CI Monitoring

After creating a PR, monitor CI using `/running-in-ci` (includes codecov/patch
polling).
