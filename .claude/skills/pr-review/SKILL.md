---
name: pr-review
description: Reviews a pull request for idiomatic Rust, project conventions, and code quality. Use when asked to review a PR or when running as an automated PR reviewer.
argument-hint: "[PR number]"
---

# Worktrunk PR Review

Review a pull request to worktrunk, a Rust CLI tool for managing git worktrees.

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


# Find the bot's most recent substantive review (any state).
# Include reviews with a non-empty body OR approvals (LGTM uses --approve -b "").
# Uses "| length > 0" instead of "!= \"\"" to avoid bash ! history expansion.
LAST_REVIEW_SHA=$(gh pr view <number> --json reviews \
  --jq "[.reviews[] | select(.author.login == \"$BOT_LOGIN\" and (.body | length > 0 or .state == \"APPROVED\"))] | last | .commit.oid // empty")
```

If `LAST_REVIEW_SHA == HEAD_SHA`, this commit has already been reviewed — exit
silently. The only exception: a conversation comment asks the bot a question
(checked below).

If the bot reviewed a previous commit (`LAST_REVIEW_SHA` exists but differs from
`HEAD_SHA`), check the incremental changes:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
gh api "repos/$REPO/compare/$LAST_REVIEW_SHA...$HEAD_SHA" \
  --jq '{total: ([.files[] | .additions + .deletions] | add), files: [.files[] | "\(.filename)\t+\(.additions)/-\(.deletions)"]}'
```

If the incremental changes are trivial, skip the full review (steps 2-3) — the
existing review stands. Still proceed to step 6 to resolve any bot threads
addressed by the new changes, then exit. Rough heuristic: changes under ~20
added+deleted lines that don't introduce new functions, types, or control flow
are typically trivial (review feedback addressed, CI/formatting fixes, small
corrections). Only proceed with a full review for non-trivial changes.

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

**Do not repeat any point from previous reviews.** If a previous review already
noted an issue, don't raise it again.

If a conversation comment asks the bot a question (mentions `$BOT_LOGIN`,
replies to a bot comment, or is clearly directed at the reviewer), address it in
the review body.

### 2. Read and understand the change

1. Read the PR diff with `gh pr diff <number>`.
2. Before going deeper, look at the PR as a reader would — not just the code,
   but the shape: what files are being added/changed, and does anything look
   off?
3. Read the changed files in full (not just the diff) to understand context.

### 3. Review

Review design first, then tactical checklist.

**Idiomatic Rust and project conventions:**

- Does the code follow Rust idioms? (Iterator chains over manual loops, `?` over
  match-on-error, proper use of Option/Result, etc.)
- Does it follow the project's conventions documented in CLAUDE.md? (Cmd for
  shell commands, error handling with anyhow, accessor naming conventions, etc.)
- Are there unnecessary allocations, clones, or owned types where borrows would
  suffice?

**Code quality:**

- Is the code clear and well-structured?
- Are there simpler ways to express the same logic?
- Does it avoid unnecessary complexity, feature flags, or compatibility layers?

**Correctness:**

- Are there edge cases that aren't handled?
- Could the changes break existing functionality?
- Are error messages helpful and consistent with the project style?
- Does new code use `.expect()` or `.unwrap()` in functions returning `Result`?
  These should use `?` or `bail!` instead — panics in fallible code bypass error
  handling.
- **Trace failure paths, don't just note error handling exists.** For code that
  modifies state through multiple fallible steps, walk through what happens when
  each `?` fires. What has already been mutated? Is the system left in a
  recoverable state? Describing the author's approach ("ordered for safety") is
  not the same as verifying it.

**Testing:**

- Are the changes adequately tested?
- Do the tests follow the project's testing conventions (see tests/CLAUDE.md)?

**Documentation accuracy:**

When a PR changes behavior, check that related documentation still matches.
This is a common source of staleness — new features get added or behavior
changes, but help text, config comments, and doc pages aren't updated.

- Does `after_long_help` in `src/cli/mod.rs` and `src/cli/config.rs` still
  describe what the code does? (These are the primary sources for doc pages.)
