---
name: running-in-ci
description: CI environment rules for GitHub Actions workflows. Use when operating in CI — covers security, CI monitoring, comment formatting, and investigating session logs from other runs.
metadata:
  internal: true
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

## Tool Permission Errors

If a tool call (Edit, Write, etc.) is denied by the permission system, do not
retry the same tool — the permission won't change mid-session. Immediately fall
back to an alternative: use `git apply` with a patch, `sed`, or post the change
as a code snippet in a comment.

## PR Creation

When asked to create a PR, use `gh pr create` directly.

Before creating a branch or PR, check for existing work:

```bash
gh pr list --state open --json number,title,headRefName --jq '.[] | "#\(.number) [\(.headRefName)]: \(.title)"'
git branch -r --list 'origin/fix/*'
```

If an existing PR addresses the same problem, work on that PR instead.

## Pushing to PR Branches

Always use `git push` without specifying a remote — `gh pr checkout` configures
tracking to the correct remote, including for fork PRs. Specifying `origin`
explicitly can push to the wrong place.

If pushing fails (fork PR with edits disabled), fall back to posting code
snippets in a comment. Don't reference commit SHAs from temporary branches —
post code inline.

## CI Monitoring

After pushing, wait for CI before reporting completion.

**Use `run_in_background: true`** for the polling loop so it does not block the
session. When the background task completes you will be notified — check the
result and take any follow-up action (dismiss approval, post analysis) at that
point.

```bash
# Run with Bash tool's run_in_background: true
for i in $(seq 1 10); do
  sleep 60
  if ! gh pr checks <number> --required 2>&1 | grep -q 'pending\|queued\|in_progress'; then
    gh pr checks <number> --required
    exit 0
  fi
done
echo "CI still running after 10 minutes"
exit 1
```

1. Poll `gh pr checks <number> --required` every 60 seconds until all required
   checks complete (up to ~10 minutes). Ignore non-required checks (benchmarks).
2. If a required check fails, diagnose with `gh run view <run-id> --log-failed`,
   fix, commit, push, repeat.
3. After required checks pass, poll `codecov/patch` separately — it is
   mandatory despite being marked non-required. Use a polling loop (up to
   ~5 minutes) since codecov often reports after the required checks finish:
   ```bash
   for i in $(seq 1 5); do
     CODECOV=$(gh pr checks <number> 2>&1 | grep 'codecov/patch' || true)
     if echo "$CODECOV" | grep -q 'pass'; then
       echo "codecov/patch passed"; exit 0
     elif echo "$CODECOV" | grep -q 'fail'; then
       echo "codecov/patch FAILED"; exit 1
     fi
     sleep 60
   done
   echo "codecov/patch not reported after 5 minutes"
   exit 1
   ```
   If it fails, investigate with `task coverage` and
   `cargo llvm-cov report --show-missing-lines | grep <file>`.
4. Report completion only after all required checks **and** `codecov/patch` pass.

Never report "done" before CI passes — CI runs on Linux, Windows, and macOS.
Avoid `gh run watch` and `gh pr checks --watch` — both can hang indefinitely.

Before dismissing local test failures as "pre-existing", check main branch CI:

```bash
gh api "repos/{owner}/{repo}/actions/runs?branch=main&status=completed&per_page=3" \
  --jq '.workflow_runs[] | {conclusion, created_at: .created_at}'
```

If you cannot verify, say "I haven't confirmed whether these failures are
pre-existing."

## Replying to Comments

Reply in context rather than creating new top-level comments:

- **Inline review comments** (`#discussion_r`): To read a single review
  comment, use the comment ID **without** the PR number in the path:
  ```bash
  gh api repos/{owner}/{repo}/pulls/comments/{comment_id}
  ```
  To reply:
  ```bash
  cat > /tmp/reply.md << 'EOF'
  Your response here
  EOF
  gh api repos/{owner}/{repo}/pulls/{number}/comments/{comment_id}/replies \
    -F body=@/tmp/reply.md
  ```

