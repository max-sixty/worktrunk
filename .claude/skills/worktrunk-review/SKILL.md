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

Then read CLAUDE.md (project root) to understand project-specific conventions.

## Instructions

1. Read the PR diff with `gh pr diff <number>`.
2. Read the changed files in full (not just the diff) to understand context.
3. Follow the `reviewing-code` skill's structure: design review first, then
   tactical checklist.

## What to review

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

## Review discipline

- Submit **one formal review per run** via `gh pr review` (approve, request
  changes, or comment). Never call `gh pr review` multiple times.
- **Don't use `gh pr comment`** — the CI action manages the summary comment
  (sticky comment from Claude's stdout).
- Always either **approve** or **comment** — never leave a PR without a
  verdict. Don't use "request changes" (that implies authority to block).
- **Before approving**, check if the bot already approved this revision:
  ```bash
  APPROVED_SHA=$(gh pr view <number> --json reviews --jq '[.reviews[] | select(.state == "APPROVED") | .commit.oid] | last')
  HEAD_SHA=$(gh pr view <number> --json commits --jq '.commits[-1].oid')
  ```
  If `APPROVED_SHA == HEAD_SHA`, skip the redundant re-approval.

## LGTM behavior

When the PR has no issues worth raising:

1. Approve with no body — the approval itself is the signal:
   ```bash
   gh pr review <number> --approve
   ```
2. Add a thumbs-up reaction to the PR:
   ```bash
   gh api repos/{owner}/{repo}/issues/<number>/reactions -f content="+1"
   ```
3. Write nothing to stdout — the approval speaks for itself. Only produce
   stdout when you have actionable feedback (it becomes the sticky comment).

## Inline suggestions

For small, confident fixes (typos, doc updates, naming, missing imports, minor
refactors), use GitHub suggestion format via `gh api`:

`````bash
gh api repos/{owner}/{repo}/pulls/<number>/reviews \
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
