---
name: review-reviewers
description: Hourly analysis of Claude CI session logs — identifies behavioral problems, skill gaps, and workflow issues.
metadata:
  internal: true
---

# Review Reviewers

Analyze Claude-powered CI runs from the past hour. Identify behavioral problems,
skill gaps, and workflow issues — then create PRs or issues to fix them.

## Step 1: Find recent runs

Run `.github/scripts/list-recent-runs.sh` for recently completed Claude CI runs.
If empty, report "no runs to review" and exit.

Include `claude-hourly-review-reviewers` runs — self-analysis is intentional so
we can catch bugs in the reviewer itself.

## Step 2: Download and analyze session logs

```bash
gh run download <run-id> --name claude-session-logs --dir /tmp/logs/<run-id>/
```

Skip runs without artifacts. Find JSONL files under `/tmp/logs/` and extract:

```bash
# Tool calls
jq -c 'select(.type == "assistant") | .message.content[]? |
  select(.type == "tool_use") | {tool: .name, input: .input}' < file.jsonl

# Assistant reasoning
jq -r 'select(.type == "assistant") | .message.content[]? |
  select(.type == "text") | .text' < file.jsonl
```

Trace decision chains: what did Claude decide, what evidence did it use, what
was the outcome?

## Step 3: Cross-check review sessions

For `claude-review` runs, compare what the bot said against what happened next:

```bash
HEAD_BRANCH=$(gh run view <run-id> --json headBranch --jq '.headBranch')
PR_NUMBER=$(gh pr list --head "$HEAD_BRANCH" --state all --json number --jq '.[0].number')
```

Check for subsequent commits that undid something the bot approved (gap in
review), and human review comments flagging issues the bot missed. Pull in the
full PR context — not just changes from the past hour.

CI polling time is expected and acceptable — do not flag it.

## Step 4: Deduplicate

Before creating issues or PRs, check exhaustively for existing ones:

```bash
gh issue list --state open --label claude-behavior --json number,title,body
gh issue list --state open --json number,title,body  # also check unlabeled issues
gh pr list --state open --json number,title,headRefName,body
gh issue list --state closed --label claude-behavior --json number,title,closedAt --limit 30
```

Search titles AND bodies for related keywords. Only comment on existing issues
if you have material new cases that would change the approach or increase
prioritization. Do not comment with progress updates, fix-PR status, or
re-statements of evidence already in the issue.

## Step 5: Act on findings

**Prefer PRs over issues.** A PR with a clear description is immediately
actionable.

- **PR** (default): Branch `hourly/review-$GITHUB_RUN_ID`, fix, commit, push,
  create with label `claude-behavior`. Put full analysis in PR description (run
  ID, log excerpts, root cause). Don't also create a separate issue.
- **Issue** (fallback): Only for problems too large or ambiguous to fix
  directly. Include run ID, log excerpts, root cause analysis.

Group multiple findings by broad theme.

## Step 6: Summary

If no problems found, report "all clear" with: runs analyzed, sessions reviewed,
brief quality assessment.
