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

After pushing changes to a PR branch, monitor CI until all checks pass:

1. Monitor with `gh pr checks` or `gh run list --branch <branch>`
2. Wait for completion with `gh run watch`
3. If CI fails, diagnose with `gh run view <run-id> --log-failed`
4. Fix issues, commit, push, and repeat
5. Do not return until CI is green — local tests alone are not sufficient (CI
   runs on Linux, Windows, macOS)

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

- **Repository files** — link to the file on GitHub, preferably with a commit
  SHA: [`docs/content/hook.md`](https://github.com/max-sixty/worktrunk/blob/ab1c2d3/docs/content/hook.md),
  not just `docs/content/hook.md`
- **Issues and PRs** — use `#123` shorthand (GitHub auto-links these)
- **Specific lines** — always use a permalink (commit SHA) so the link remains
  accurate:
  [`src/cli/mod.rs#L42`](https://github.com/max-sixty/worktrunk/blob/ab1c2d3/src/cli/mod.rs#L42)
- **External resources** — always use `[text](url)` format

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

Claude tends to mangle shell quoting in CI. Two common failure modes:

1. **`$` in GraphQL queries** — `gh api graphql -f query='...$var...'` fails
   because Claude corrupts the `$` signs. Write queries to a temp file instead:

   ```bash
   cat > /tmp/query.graphql << 'GRAPHQL'
   query($owner: String!, $repo: String!, $name: String!) {
     repository(owner: $owner, name: $name) { ... }
   }
   GRAPHQL

   gh api graphql -F query=@/tmp/query.graphql -f owner="$OWNER" -f name="$NAME"
   ```

2. **`!` in comment/body text** — `gh issue comment N --body "Thanks!"` gets
   over-escaped to `Thanks\!` because `!` is a bash history expansion character.
   Use a heredoc:

   ```bash
   gh issue comment N --body "$(cat <<'EOF'
   Comment text here — no escaping needed.
   EOF
   )"
   ```

**General rule:** When a `gh` command argument contains `$` or `!`, use either
a temp file (`-F field=@file`) or a heredoc with a quoted delimiter (`<<'EOF'`).

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
