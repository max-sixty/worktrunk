---
name: cd-review
description: Reviews a pull request for code quality and correctness. Use when asked to review a PR or when running as an automated PR reviewer.
argument-hint: "[PR number]"
metadata:
  internal: true
---

# PR Review

Review a pull request.

**PR to review:** $ARGUMENTS

## Workflow

Follow these steps in order.

### 1. Pre-flight checks

Before reading the diff, run cheap checks to avoid redundant work. Shell state
doesn't persist between tool calls — re-derive `REPO` in each bash invocation or
combine commands.

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')
HEAD_SHA=$(gh pr view <number> --json commits --jq '.commits[-1].oid')
PR_AUTHOR=$(gh pr view <number> --json author --jq '.author.login')

# Find the bot's most recent substantive review (any state).
# Include reviews with a non-empty body OR approvals (LGTM uses --approve -b "").
# Uses "| length > 0" instead of "!= \"\"" to avoid bash ! history expansion.
# IMPORTANT: `gh pr view --json reviews` returns `.commit.oid` (NOT `.commit_id`).
# The REST API (`gh api .../reviews`) uses `.commit_id` — don't confuse the two.
LAST_REVIEW_SHA=$(gh pr view <number> --json reviews \
  --jq "[.reviews[] | select(.author.login == \"$BOT_LOGIN\" and (.body | length > 0 or .state == \"APPROVED\"))] | last | .commit.oid // empty")
```

If `LAST_REVIEW_SHA == HEAD_SHA`, this commit has already been reviewed — exit
silently. The only exception: an unanswered conversation question directed at
the bot (check below).

If the bot reviewed a previous commit (`LAST_REVIEW_SHA` exists but differs from
`HEAD_SHA`), check the incremental changes:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
gh api "repos/$REPO/compare/$LAST_REVIEW_SHA...$HEAD_SHA" \
  --jq '{total: ([.files[] | .additions + .deletions] | add), files: [.files[] | "\(.filename)\t+\(.additions)/-\(.deletions)"]}'
```

If the incremental changes are trivial, skip the full review **and do not
submit a new approval** — the existing review stands. Go directly to step 7 to
resolve any bot threads addressed by the new changes, then exit. Do NOT proceed
to steps 2, 3, or 4. Rough heuristic: changes under ~20 added+deleted lines
that don't introduce new functions, types, or control flow are typically
trivial.

Then read all previous bot feedback and conversation:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')
# Previous review bodies
gh api "repos/$REPO/pulls/<number>/reviews" \
  --jq ".[] | select(.user.login == \"$BOT_LOGIN\" and (.body | length > 0)) | {state, body}"
# Inline review comments
gh api "repos/$REPO/pulls/<number>/comments" --paginate \
  --jq ".[] | select(.user.login == \"$BOT_LOGIN\") | {path, line, body}"
# Conversation (catch questions directed at the bot)
gh api "repos/$REPO/issues/<number>/comments" --paginate \
  --jq '.[] | {author: .user.login, body: .body}'
