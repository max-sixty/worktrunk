---
name: nightly-cleaner
description: Nightly code quality sweep — resolves bot PR conflicts, reviews recent commits, surveys existing code, and closes resolved issues.
metadata:
  internal: true
---

# Worktrunk Nightly Sweep

Load `/nightly` for the generic nightly sweep workflow (conflict resolution,
commit review, issue closing, survey methodology, findings reporting). This file
adds worktrunk-specific configuration.

## Bot Identity

Use `worktrunk-bot` when filtering PRs for conflict resolution.

## Survey Script

```bash
.github/scripts/todays-survey-files.sh
```

## Test Command

```bash
cargo run -- hook pre-merge --yes
```

## Labels and Branches

- Issue label: `nightly-cleanup`
- Branch naming: `nightly/clean-$GITHUB_RUN_ID`

## CI Monitoring

After creating a PR, monitor CI using `/continuous-running-in-ci` (includes codecov/patch
polling).
