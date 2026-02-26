---
name: issue-triage
description: Triages new GitHub issues — classifies, reproduces bugs, attempts conservative fixes, and comments. Use when a new issue is opened and needs automated triage.
argument-hint: "[issue number]"
---

# Issue Triage

Triage a newly opened GitHub issue on this project (a Rust CLI tool for
managing git worktrees).

**Issue to triage:** $ARGUMENTS

## Step 1: Setup

Load `/running-in-ci` first (CI environment rules, security).

Follow the AD FONTES principle throughout: reproduce before fixing, evidence
before speculation, test before committing.

## Step 2: Read and classify the issue

```bash
gh issue view $ARGUMENTS --json title,body,labels,author
```

Classify into one of:

- **Bug report** — describes unexpected behavior, includes steps to reproduce or
  error output
- **Feature request** — asks for new functionality or behavior changes
- **Question** — asks how to do something or how something works
- **Other** — doesn't fit the above categories

If not a bug report, skip to step 6 (comment only).

## Step 3: Check for duplicates

Before doing any work, check if this issue is already being addressed:

```bash
# Search open issues for similar problems
gh issue list --state open --json number,title,labels --limit 50

# Check for existing fix branches
git branch -r --list 'origin/fix/issue-*'
git branch -r --list 'origin/repro/issue-*'

# Check open PRs
gh pr list --state open --json number,title,headRefName --limit 50
```

If a duplicate or existing fix is found, note it for the comment in step 6.
Don't create a duplicate fix.

## Step 4: Reproduce the bug

Follow the AD FONTES principle — reproduce before fixing:

1. **Understand the report** — What command was run? What was expected? What
   actually happened?
2. **Find relevant code** — Search the codebase for the functionality described
3. **Write a failing test** — Add a test to the appropriate *existing* test file
   that demonstrates the bug. Don't create new test files.
4. **Run the test** to confirm it fails:
   ```bash
   cargo test --lib --bins -- test_name
   # or for integration tests:
   cargo test --test integration -- test_name
   ```

If the test passes (bug may already be fixed), note this for the comment.

If you cannot reproduce the bug (unclear steps, environment-specific, etc.),
note what you tried and skip to step 6.

## Step 5: Fix (conservative)

**Only attempt a fix if ALL of these conditions are met:**

- Bug is clearly reproducible (test fails)
- Root cause is understood
- Fix is localized (1-3 files changed)
- Confident the fix is correct

**Prefer the smallest change that fixes the bug.** Address only the reported
problem — don't restructure, reorder, or rename surrounding code even if it
would be "cleaner." If a larger refactoring seems beneficial, propose it in the
issue comment for the maintainer to decide. Triage fixes should be easy to
review and low-risk to merge.

### If fixing

1. Fix the root cause (not just the symptom)
2. Confirm the test now passes
3. Run the full test suite and lints:
   ```bash
   cargo run -- hook pre-merge --yes
   ```
4. Create branch, commit, push, and create PR:
   ```bash
   git checkout -b fix/issue-$ARGUMENTS
   git add -A
   git commit -m "fix: <description>

   Closes #$ARGUMENTS

   Co-authored-by: Claude <noreply@anthropic.com>"
   git push -u origin fix/issue-$ARGUMENTS
   gh pr create --title "fix: <description>" --label "automated-fix" --body "## Problem
   [What the issue reported and the root cause]

   ## Solution
   [What was fixed and why]

   ## Testing
   [How the fix was verified — mention the reproduction test]

   ---
   Closes #<issue-number> — automated triage"
   ```
5. Monitor CI until green:
   ```bash
   gh run list --branch fix/issue-$ARGUMENTS
   gh run watch
   ```
   If CI fails, diagnose with `gh run view <run-id> --log-failed`, fix, and
   repeat.

### If reproduction test works but fix is not confident

Commit just the failing test on a reproduction branch and open a PR:

```bash
git checkout -b repro/issue-$ARGUMENTS
git add -A
git commit -m "test: add reproduction for #$ARGUMENTS

Co-authored-by: Claude <noreply@anthropic.com>"
git push -u origin repro/issue-$ARGUMENTS
gh pr create --title "test: reproduction for #$ARGUMENTS" --label "automated-fix" --body "## Context
Adds a failing test that reproduces #$ARGUMENTS. The fix is not yet included —
this PR captures the reproduction so a maintainer can investigate.

---
Automated triage for #<issue-number>"
```

Note the PR number for the comment.

## Step 6: Comment on the issue

Always comment via `gh issue comment`. Keep it brief, polite, and specific. A
maintainer will always review — never claim the issue is fully resolved by
automation alone.

**Stay within what you verified.** State facts you found in the codebase — don't
characterize something as "known" unless you find prior issues or documentation
about it. Don't speculate beyond the code you read. Follow the templates below
closely; they are deliberately scoped to leave authoritative analysis to
maintainers.

Use the heredoc pattern from `/running-in-ci` for `--body` arguments to avoid
shell quoting issues (e.g., `!` getting escaped as `\!`).

Choose the appropriate template:

### Fix PR created

> Thanks for reporting this! I was able to reproduce the issue and identified
> the root cause: [one-sentence explanation].
>
> I've opened #PR_NUMBER with a fix. A maintainer will review it shortly.

### Reproduction test only (no fix attempted)

> Thanks for reporting this! I was able to reproduce the issue — #PR_NUMBER
> adds a failing test that demonstrates the bug.
>
> Root cause appears to be [brief explanation if known, or "still under
> investigation"]. A maintainer will take a closer look.

### Could not reproduce

> Thanks for reporting this! I tried to reproduce this but wasn't able to with
> the information provided.
>
> Could you share [specific information needed — exact command, config file,
> git repo structure, OS, shell, etc.]? That would help narrow it down.
>
> A maintainer will also take a look.

### Bug already fixed

> Thanks for reporting this! I looked into this and it appears the behavior
> described may already be fixed on `main` (the relevant test passes).
>
> Could you confirm which version you're running (`wt --version`)? If you're
> on an older release, updating should resolve this. A maintainer will
> confirm.

### Feature request

> Thanks for the suggestion! This is a feature request rather than a bug, so
> I'll leave it for a maintainer to evaluate and prioritize.

### Question

> Thanks for reaching out! This looks like a usage question rather than a bug
> report.
>
> [Brief answer if obvious from the codebase, or pointer to relevant docs/help
> text.]
>
> A maintainer can provide more detail if needed.

### Duplicate

> Thanks for reporting this! This appears to be related to #EXISTING_ISSUE
> [and/or PR #EXISTING_PR]. I'll leave it to a maintainer to confirm and
> link them.
