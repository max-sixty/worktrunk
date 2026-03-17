---
name: review-reviewers
description: Nightly analysis of Claude CI session logs — identifies behavioral problems, skill gaps, and workflow issues.
metadata:
  internal: true
---

# Review Reviewers

Analyze Claude-powered CI runs from the past day. Identify behavioral problems,
skill gaps, and workflow issues — then create PRs or issues to fix them.

## Confidence and magnitude gates

Before creating a PR, every finding must pass two gates:

### Gate 1: Confidence — is this a real problem?

Rate each finding on the evidence scale:

| Evidence level | Meaning | Minimum occurrences to act |
|---|---|---|
| **Critical** | Clearly wrong outcome (closed wrong issue, merged broken code, deleted user data) | 1 |
| **High** | Consistent pattern across multiple sessions | 2–3 |
| **Medium** | Plausible problem seen once, could be noise | 5+ |
| **Low** | Nitpick or stylistic preference | Do not act |

If a finding doesn't meet the threshold, **skip it** — don't create a PR, don't
create an issue, don't comment. Log it in the summary (Step 6) so it can
accumulate evidence over future runs.

### Gate 2: Magnitude — is the fix proportionate?

Rate the proposed change:

| Change type | Examples | Evidence bar |
|---|---|---|
| **Removal / simplification** | Remove confusing sentence, delete dead guidance | Low (1 occurrence is enough) |
| **Targeted fix** | Fix a specific incorrect instruction, add a missing step | Normal (use Gate 1 thresholds) |
| **New paragraph or section** | Add explanation of a concept, new workflow guidance | High (need 3+ occurrences showing the gap) |
| **Structural change** | Reorganize a skill, add a new skill file, change workflow | Very high (need 5+ occurrences or a critical failure) |

**The larger the change, the more evidence required.** A one-line simplification
needs less justification than a new paragraph. Prefer small, targeted fixes over
broad rewrites.

### Applying the gates

For each finding, state:
1. The evidence level and occurrence count
2. The proposed change type
3. Whether it passes both gates

Only proceed to Step 5 for findings that pass both gates.

## Step 1: Find recent runs

Run `.github/scripts/list-recent-runs.sh` for recently completed Claude CI runs.
If empty, report "no runs to review" and exit.

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
full PR context — not just changes from the past day.

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

- **PR** (default): Branch `nightly/review-$GITHUB_RUN_ID`, fix, commit, push,
  create with label `claude-behavior`. Put full analysis in PR description (run
  ID, log excerpts, root cause, **gate assessment**). Don't also create a
  separate issue.
- **Issue** (fallback): Only for problems too large or ambiguous to fix
  directly. Include run ID, log excerpts, root cause analysis.

Group multiple findings by broad theme. **Limit to at most 2 PRs per run** —
if you have more findings, pick the highest-confidence ones and note the rest
in the summary.

## Step 6: Summary

If no problems found (or none passed the gates), report "all clear" with: runs
analyzed, sessions reviewed, brief quality assessment, and any below-threshold
findings noted for future evidence accumulation.
