---
name: fix-ci
description: Debug and fix failing CI on main. Use when CI or docs workflow fails on main branch.
argument-hint: "[run-id and context]"
metadata:
  internal: true
---

# Fix CI on Main

CI has failed on the main branch. Diagnose the root cause, fix it, and create
a PR.

**Failed run:** $ARGUMENTS

## Workflow

### 1. Check for existing fixes

```bash
gh pr list --state open --label "automated-fix" --json number,title,headRefName
gh pr list --state open --head "fix/ci-" --json number,title,headRefName
```

If an existing PR addresses the same failure, comment on it linking the new run
and stop.

### 2. Diagnose and fix

1. Get failure logs: `gh run view <run-id> --log-failed`
2. Identify the failing job and root cause — don't just fix the symptom
3. Search for the same pattern elsewhere in the codebase
4. Reproduce locally using test commands from CLAUDE.md
5. Fix at the right level (shared helper > per-file fix)

### 3. Create PR

Re-check for existing fix PRs (one may have been created while you worked).

```bash
git checkout -b fix/ci-<run-id>
git add <files>
git commit -m "fix: <description>

Co-Authored-By: Claude <noreply@anthropic.com>"
git push -u origin fix/ci-<run-id>
```

Create the PR with `gh pr create --label "automated-fix"`. PR body format:

```
## Problem
[What failed and the root cause]

## Solution
[What was fixed and why this is the right level]

## Testing
[How the fix was verified]

---
🤖 Automated fix for [failed run](run-url)
```

### 4. Monitor CI

Poll CI using the approach from `/running-in-ci`. If CI fails, diagnose with
`gh run view <run-id> --log-failed`, fix, commit, push, and repeat.
