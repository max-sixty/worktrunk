---
name: running-in-ci
description: CI environment rules for GitHub Actions workflows. Use when operating in CI — covers security, CI monitoring, and PR comment formatting.
---

# Running in CI

## Security

NEVER run commands that could expose secrets (`env`, `printenv`, `set`,
`export`, `cat`/`echo` on config files containing credentials). NEVER include
environment variables, API keys, tokens, or credentials in responses or
comments.

## CI Monitoring

After pushing changes to a PR branch, monitor CI until all checks pass:

1. Monitor with `gh pr checks` or `gh run list --branch <branch>`
2. Wait for completion with `gh run watch`
3. If CI fails, diagnose with `gh run view <run-id> --log-failed`
4. Fix issues, commit, push, and repeat
5. Do not return until CI is green — local tests alone are not sufficient (CI
   runs on Linux, Windows, macOS)

## PR Comment Formatting

Keep PR comments concise. Put detailed analysis (file-by-file breakdowns, code
snippets) inside `<details>` tags with a short summary. The top-level comment
should be a brief overview (a few sentences); all supporting detail belongs in
collapsible sections.

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

## Tone

You are a helpful reviewer raising observations, not a manager assigning work.
Never create checklists or task lists for the PR author. Instead, note what you
found and let the author decide what to act on.

## PR Review Comments

For PR review comments on specific lines (shown as `[Comment on path:line]` in
`<review_comments>`), ALWAYS read that file and examine the code at that line
before answering. The question is about that specific code, not the PR in
general.
