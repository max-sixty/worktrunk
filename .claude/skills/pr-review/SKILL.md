---
name: pr-review
description: Reviews a pull request for idiomatic Rust, project conventions, and code quality. Use when asked to review a PR or when running as an automated PR reviewer.
argument-hint: "[PR number]"
---

# Worktrunk PR Review

Review a pull request to worktrunk, a Rust CLI tool for managing git worktrees.

**PR to review:** $ARGUMENTS

## Setup

Load these skills first:

1. `/reviewing-code` — systematic review checklist (design review, universal
   principles, completeness)
2. `/developing-rust` — Rust idioms and patterns

## Workflow

Follow these steps in order.

### 1. Pre-flight checks

Before reading the diff, run cheap checks to avoid redundant work. Shell state
doesn't persist between tool calls — re-derive `REPO` in each bash invocation or
combine commands.

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
BOT_LOGIN=$(gh api user --jq '.login')

# Check if bot already approved this exact revision
APPROVED_SHA=$(gh pr view <number> --json reviews \
  --jq "[.reviews[] | select(.state == \"APPROVED\" and .author.login == \"$BOT_LOGIN\") | .commit.oid] | last")
HEAD_SHA=$(gh pr view <number> --json commits --jq '.commits[-1].oid')
```

If `APPROVED_SHA == HEAD_SHA`, exit silently — this revision is already approved.

If the bot approved a previous revision (`APPROVED_SHA` exists but differs from
`HEAD_SHA`), check the incremental changes since the last approval:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
gh api "repos/$REPO/compare/$APPROVED_SHA...$HEAD_SHA" \
  --jq '{total: ([.files[] | .additions + .deletions] | add), files: [.files[] | "\(.filename)\t+\(.additions)/-\(.deletions)"]}'
```

If the new changes are trivial, skip the full review (steps 2-3) and do not
re-approve — the existing approval stands. Still proceed to step 4 to resolve
any bot threads that the trivial changes addressed, then exit. Rough heuristic:
changes under ~20 added+deleted lines that don't introduce new functions, types,
or control flow are typically trivial (review feedback addressed, CI/formatting
fixes, small corrections). Only proceed with a full review and potential
re-approval for non-trivial changes (new logic, architectural changes,
significant additions).

Then check existing review comments to avoid repeating prior feedback:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
gh api "repos/$REPO/pulls/<number>/comments" --paginate --jq '.[].body'
gh api "repos/$REPO/pulls/<number>/reviews" --jq '.[] | select(.body != "") | .body'
```

### 2. Read and understand the change

1. Read the PR diff with `gh pr diff <number>`.
2. Read the changed files in full (not just the diff) to understand context.

### 3. Review

Follow the `reviewing-code` skill's structure: design review first, then
tactical checklist.

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

### 4. Resolve handled suggestions

After reviewing the code, check if any unresolved review threads from the bot
have been addressed. For each unresolved bot thread, you've already read the
file during review — if the suggestion was applied or the issue was otherwise
fixed, resolve the thread:

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

### 5. Submit

Submit **one formal review per run** via `gh pr review`. Never call it multiple
times.

- Always give a verdict: **approve** or **comment**. Don't use "request changes"
  (that implies authority to block).
- **Don't use `gh pr comment`** — use review comments (`gh pr review` or
  `gh api` for inline suggestions) so feedback is threaded with the review.
- Don't repeat suggestions already made by humans or previous bot runs
  (checked in step 1).
- **Default to code suggestions** for specific fixes — see "Inline suggestions"
  below. Prose comments are for changes too large or uncertain for a suggestion.

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

## How to provide feedback

- Use inline review comments for specific code issues. Prefer suggestion format
  (see above) for narrow fixes.
- Be constructive and explain *why* something should change, not just *what*.
- Distinguish between suggestions (nice to have) and issues (should fix).
- Don't nitpick formatting — that's what linters are for.