- Do inline TOML comments in config examples match the actual behavior?
- Are references to CLI commands still valid? (e.g., a migration note
  referencing `wt config show` when the right command is `wt config update`)
- If a new feature was added, does the relevant help text mention it?

**Same pattern elsewhere:**

When a PR fixes a bug or changes a pattern, search for the same pattern in
other files. A fix applied to one location often needs to be applied to sibling
files. For example, if a PR fixes a broken path in one workflow file, grep for
the same broken path across all workflow files.

```bash
# Example: PR fixes `${{ env.HOME }}` in one workflow — check all workflows
rg 'env\.HOME' .github/workflows/
```

If the same issue exists elsewhere, add inline suggestions fixing each
occurrence.

### 4. Submit

#### Staleness check

Before posting, verify the PR hasn't received new commits since you started:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
# HEAD_SHA was captured in pre-flight (step 1)
CURRENT_HEAD=$(gh pr view <number> --json commits --jq '.commits[-1].oid')
if [ "$CURRENT_HEAD" != "$HEAD_SHA" ]; then
  echo "HEAD moved — newer commit will trigger a fresh review"
  exit 0
fi
```

If HEAD moved, skip posting. A newer workflow run will review the latest code.

#### What to post

Separate internal analysis from postable feedback. The review exists to help the
author improve the code — not to demonstrate understanding.

Every comment must be **actionable** — the author can do something with it:

| Don't post (internal analysis) | Post (actionable) |
|---|---|
| "The fix correctly delegates to `default_config_path()`" | "The error hints still reference `$XDG_CONFIG_HOME` but the code uses `etcetera` now" |
| "The threshold logic is correct — spacing reclaim matches allocation" | _(nothing — silence means correct)_ |
| "Good use of `Iterator::scan` here" | "This `.collect::<Vec<_>>()` is only iterated once — can stay as an iterator" |

**Rules:**

- **Don't explain what the code does.** The author wrote it.
- **If the code needs explanation for future readers**, suggest a docstring or
  inline comment — as a code suggestion, not prose.
- **Always use inline suggestions** for concrete fixes — never put replacement
  code as a fenced code block in the review body. If you can name the file,
  line, and new text, it must be a `suggestion` block on that line via the
  review API. The review body is only for summary text.
- **Explain *why*** something should change, not just *what*.
- **Distinguish severity** — "should fix" vs. "nice to have".
- **Don't nitpick formatting** — that's what linters are for.

**Never post a comment with nothing useful to contribute.** If there are no
issues, the author doesn't need to hear that. Use the LGTM verdict (approve
with empty body) or stay silent.

#### Verdict

Decide how confident you are in the change:

```bash
PR_AUTHOR=$(gh pr view <number> --json author --jq '.author.login')
```

**Self-authored PRs:** If `PR_AUTHOR == BOT_LOGIN`, you cannot approve — GitHub
rejects self-approvals. Submit as COMMENT when there are concerns, or stay
silent if there are none.

- **Confident** (small, mechanical, well-tested): Approve.
- **Moderately confident** (non-trivial but looks correct): Approve.

When approving with no issues, approve with an empty body and react:

```bash
gh pr review <number> --approve -b ""
gh api "repos/$REPO/issues/<number>/reactions" -f content="+1"
```

- **Unsure** (complex logic, edge cases, untested paths): Run tests locally
  (`cargo run -- hook pre-merge --yes`) if the toolchain is available. Otherwise
  submit as COMMENT noting specific concerns.

**Confidence factors:**

Increases confidence: small diffs, existing test coverage, mechanical changes,
author has deep familiarity with the affected code.

Decreases confidence: new algorithms, concurrency, error handling changes,
untested paths, author hasn't
contributed to the affected module before, LLM-generated code (may duplicate
existing APIs or miss design intent).

**When confidence is low**, go beyond checking the implementation — question the
approach:

- "Does this bypass or duplicate an existing API?"
- "What does this change *not* handle?"
- Check that the fix doesn't introduce a different class of bug (e.g., ignoring
  config overrides, using fixed sleeps instead of polling).
- If the design involves a judgment call, flag it for human review as a COMMENT.

#### Posting

Submit **one formal review per run** via `gh pr review`. Note that `--comment`
requires a non-empty body (`-b ""` fails) — if there's nothing to say, use the
approve-with-empty-body pattern instead.

- Always give a verdict: **approve** or **comment**. Don't use "request changes"
  (that implies authority to block).
- **Don't use `gh pr comment`** — use review comments (`gh pr review` or
  `gh api` for inline suggestions) so feedback is threaded with the review.
- Don't repeat suggestions already made by humans or previous bot runs
  (checked in step 1).

**Inline suggestions are mandatory for specific fixes.** Whenever there's a
concrete fix (typos, doc updates, naming, missing imports, minor refactors, any
change expressible as replacement lines), post it as an inline suggestion on the
exact line — never as a code block in the review body. Inline suggestions let
the author apply with one click; code blocks in the body force them to find the
line and copy-paste manually.

**Anti-pattern — code block in review body:**

> The description on line 3 should be updated:
> ```
> description: new text here
> ```

**Correct — inline suggestion on the line:**

`````bash
gh api "repos/$REPO/pulls/<number>/reviews" \
  --method POST \
  -f event=COMMENT \
  -f body="Summary of suggestions" \
  -f 'comments[0][path]=.claude/skills/example/SKILL.md' \
  -f 'comments[0][line]=3' \
  -f 'comments[0][body]=```suggestion
