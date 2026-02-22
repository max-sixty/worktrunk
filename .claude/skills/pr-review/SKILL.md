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
existing review stands. Still proceed to step 5 to resolve any bot threads
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

If the same issue exists elsewhere, flag it in the review.

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

#### Content filter

Separate internal analysis from postable feedback. The review exists to help the
author improve the code — not to demonstrate understanding.

- **Post**: Problems found, improvements suggested, questions about intent. Each
  must be something the author can act on.
- **Don't post**: Explanations of what the code does, confirmation that the
  approach is correct, summaries of the change. This is internal analysis.

If the code lacks explanation for future readers, suggest a docstring or
comment — as a code suggestion, not prose.

If nothing is actionable, use the LGTM behavior (approve with empty body).

#### Confidence-based verdict

After reviewing, check CI status and decide:

```bash
PR_AUTHOR=$(gh pr view <number> --json author --jq '.author.login')
gh pr view <number> --json statusCheckRollup \
  --jq '.statusCheckRollup[] | {name: .name, status: .status, conclusion: .conclusion}'
```

**Self-authored PRs:** If `PR_AUTHOR == BOT_LOGIN`, you cannot approve — GitHub
rejects self-approvals. Skip directly to submitting as COMMENT.

- **Confident** (small, mechanical, well-tested): Approve immediately.
- **Moderately confident** (non-trivial but looks correct): Approve if CI is
  green. If CI is pending, submit as COMMENT — don't approve unverified changes.
- **Unsure** (complex logic, edge cases, untested paths): Run tests locally
  (`cargo run -- hook pre-merge --yes`) if the toolchain is available. Otherwise
  submit as COMMENT noting specific concerns.

**Never promise follow-up actions.** This workflow runs once per push and does
not re-trigger when CI completes. Don't say "Will approve once CI finishes" or
"Will approve once CI is green" — that implies a follow-up that won't happen.
Instead, state the review outcome and the current CI status as facts:
- Good: "No issues found. CI is still running — submitting as comment, not approval."
- Bad: "Will approve once CI finishes." (promises action the bot can't take)

Factors: small diffs, existing test coverage, and mechanical changes increase
confidence. New algorithms, concurrency, error handling changes, and untested
paths decrease it.

#### Posting

Submit **one formal review per run** via `gh pr review`. Never call it multiple
times. Note that `--comment` requires a non-empty body (`-b ""`
fails) — if you have nothing to say, use LGTM behavior (`--approve -b ""`)
instead. Never fall back from a failed `--comment` to `--approve`.

- Always give a verdict: **approve** or **comment**. Don't use "request changes"
  (that implies authority to block).
- **Don't use `gh pr comment`** — use review comments (`gh pr review` or
  `gh api` for inline suggestions) so feedback is threaded with the review.
- Don't repeat suggestions already made by humans or previous bot runs
  (checked in step 1).
- **Default to code suggestions** for specific fixes — see "Inline suggestions"
  below. Prose comments are for changes too large or uncertain for a suggestion.

### 5. Resolve handled suggestions

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

## LGTM behavior

When the PR has no issues worth raising:

1. Approve with an empty body (no fluff — silence is the best compliment):
   ```bash
   gh pr review <number> --approve -b ""
   ```
2. Add a thumbs-up reaction:
   ```bash
   gh api "repos/$REPO/issues/<number>/reactions" -f content="+1"
   ```

## Inline suggestions

**Code suggestions are the default format for specific fixes.** Whenever you
have a concrete fix (typos, doc updates, naming, missing imports, minor
refactors, any change you can express as replacement lines), use GitHub's
suggestion format so the author can apply it with one click:

`````bash
gh api "repos/$REPO/pulls/<number>/reviews" \
  --method POST \
  -f event=COMMENT \
  -f body="Summary of suggestions" \
  -f 'comments[0][path]=src/foo.rs' \
  -f 'comments[0][line]=42' \
  -f 'comments[0][body]=```suggestion
fixed line content here
```'
`````

**Rules:**
- Use suggestions for any small fix you're confident about — no limit on count.
- Only use prose comments for changes that are too large or uncertain for a
  direct suggestion.
- Multi-line suggestions: set `start_line` and `line` to define the range.

### 6. Request fixes on bot PRs

The review workflow is read-only (`contents: read`) and cannot push fixes. For
bot PRs (Dependabot, renovate, etc.), request fixes via the `@worktrunk-bot` mention
workflow, which has write access and can push commits to the PR branch.

**When to use:** The review found concrete, fixable issues (CI failures, missing
test updates, small code problems) on a bot-authored PR. Don't use this for
human PRs — leave suggestions for the author instead.

After submitting the review, post a separate comment:

```bash
gh pr comment <number> --body "@worktrunk-bot The review found issues on this Dependabot PR. Please fix:

- [specific issue 1]
- [specific issue 2]

See the review comments for details."
```

This triggers the `claude-mention` workflow, which checks out the PR branch,
applies fixes, and pushes. CI reruns automatically.

## What makes good review feedback

Every comment must be **actionable** — the author can do something with it.
Apply this filter before posting:

- **Actionable**: "These error messages reference `$XDG_CONFIG_HOME` but the
  code uses `etcetera` now — the hints are stale" → author can fix this
- **Actionable**: A code suggestion fixing the stale hint → one-click apply
- **Not actionable**: "The fix correctly eliminates the duplicate path
  resolution by delegating to `default_config_path()`" → the author knows this

**Rules:**

- **Don't explain what the code does.** The author wrote it. Explanations add
  noise, not value.
- **If the code needs explanation for future readers**, suggest a docstring or
  inline comment — as a code suggestion.
- **Use code suggestions** for anything expressible as replacement lines.
- **Explain *why*** something should change, not just *what*.
- **Distinguish severity** — "should fix" vs. "nice to have".
- **Don't nitpick formatting** — that's what linters are for.
