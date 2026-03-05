---
name: running-in-ci
description: CI environment rules for GitHub Actions workflows. Use when operating in CI — covers security, CI monitoring, and comment formatting.
---

# Running in CI

## First Steps — Read Context

When triggered by a comment or issue, read the full context before responding.
The prompt provides a URL — extract the PR/issue number from it.

For PRs:

```bash
gh pr view <number> --json title,body,comments,reviews,state,statusCheckRollup
gh pr diff <number>
gh pr checks <number>
```

For issues:

```bash
gh issue view <number> --json title,body,comments,state
```

Read the triggering comment, the PR/issue description, the diff (for PRs), and
recent comments to understand the full conversation before taking action.

## Security

NEVER run commands that could expose secrets (`env`, `printenv`, `set`,
`export`, `cat`/`echo` on config files containing credentials). NEVER include
environment variables, API keys, tokens, or credentials in responses or
comments.

## PR Creation

When the triggering comment asks for a PR (e.g., "make a new PR", "open a PR",
"create a PR"), create it directly with `gh pr create`. The comment is the
user's explicit request — don't downgrade it to a compare link.

## CI Monitoring

After pushing changes to a PR branch, you **must** wait for CI before saying
"done" or reporting completion. A push without green CI is not finished work.

1. Push your changes
2. Wait for CI completion with `gh run watch` or poll `gh pr checks <number>`
3. If CI fails, diagnose with `gh run view <run-id> --log-failed`
4. Fix issues, commit, push, and repeat from step 2
5. Only after all checks pass, report completion

**Never** post a "done" or "fixed" comment before CI passes. Local tests alone
are not sufficient — CI runs on Linux, Windows, and macOS. If you report
completion and CI later fails, the user has to come back and ask you to fix it
again.

## Replying to Comments

Prefer replying in context rather than creating a new top-level comment:

- **Inline review comments** (URLs containing `#discussion_r`): Reply in the
  review thread using `gh api`, not as a top-level conversation comment. Use the
  review comment ID from the prompt:
  ```bash
  cat > /tmp/reply.md << 'EOF'
  Your response here
  EOF
  gh api repos/{owner}/{repo}/pulls/{number}/comments/{comment_id}/replies \
    -F body=@/tmp/reply.md
  ```
  This keeps the discussion co-located with the code it references.

- **Conversation comments** (URLs containing `#issuecomment-`): Post a regular
  comment — GitHub doesn't support threading for these, so a new comment is
  correct.

## Comment Formatting

Keep comments concise. Put detailed analysis (file-by-file breakdowns, code
snippets) inside `<details>` tags with a short summary. The top-level comment
should be a brief overview (a few sentences); all supporting detail belongs in
collapsible sections.

### Use Links

When referencing files, issues, PRs, or docs, always use markdown links so
readers can click through — never leave them as plain text.

Prefer **permalinks** (URLs with a commit SHA) over branch-based links
(`blob/main/...`). Permalinks stay valid even when files move or lines shift.
This is especially important for line references — a `blob/main/...#L42` link
breaks as soon as the line numbers change. On GitHub, pressing `y` on any file
view copies the permalink.

- **Repository files** — link to the file on GitHub:
  [`docs/content/hook.md`](https://github.com/max-sixty/worktrunk/blob/main/docs/content/hook.md),
  not just `docs/content/hook.md`
- **Issues and PRs** — use `#123` shorthand (GitHub auto-links these)
- **Specific lines** — link with a line fragment:
  [`src/cli/mod.rs#L42`](https://github.com/max-sixty/worktrunk/blob/main/src/cli/mod.rs#L42)
- **External resources** — always use `[text](url)` format

For file-level links, `blob/main/...` is acceptable since file paths are stable.
For **line references**, always use a permalink with a commit SHA
(`blob/<sha>/...#L42`) — line numbers shift frequently and branch-based line
links go stale fast.

Example:

```
<details><summary>Detailed findings (6 files)</summary>

...details here...

</details>
```

Do not add job links, branch links, or other footers at the bottom of your
comment. `claude-code-action` automatically adds these to the comment header.
Adding them yourself creates duplicates and broken links (the action deletes
unused branches after the run).

## Shell Quoting in `gh` Commands

Shell expansion corrupts `$` and `!` in command arguments. **This is a Claude
Code bug** — bash history expansion mangles `!` in double-quoted strings (e.g.,
`format!` becomes `format\!`) and it's the most common source
of broken bot comments.

**Rule: always use a temp file for comment/reply bodies and other shell-sensitive
arguments.** Never pass the body directly as a `-f body="..."` argument.

```bash
# Posting a comment — ALWAYS use a file
cat > /tmp/comment.md << 'EOF'
Fixed — the `format!` macro needed its arguments on separate lines.
CI is now green across all platforms.
EOF
gh pr comment 1286 -F /tmp/comment.md

# Replying to a review comment — ALWAYS use a file
cat > /tmp/reply.md << 'EOF'
Good catch! Updated to use `assert_eq!` instead.
EOF
gh api repos/{owner}/{repo}/pulls/{number}/comments/{id}/replies \
  -F body=@/tmp/reply.md

# GraphQL with $ — write query to a file, pass with -F
cat > /tmp/query.graphql << 'GRAPHQL'
query($owner: String!, $repo: String!) { ... }
GRAPHQL
gh api graphql -F query=@/tmp/query.graphql -f owner="$OWNER"

# jq with ! — use a file
cat > /tmp/jq_filter << 'EOF'
select(.status != "COMPLETED")
EOF
gh api ... --jq "$(cat /tmp/jq_filter)"
```

**Key details:**
- Use `<< 'EOF'` (single-quoted delimiter) to prevent all shell expansion
- Use `-F body=@/tmp/reply.md` (capital `-F` with `@` prefix) to read from file
- For `gh pr comment` and `gh issue comment`, use `-F /tmp/comment.md` (the
  `-F` flag reads body from file)

## Keeping PR Titles and Descriptions Current

When you revise a PR's code in response to review feedback, check whether the
title and description still accurately describe the changes. If the approach
changed (e.g., from "exclude all X" to "add targeted exclusions for X"), update
the title and body to match. A reviewer reading the description before the diff
should not be confused by stale framing.

Use the GitHub API to update:

```bash
gh api repos/{owner}/{repo}/pulls/{number} -X PATCH \
  -f title="new title" -F body=@/tmp/updated-body.md
```

## Atomic PRs

When creating PRs, split unrelated changes into separate PRs — one concern per
PR. For example, a skill file fix and a workflow dependency cleanup are two
independent changes and should be two PRs, even if discovered in the same
session. This makes PRs easier to review, revert, and bisect.

A good test: if one change could be reverted without affecting the other, they
belong in separate PRs.

## Tone

You are a helpful reviewer raising observations, not a manager assigning work.
Never create checklists or task lists for the PR author. Instead, note what you
found and let the author decide what to act on.

## PR Review Comments

For PR review comments on specific lines (shown as `[Comment on path:line]` in
`<review_comments>`), ALWAYS read that file and examine the code at that line
before answering. The question is about that specific code, not the PR in
general.
