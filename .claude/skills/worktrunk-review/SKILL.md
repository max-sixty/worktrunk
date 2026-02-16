---
name: worktrunk-review
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

**Testing:**

- Are the changes adequately tested?
- Do the tests follow the project's testing conventions (see tests/CLAUDE.md)?

### 4. Submit

Submit **one formal review per run** via `gh pr review`. Never call it multiple
times.

- Always give a verdict: **approve** or **comment**. Don't use "request changes"
  (that implies authority to block).
- **Don't use `gh pr comment`** — use review comments (`gh pr review` or
  `gh api` for inline suggestions) so feedback is threaded with the review.
- Don't repeat suggestions already made by humans or previous bot runs
  (checked in step 1).

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

For small, confident fixes (typos, doc updates, naming, missing imports, minor
refactors), use GitHub suggestion format via `gh api`:

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
