---
name: review-reviewers
description: Hourly analysis of Claude CI session logs — identifies behavioral problems, skill gaps, and workflow issues.
metadata:
  internal: true
---

# Review Reviewers

Analyze Claude-powered CI runs from the past hour. Identify behavioral problems,
skill gaps, and workflow issues — then create PRs or issues to fix them.

## Confidence and magnitude gates

Before creating a PR, every finding must pass two gates.

### Gate 1: Confidence — is this a real problem?

Rate each finding on the evidence scale:

| Evidence level | Meaning | Minimum occurrences to act |
|---|---|---|
| **Critical** | Clearly wrong outcome (closed wrong issue, merged broken code, deleted user data) | 1 |
| **High** | Consistent pattern across multiple sessions | 2–3 |
| **Medium** | Plausible problem seen once, could be noise | 5+ |
| **Low** | Nitpick or stylistic preference | Do not act |

Occurrences include both the current hour's sessions **and** historical evidence
from the tracking issue (see [Evidence accumulation](#evidence-accumulation)).

If a finding doesn't meet the threshold, **skip it** — don't create a PR, don't
create an issue, don't comment. Record it in the tracking issue so it can
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
1. The evidence level and occurrence count (current hour + historical)
2. The proposed change type
3. Whether it passes both gates

Only proceed to Step 5 for findings that pass both gates.

## Evidence accumulation

Each run only sees the past hour of CI sessions, but patterns may emerge over
days or weeks. Use a **monthly tracking issue** to accumulate evidence across
runs.

### Finding or creating the tracking issue

```bash
# Look for this month's tracking issue
MONTH=$(date +%Y-%m)
gh issue list --state open --label review-reviewers-tracking \
  --json number,title --jq ".[] | select(.title | contains(\"$MONTH\"))"
```

If none exists for the current month, create one:

```bash
cat > /tmp/tracking-body.md << 'EOF'
Monthly tracking issue for review-reviewers findings that haven't yet met
the confidence threshold. Each run appends below-threshold findings as a
comment. Future runs read these to build cumulative evidence.

**Do not close manually** — a new issue is created each month.
EOF
gh issue create \
  --title "review-reviewers tracking: $MONTH" \
  --label review-reviewers-tracking \
  -F /tmp/tracking-body.md
```

### Reading historical evidence

Before applying the gates, read the current tracking issue's comments to find
prior observations that overlap with this hour's findings:

```bash
TRACKING_NUMBER=<number from above>
gh issue view "$TRACKING_NUMBER" --json comments \
  --jq '.comments[] | {author: .author.login, body: .body}'
```

Also check last month's tracking issue (if it exists) for recent carry-over.

When a historical entry looks like it might match a current finding, **download
and investigate the linked workflow's session logs** — don't rely on the summary
text alone, which lacks sufficient context to judge relatedness:

```bash
# Each tracking entry has a Run ID — use it to pull the actual logs
gh run download <run-id> --name claude-session-logs --dir /tmp/logs/<run-id>/
```

Trace the original decision chain in the session JSONL to confirm the historical
case is genuinely the same pattern, not just superficially similar. This is
laborious but necessary for accurate evidence counts.

Add confirmed matching historical occurrences to your tally when evaluating
gates.

### Recording below-threshold findings

After analysis, find **the bot's existing comment** on the tracking issue and
**edit it** to include any new findings. If no bot comment exists yet, create
one. This avoids notification spam from hourly runs.

```bash
# Find existing bot comment on the tracking issue
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')
EXISTING_COMMENT=$(gh api "repos/$REPO/issues/$TRACKING_NUMBER/comments" \
  --jq "[.[] | select(.user.login == \"$BOT_LOGIN\")] | last | .id // empty")
```

If `EXISTING_COMMENT` is non-empty, update it via
`REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner') && gh api repos/$REPO/issues/comments/$EXISTING_COMMENT -X PATCH -F body=@/tmp/findings.md`.
Otherwise create a new comment.

Format each finding in the comment body as:

```
### <short description>
- **Evidence level**: Medium
- **Occurrences this run**: 1
- **Run ID**: <run-id>
- **Workflow**: https://github.com/{owner}/{repo}/actions/runs/<run-id>
- **Session**: <session file>
- **Detail**: <brief description of what was observed>
```

This lets future runs search for the description and count prior occurrences.

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

CI polling time (including codecov timeouts on docs-only or test-only PRs) is
expected and acceptable — do not flag it. Wasted polling is a very low-priority
concern that does not warrant skill changes or PRs.

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
  ID, log excerpts, root cause, **gate assessment** including historical
  evidence count). Don't also create a separate issue.
- **Issue** (fallback): Only for problems too large or ambiguous to fix
  directly. Include run ID, log excerpts, root cause analysis.

Group multiple findings by broad theme. **Limit to at most 2 PRs per run** —
if you have more findings, pick the highest-confidence ones and note the rest
in the tracking issue.

## Step 6: Summary

If no problems found (or none passed the gates), report "all clear" with: runs
analyzed, sessions reviewed, brief quality assessment, and any below-threshold
findings recorded in the tracking issue.