description: new text here
```'
`````

- Use suggestions for any small fix — no limit on count.
- If a review has both suggestions and prose observations, put the suggestions
  as inline comments and the prose in the review body.
- Prose-only comments are for changes too large or uncertain for a direct
  suggestion.
- Multi-line suggestions: set `start_line` and `line` to define the range.

### 5. Monitor CI

After approving, check whether CI has finished. Exclude the current workflow's
own check to avoid a circular wait (Claude polling itself):

```bash
# $GITHUB_WORKFLOW is set in CI; when unset, no checks are excluded.
gh pr view <number> --json statusCheckRollup \
  --jq '[.statusCheckRollup[]
    | select(env.GITHUB_WORKFLOW == null
             or (.workflowName == env.GITHUB_WORKFLOW | not))]
    | .[]
    | {name, status, conclusion}'
```

- **All checks passed** → done, no further action.
- **Checks still running** → poll until complete (sleep 30–60s between checks).
  Polling is intentionally unbounded — CI compute is cheap and catching failures
  before the author looks at the PR is more valuable than saving a few minutes
  of polling. The results are always visible on the PR regardless, so the worst
  case of a long poll is wasted compute, not missed information.
- **A check failed** → if it's a flaky test or unrelated infrastructure
  failure, no action needed. If the failure is related to the PR changes:
  1. Dismiss the bot's approval if one exists (empty dismiss message). Skip
     if already dismissed — redundant dismissals create timeline noise.
  2. Investigate the failure and post a follow-up review (COMMENT) with
     analysis, inline suggestions, and an offer to fix. Same rules as
     step 4 — no repeated points from previous reviews.

### 6. Resolve handled suggestions

After submitting the review, check if any unresolved review threads from the bot
have been addressed. You've already read the changed files during review — if a
suggestion was applied or the issue was otherwise fixed, resolve the thread.

Use the file-based GraphQL pattern from `/running-in-ci` to avoid quoting
issues with `$` variables:

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

### 7. Push mechanical fixes on bot PRs

If the review found concrete, fixable issues on a bot PR (Dependabot, renovate,
etc.) where there's no human author to act on feedback, commit and push the fix
directly to the PR branch.

```bash
gh pr checkout <number>
git add <files>
git commit -m "fix: <description>

Co-Authored-By: Claude <noreply@anthropic.com>"
git push
```

Only do this for mechanical changes where correctness is obvious. For human PRs,
leave inline suggestions for the author instead.
