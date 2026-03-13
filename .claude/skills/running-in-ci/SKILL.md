---
name: running-in-ci
description: CI environment rules for GitHub Actions workflows. Use when operating in CI — covers security, CI monitoring, comment formatting, and investigating session logs from other runs.
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

### Before creating a PR

**Check for existing work.** Before creating a new branch or PR, check whether
someone (including the bot) has already opened a PR for the same issue or topic:

```bash
# Check open PRs — look for related titles and branches
gh pr list --state open --json number,title,headRefName --jq '.[] | "#\(.number) [\(.headRefName)]: \(.title)"'

# Check for fix branches
git branch -r --list 'origin/fix/*'
```

If an existing PR addresses the same problem, work on that PR instead of
creating a duplicate. Comment on the existing PR or the issue linking to it.

## Pushing to PR Branches

**Always use `git push` without specifying a remote.** The workflow uses
`gh pr checkout` which configures branch tracking to the correct remote —
including for fork PRs. Specifying `origin` explicitly bypasses this and can
push to the wrong place.

If pushing fails (e.g., fork PR with "Allow edits from maintainers" disabled),
fall back to posting suggested changes as code snippets in a comment.

When posting code from work you did locally, do not reference commit SHAs from
temporary or deleted branches — those links will 404. Post the code inline
instead.

## CI Monitoring

After pushing changes to a PR branch, you **must** wait for CI before saying
"done" or reporting completion. A push without green CI is not finished work.

1. Push your changes
2. Run `gh pr checks <number> --required` once
3. If checks are still running, poll with `gh pr checks <number> --required`
   every 60 seconds until all required checks complete (this may take up to
   10 minutes). Non-required checks (e.g., benchmarks) are ignored — do not
   wait for them.
4. If a required check fails, diagnose with `gh run view <run-id> --log-failed`,
   fix issues, commit, push, and repeat from step 2
5. After all required checks pass, also check `codecov/patch`. Although it is
   marked non-required in GitHub, this repo treats it as mandatory (see
   CLAUDE.md). Run `gh pr checks <number>` (without `--required`) and look for
   the `codecov/patch` row. If it hasn't completed yet, continue polling every
   60 seconds. If it fails, investigate and fix the coverage gap before
   reporting completion.
6. Only after all required checks **and** `codecov/patch` pass, report
   completion

**Never** post a "done" or "fixed" comment before CI passes. Local tests alone
are not sufficient — CI runs on Linux, Windows, and macOS. If you report
completion and CI later fails, the user has to come back and ask you to fix it
again.

Avoid `gh run watch` — it can hang indefinitely. Use the poll loop above
instead, which has a natural bound on CI completion time.

### Verifying local test failures before pushing

When running local tests before pushing and some tests fail, do **not**
characterize them as "pre-existing" or "environment-dependent" without
evidence. The same grounded-analysis rule from the Thoroughness section applies
here — check main branch CI history before dismissing failures:

```bash
gh api "repos/{owner}/{repo}/actions/runs?branch=main&status=completed&per_page=3" \
  --jq '.workflow_runs[] | {conclusion, created_at: .created_at}'
```

If you cannot verify, say "I haven't confirmed whether these failures are
pre-existing" rather than asserting they are.

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

Keep comments concise. Put detailed analysis inside `<details>` tags with a
short summary. The top-level comment should be a brief overview (a few
sentences); all supporting detail belongs in collapsible sections.

Use `<details>` tags when a comment has supporting detail that isn't the main
point — multi-section analyses, diagnostic breakdowns, or supplementary context
around a short conclusion. The reader should get the gist without expanding.
Don't collapse content that *is* the answer: if someone asked for a survey or
detailed analysis, the full response is the substance, not boilerplate, and
should stay inline.

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

## Investigating Other CI Runs

When asked to diagnose what a bot did in a previous CI run, the primary evidence
is the session log artifact — not the console output. Console output
(`gh run view <id> --log`) contains only workflow boilerplate because
`show_full_output` defaults to `false`. The actual conversation is in the
artifact.

### Downloading session logs

All Claude workflows upload session logs as artifacts named
`claude-session-logs`. Download with:

```bash
gh run download <run-id> -n claude-session-logs -D /tmp/session-logs-<run-id>
```

The artifact contains JSONL files under a path like
`-home-runner-work-worktrunk-worktrunk/<session-id>.jsonl`.

### Parsing session logs

Each JSONL line is a message with a `type` field (`user`, `assistant`, `system`).

```bash
# Skills loaded
jq -r 'select(.type == "assistant") | .message.content[]? |
  select(.type == "tool_use" and .name == "Skill") | .input.skill' <FILE>.jsonl

# Tool calls
jq -r 'select(.type == "assistant") | .message.content[]? |
  select(.type == "tool_use") |
  "\(.name): \(.input | tostring | .[0:100])"' <FILE>.jsonl

# Assistant reasoning
jq -r 'select(.type == "assistant") | .message.content[]? |
  select(.type == "text") | .text' <FILE>.jsonl
```

### Finding the right run

Multiple workflows may trigger on the same event. Use the event type to narrow:

```bash
gh api 'repos/{owner}/{repo}/actions/runs?per_page=30' \
  --jq '.workflow_runs[] | select(.name | startswith("claude-")) |
    {id, name, event, head_branch, created_at, conclusion}'
```

Check which runs have artifacts before downloading:

```bash
gh api repos/{owner}/{repo}/actions/runs/<run-id>/artifacts \
  --jq '.artifacts[] | {name, size_in_bytes}'
```

Review-response runs triggered by `pull_request_review` or
`pull_request_review_comment` events sometimes produce no artifact when the
session is very short.

## Thoroughness — Grounded Analysis

CI runs are not interactive chat. There is no back-and-forth — the user reads
your output after the session ends. Every claim must be grounded in evidence you
actually examined.

- **Do the work, don't speculate.** If you have access to logs, code, or API
  data, read it before drawing conclusions. "This suggests X may be the cause"
  is not acceptable when you can check whether X is actually the cause.
- **Never claim a CI failure is "pre-existing" or "unrelated" without
  evidence.** Before characterizing any failure this way, check main branch CI
  history (e.g., `gh api "repos/{owner}/{repo}/actions/runs?branch=main&status=completed&per_page=5"`)
  to verify the same test fails there. If you cannot verify, say "I haven't
  confirmed whether this is pre-existing" rather than asserting it is.
- **Show evidence.** Cite specific log lines, file paths, commit SHAs, or API
  responses. A conclusion without evidence is speculation.
- **Trace causation, don't guess at correlation.** If two things co-occur, find
  the mechanism — don't say "this may be related."
- **Distinguish what you verified from what you inferred.** If you couldn't
  verify something (e.g., logs weren't available), say so explicitly rather than
  hedging with "may" or "suggests."
- **Check artifacts, not just console logs.** Console output from Claude runs is
  hidden by default. Session log artifacts are the primary evidence source — see
  "Investigating Other CI Runs" above.

The user can't ask follow-up questions in the same session. Treat every response
as your final answer.

## Tone

You are a helpful reviewer raising observations, not a manager assigning work.
Never create checklists or task lists for the PR author. Instead, note what you
found and let the author decide what to act on.

## PR Review Comments

For PR review comments on specific lines (shown as `[Comment on path:line]` in
`<review_comments>`), ALWAYS read that file and examine the code at that line
before answering. The question is about that specific code, not the PR in
general.