```

**Do not repeat any point from previous reviews** — cross-reference previous bot
comments before posting inline comments. When concurrent runs race (a new push
while the first run is still responding), both see the same unanswered
question — check whether a bot reply exists after the question's timestamp
before answering. Address unanswered questions in the review body (not via
`gh pr comment`).

### 2. Check for overlapping PRs

Before reading the diff, scan other open PRs for file overlap.
If another PR touches the same files with a similar fix, flag it in the review
so one can be closed as a duplicate.

### 3. Read and understand the change

1. Read the PR diff with `gh pr diff <number>`.
2. Before going deeper, look at the PR as a reader would — not just the code,
   but the shape: what files are being added/changed, and does anything look
   off?
3. Read the changed files in full (not just the diff) to understand context.

### 4. Review

Scale depth to the change. A docs-only PR or a mechanical rename needs a skim
for correctness, not the full checklist. A new algorithm or state-management
change needs trace analysis. Don't over-analyze trivial changes.

Check the project's CLAUDE.md for language-specific review criteria and
conventions. Load any project-specific review skill if available.

**Code quality:**

- Is the code clear and well-structured?
- Are there simpler ways to express the same logic?
- Does it avoid unnecessary complexity, feature flags, or compatibility layers?

**Correctness:**

- Are there edge cases that aren't handled?
- Could the changes break existing functionality?
- Are error messages helpful and consistent with the project style?
- **Trace failure paths, don't just note error handling exists.** For code that
  modifies state through multiple fallible steps, walk through what happens when
  each error fires. What has already been mutated? Is the system left in a
  recoverable state?

**Testing:**

- Are the changes adequately tested?

**Same pattern elsewhere:**

When a PR fixes a bug or changes a pattern, search for the same pattern in
other files. If found in the diff, add inline suggestions; if found outside the
diff, offer to push a fix commit.

**Duplication check (for new functions/types):**

For every new public or module-level function added in the diff, search the
codebase for existing functions that do the same thing. LLM-generated code
frequently reinvents internal APIs — this is the highest-value check for
externally contributed PRs.

Two search strategies, both required:

1. **Similar names and signatures.** Search for functions with similar names,
   return types, or parameter types.
2. **Overlapping subgoals.** Identify the intermediate steps the new code
   performs and search for existing code that does the same sub-tasks.

Flag duplicates — reuse is almost always better than a parallel implementation.

### 5. Submit

**If there are no issues, approve with an empty body — silence means correct.**

```bash
gh pr review <number> --approve -b ""
```

If there are actionable findings, submit as a review with inline suggestions
for concrete fixes. Every comment must give the author something to act on:

| Don't post (internal analysis) | Post (actionable) |
|---|---|
| "The fix correctly delegates to X" | "The error message still references the old behavior" |
| "The threshold logic is correct" | _(nothing — silence means correct)_ |

Don't explain what the code does — the author wrote it. Don't nitpick
formatting — that's what linters are for. Explain *why* something should
change, not just *what*.

**Form your own opinion independently.** Do not factor in other reviewers'
comments or approvals when deciding whether to approve — the value of this
review is as an uncorrelated signal.

**When confidence is low**, go beyond checking the implementation — question the
approach: "Does this bypass or duplicate an existing API?" "What does this
change *not* handle?" If the design involves a judgment call, flag it for human
review as a COMMENT.

**Self-authored PRs** (`PR_AUTHOR == BOT_LOGIN`): Still perform the full review
(steps 2-3) — self-review catches real issues. Do NOT attempt
`gh pr review --approve` — GitHub rejects self-approvals. Submit as COMMENT
when there are concerns, or stay silent and skip to step 6. Always post CI
failure analysis as a COMMENT, even on self-authored PRs.

**Not confident enough to approve** (unfamiliar module, subtle logic): Add a
`+1` reaction instead — no review needed unless there are specific observations.

```bash
gh api "repos/$REPO/issues/<number>/reactions" -f content="+1"
```

#### Posting mechanics

Before posting, verify HEAD hasn't moved and no review was already posted for
this commit:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')
CURRENT_HEAD=$(gh pr view <number> --json commits --jq '.commits[-1].oid')
[ "$CURRENT_HEAD" != "$HEAD_SHA" ] && echo "HEAD moved — skipping" && exit 0

# NOTE: REST API uses .commit_id (not .commit.oid from gh pr view --json)
ALREADY_POSTED=$(gh api "repos/$REPO/pulls/<number>/reviews" \
  --jq "[.[] | select(.user.login == \"$BOT_LOGIN\" and .commit_id == \"$HEAD_SHA\")] | last | .submitted_at // empty")
[ -n "$ALREADY_POSTED" ] && echo "Already reviewed — skipping" && exit 0
```

Post exactly one review per run. Always give a verdict: **approve** or
**comment** (never "request changes"). Use `gh pr review` for reviews, not
`gh pr comment`. Note: `--comment` requires a non-empty body — if there's
nothing to say, use the approve-with-empty-body pattern.

**Inline suggestions are mandatory for concrete fixes.** Whenever there's a
concrete fix (typos, doc updates, naming, missing imports, minor refactors),
post it as an inline suggestion on the exact line — never as a code block in the
review body. Inline suggestions let the author apply with one click; code blocks
force them to find the line and copy-paste manually.

For fixes targeting lines outside the diff, offer to push a fix commit instead.

Post inline suggestions via the review API:

`````bash
cat > /tmp/review-body.md << 'EOF'
Summary of suggestions
EOF

cat > /tmp/review-payload.json << 'ENDJSON'
{
  "event": "COMMENT",
  "comments": [
    {
      "path": "example/file.txt",
      "line": 3,
      "body": "```suggestion\nnew text here\n```"
    }
  ]
}
ENDJSON

BODY=$(cat /tmp/review-body.md)
jq --arg body "$BODY" '.body = $body' /tmp/review-payload.json > /tmp/review-final.json

gh api "repos/$REPO/pulls/<number>/reviews" \
  --method POST \
  --input /tmp/review-final.json
`````

