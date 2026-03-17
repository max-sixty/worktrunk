---
name: nightly-cleaner
description: Nightly code quality sweep — resolves bot PR conflicts, reviews recent commits, surveys existing code, and closes resolved issues.
metadata:
  internal: true
---

# Nightly Code Quality Sweep

Three phases: resolve conflicts on bot PRs, review recent commits, and survey
a slice of existing code/docs.

## Step 1: Resolve conflicts on bot PRs

```bash
gh pr list --author worktrunk-bot --json number,title,mergeable,headRefName \
  --jq '.[] | select(.mergeable == "CONFLICTING")'
```

For each conflicted PR, dispatch a subagent to:

1. Check out the PR: `gh pr checkout <number>`
2. Merge main: `git merge origin/main`
3. Resolve conflicts (read files, understand both sides), `git add`,
   `git commit --no-edit`
4. Push and poll CI using the approach from `/running-in-ci`
5. If conflicts are too complex, `git merge --abort` and comment explaining
   manual resolution is needed

Run subagents in parallel. Each must work in isolation
(`git worktree add /tmp/pr-<number> <branch>`). After all complete, clean up
temp worktrees and `git checkout main`.

Skip if no PRs have conflicts.

## Step 2: Review recent commits

```bash
git log --since='24 hours ago' --oneline main
```

If no commits in the past 24 hours, skip to Step 4.

Get the aggregate diff:

```bash
OLDEST=$(git log --since='24 hours ago' --format='%H' main | tail -1)
git diff ${OLDEST}^..HEAD
git log --since='24 hours ago' --format='%h %s' main
```

Review for: bugs, inconsistencies with existing patterns, missing/outdated
documentation (does `--help` still match?), missing test coverage, dead code,
non-canonical patterns, CLAUDE.md/skill drift.

## Step 3: Check existing issues and close resolved ones

```bash
gh issue list --state open --json number,title
gh pr list --state open --json number,title,headRefName
```

For each open issue, check whether recent commits or the current codebase
state already resolve it. If resolved, comment briefly and close with
`gh issue close`. Skip partially unresolved issues.

## Step 4: Rolling survey

Run `.github/scripts/todays-survey-files.sh` for today's file list (~10 files).

For each file, look for: bugs, stale documentation, dead code, simplification
opportunities, missing tests, CLAUDE.md/skill drift. Spend roughly equal time
per file.

## Step 5: Report findings

Before creating issues, check for duplicates:

```bash
gh issue list --state open --label nightly-cleanup --json number,title
```

For each finding (from both recent-commit review and rolling survey):

1. **Create a GitHub issue** with label `nightly-cleanup` — clear actionable
   title, include file location and suggested fix
2. **For confident fixes** (clear bugs, stale docs, obvious missing tests):
   branch `nightly/clean-$GITHUB_RUN_ID`, fix, test
   (`cargo run -- hook pre-merge --yes`), commit, push, create PR, poll CI.
   **Every bug fix must include a regression test that would have failed before
   the fix.** Write the test, verify it passes with the fix, and confirm it
   targets the specific behavior that was broken. If a test is not feasible
   (e.g., pure documentation changes), note why in the PR description.

## Step 6: Summary

Report: commits reviewed, files surveyed, findings, actions taken, assessment
(clean / minor issues / needs attention).