- **Conversation comments** (`#issuecomment-`): Post a regular comment (GitHub
  doesn't support threading).

## Comment Formatting

Keep comments concise. Put supporting detail inside `<details>` tags — the
reader should get the gist without expanding. Don't collapse content that *is*
the answer (e.g., a requested analysis).

```
<details><summary>Detailed findings (6 files)</summary>

...details here...

</details>
```

Always use markdown links for files, issues, PRs, and docs. Prefer permalinks
(commit SHA URLs) over branch-based links for line references — line numbers
shift and `blob/main/...#L42` links go stale.

- **Files**: link to GitHub (`blob/main/...` for file-level,
  `blob/<sha>/...#L42` for lines)
- **Issues/PRs**: `#123` shorthand
- **External**: `[text](url)` format

Don't add job links or footers — `claude-code-action` adds these automatically.

## Shell Quoting

Shell expansion corrupts `$` and `!` in arguments (bash history expansion
mangles `!` in double-quoted strings). Always use a temp file for comment bodies
and shell-sensitive arguments:

```bash
# Comments — ALWAYS use a file
cat > /tmp/comment.md << 'EOF'
Fixed — the `format!` macro needed its arguments on separate lines.
EOF
gh pr comment 1286 -F /tmp/comment.md

# Review replies — ALWAYS use a file
cat > /tmp/reply.md << 'EOF'
Good catch! Updated to use `assert_eq!` instead.
EOF
gh api repos/{owner}/{repo}/pulls/{number}/comments/{id}/replies \
  -F body=@/tmp/reply.md

# GraphQL — write query to a file
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

Use `<< 'EOF'` (single-quoted delimiter) to prevent expansion. For file-based
bodies: `gh pr comment` uses `-F /tmp/file` (path directly), while `gh api`
uses `-F body=@/tmp/file` (field assignment with `@` prefix).

## Keeping PR Titles and Descriptions Current

When revising code after review feedback, update the title and description if
the approach changed:

```bash
gh api repos/{owner}/{repo}/pulls/{number} -X PATCH \
  -f title="new title" -F body=@/tmp/updated-body.md
```

## Atomic PRs

Split unrelated changes into separate PRs — one concern per PR. If one change
could be reverted without affecting the other, they belong in separate PRs.

## Investigating Other CI Runs

The primary evidence for diagnosing bot behavior is the session log artifact —
not console output (`show_full_output` defaults to `false`).

```bash
gh run download <run-id> -n claude-session-logs -D /tmp/session-logs-<run-id>
```

The artifact contains JSONL files (path like
`-home-runner-work-worktrunk-worktrunk/<session-id>.jsonl`). Each line has a
`type` field (`user`, `assistant`, `system`).

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

Find the right run among multiple workflows:

```bash
gh api 'repos/{owner}/{repo}/actions/runs?per_page=30' \
  --jq '.workflow_runs[] | select(.name | startswith("claude-")) |
    {id, name, event, head_branch, created_at, conclusion}'
```

Check for artifacts before downloading:

```bash
gh api repos/{owner}/{repo}/actions/runs/<run-id>/artifacts \
  --jq '.artifacts[] | {name, size_in_bytes}'
```

Review-response runs triggered by `pull_request_review` or
`pull_request_review_comment` events sometimes produce no artifact when the
session is very short.

## Grounded Analysis

CI runs are not interactive — every claim must be grounded in evidence. The user
can't ask follow-up questions; treat every response as your final answer.

Read logs, code, and API data before drawing conclusions. Show evidence: cite
log lines, file paths, commit SHAs. Trace causation — if two things co-occur,
find the mechanism rather than saying "this may be related." Never claim a
failure is "pre-existing" without checking main branch CI history. Distinguish
what you verified from what you inferred.

## Tone

Raise observations, don't assign work. Never create checklists or task lists
for the PR author.

## PR Review Comments

For review comments on specific lines (`[Comment on path:line]`), read that file
and examine the code at that line before answering.

When the GitHub API returns a `diff_hunk`, the reviewer's comment targets the
**last line** of that hunk. Use this to disambiguate when multiple candidates
exist nearby — match the reviewer's request against the specific anchored line,
not the surrounding region.