**Do not** use `-f 'comments[0][path]=...'` flag syntax — `gh api` converts
array indices to object keys, which GitHub rejects.

- If a review has both suggestions and prose observations, put the suggestions
  as inline comments and the prose in the review body.
- Multi-line suggestions: set `start_line` and `line` to define the range.
  GitHub **replaces** every line in that range with the suggestion content — any
  line in the range that isn't reproduced in the replacement is **deleted**.

  **Before posting any multi-line suggestion, verify it:**

  1. **Read the exact lines** `start_line` through `line` from the diff hunk.
  2. **Diff mentally**: every line in that range must either appear (possibly
     modified) in the replacement text, or be a line you intend to delete. If
     any line would be silently dropped, **shrink the range** or include the
     line in the replacement.
  3. **Cap the range at ~10 lines.** Larger suggestions are error-prone and hard
     to review. For changes spanning more than 10 lines, split into multiple
     suggestions or push a fix commit instead.
  4. **Never span markdown fences.** If the range includes a `` ``` `` line,
     GitHub's suggestion parser may consume it as a delimiter, corrupting the
     result. Either shrink the range to avoid the fence or push a commit.

### 6. Monitor CI

After approving or staying silent, monitor CI using the approach from
/cd-running-in-ci.

- **All required checks passed** -> done.
- **A check failed** and it's related to the PR -> post a follow-up COMMENT
  review with analysis and inline suggestions, then dismiss the bot's approval:
  ```bash
  # Use PUT, not POST — the dismiss endpoint requires it
  gh api "repos/$REPO/pulls/<number>/reviews/$REVIEW_ID/dismissals" \
    -X PUT -f message="CI failed — <reason>"
  ```
  Skip if already dismissed. **Do not push fixes on human-authored PRs** — post
  the analysis and offer to fix, then wait for the author to accept.
- **A check failed** and it's a transient flake (unrelated to the PR changes) ->
  1. **Re-run the failed jobs:**
     ```bash
     gh run rerun <run-id> --failed
     ```
  2. **Report the flake.** Search for an open issue about the specific flaky
     test. If found, append to an existing bot comment rather than posting a new
     one.

### 7. Resolve handled suggestions

After submitting the review, check if any unresolved bot threads have been
addressed by the new changes. Resolve threads where the suggestion was applied.

**Only resolve if the substance was addressed.** Read both the suggestion and the
new code — if the author took a different approach, verify its technical accuracy
before resolving. When in doubt, leave the thread open for a human reviewer.

**Self-authored PRs are especially risky.** When the bot is both author and
reviewer, there is a bias toward accepting the code's own claims. Treat
self-authored thread resolution with extra skepticism.

```bash
cat > /tmp/review-threads.graphql << 'GRAPHQL'
query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          comments(first: 1) {
            nodes {
              author { login }
              path
              line
              body
            }
          }
        }
      }
    }
  }
}
GRAPHQL

REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')
OWNER=$(echo "$REPO" | cut -d/ -f1)
NAME=$(echo "$REPO" | cut -d/ -f2)

gh api graphql -F query=@/tmp/review-threads.graphql \
  -f owner="$OWNER" -f repo="$NAME" -F number=<number> \
  | jq --arg bot "$BOT_LOGIN" '
    .data.repository.pullRequest.reviewThreads.nodes[]
    | select(.isResolved == false)
    | select(.comments.nodes[0].author.login == $bot)
    | {id, path: .comments.nodes[0].path, line: .comments.nodes[0].line, body: .comments.nodes[0].body}'

# Resolve a thread that has been addressed
cat > /tmp/resolve-thread.graphql << 'GRAPHQL'
mutation($threadId: ID!) {
  resolveReviewThread(input: {threadId: $threadId}) {
    thread { id }
  }
}
GRAPHQL

gh api graphql -F query=@/tmp/resolve-thread.graphql -f threadId="THREAD_ID"
```

Outdated comments (null line) are best-effort — skip if the original context
can't be located.

### 8. Push mechanical fixes

**Bot PRs** (Dependabot, renovate, etc.): If the review found concrete, fixable
issues and there's no human author to act on feedback, commit and push the fix
directly to the PR branch.

**Human PRs**: Post inline suggestions first. Additionally, offer to push a
commit when the fixes are mechanical and correctness is obvious. Only push
after the author accepts.

```bash
gh pr checkout <number>
git add <files>
git commit -m "fix: <description>

Co-Authored-By: Claude <noreply@anthropic.com>"
git push
```
