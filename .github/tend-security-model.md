# CI Automation Security Model

Generic security model for Claude-powered CI workflows. This document covers
the principles; each adopting repo should document its specific configuration
(admin accounts, token names, protected environments) in its own `.github/CLAUDE.md`.

## Security layers

Two layers protect a repository, in order of importance:

1. **Merge restriction** — only designated admins can merge to the default
   branch, enforced by a ruleset or branch protection. The bot has write access
   (not admin) and cannot merge regardless of review status.
2. **Environment protection** — release secrets (registry tokens, signing keys)
   are in a protected GitHub Environment requiring deployment approval,
   preventing exfiltration via modified workflows.

Token scoping (principle of least privilege) is a secondary practice, not a
security boundary.

## What each workflow needs to do

| Capability | Triage | Mention | Review | CI Fix | Nightly | Renovate |
|------------|:---:|:---:|:---:|:---:|:---:|:---:|
| Read issues/PRs | Yes | Yes | Yes | Yes | Yes | — |
| Comment on issues | Yes | Yes | Yes | — | Yes | — |
| Create branches | Yes | Yes | Yes | Yes | Yes | Yes |
| Push commits | Yes | Yes | Yes | Yes | Yes | Yes |
| Create PRs | Yes | Yes | — | Yes | Yes | Yes |
| Post PR reviews | — | — | Yes | — | — | — |
| Resolve review threads | — | — | Yes | — | — | — |
| Monitor CI | Yes | Yes | Yes | Yes | Yes | Yes |
| **Pushes must trigger CI** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** |

The last row matters: `GITHUB_TOKEN` pushes don't trigger downstream workflows
(GitHub prevents infinite loops). Workflows that push code and need CI to run
**must** use a PAT or GitHub App installation token.

## Token assignment

Use a single bot token across all Claude workflows for consistent identity.
The merge restriction (ruleset) caps blast radius regardless of which token
is used.

Two tokens are needed:

| Token | Purpose |
|-------|---------|
| Bot token (PAT or App) | GitHub API and git operations. Consistent bot identity. |
| Claude OAuth token | Authenticates Claude Code to the Anthropic API. |

### Why one bot token

The bot token is equally safe in any workflow because the merge restriction
caps the blast radius. Using a single token gives consistent identity for
reviews and comments and avoids the `github-actions[bot]` branding.

### If a token leaks

| Token | Lifetime | If leaked, attacker can... | ...but cannot |
|-------|----------|---------------------------|---------------|
| Bot token (PAT) | Long-lived | Push to unprotected branches, create PRs, impersonate bot — **indefinitely** | Merge PRs (merge restriction), push to default branch, access release secrets (environment-protected) |
| Bot token (App) | ~1 hour | Same as PAT, but only until token expires | Same + token auto-expires |
| Claude OAuth | Long-lived | Run Claude sessions billed to the account | Access GitHub |

`GITHUB_TOKEN` is ephemeral (single job) and automatically scoped by each
workflow's `permissions:` block. Not a meaningful leak target.

**Bot token is the high-value target.** Mitigations:
- Merge restriction blocks merging by non-admins
- Environment protection blocks exfiltration of release secrets
- With a GitHub App, tokens auto-expire in ~1 hour
- With a PAT, rotate periodically

### How tokens interact with `permissions:` and `actions/checkout`

Two independent authentication paths exist in every workflow:

1. **Git CLI** (`git push`): authenticates with the token from
   `actions/checkout`. When no explicit token is passed, this defaults to
   `GITHUB_TOKEN` scoped by the `permissions:` block. When an explicit token
   is passed, that token's scopes apply instead.
2. **GitHub API** (`gh pr create`, `gh api`): `claude-code-action` overwrites
   the `GITHUB_TOKEN` env var with its `github_token` input.

All workflows should pass the bot token to both paths.

## Prompt injection threat model

| Workflow | Injection surface | Attacker control | Mitigations |
|----------|-------------------|-------------------|-------------|
| **review** | PR diff content, review body on bot PRs | Full (any PR) / Medium (reviewers) | Fixed prompt, merge restriction |
| **triage** | Issue body | Partial (structured skill) | Fixed prompt, merge restriction, environment protection |
| **mention** | Comment body on any issue/PR | Full | Fixed prompt, merge restriction, engagement verification |
| **ci-fix** | Failed CI logs | Minimal (must break CI on default branch) | Fixed prompt, automatic trigger |
| **renovate** | None | None | Fixed prompt, scheduled trigger |

### Secret exfiltration via modified workflows

The most dangerous attack from a leaked bot token is not merging malicious
code — it's exfiltrating other secrets:

1. Push a branch with a modified workflow that references a secret
2. Create a PR — the modified workflow runs from the PR branch
3. For same-repo PRs, all **repo-level** secrets are available
4. Environment-protected secrets are NOT available (require deployment approval)

This is why release secrets must be in a protected environment, not repo-level
secrets.

## Future hardening

- Migrate from PAT to GitHub App for ephemeral tokens (~1 hour vs indefinite)
- Workflow dispatch isolation: split workflows into analysis (GITHUB_TOKEN) +
  push (separate workflow with bot token) so the token never touches untrusted
  input
- Disable "Allow GitHub Actions to create and approve pull requests" in repo
  settings

## GitHub API: event types for PR comments

GitHub treats PRs as a superset of issues. Comments on a PR arrive via
different event types depending on where they're posted:

- **Conversation tab** → `issue_comment` event. The PR is at
  `github.event.issue.pull_request`. The PR number is
  `github.event.issue.number`.
- **Files changed (inline)** → `pull_request_review_comment` event. The PR is
  at `github.event.pull_request`. There is no `github.event.issue`.
- **Review submission** → `pull_request_review` event (type: `submitted`). The
  review is at `github.event.review`. The PR is at
  `github.event.pull_request`.

Individual inline comments from a review also fire as separate
`pull_request_review_comment` events.

## Rules for modifying workflows

- **No role-based gating**: Don't check `author_association` (OWNER, MEMBER,
  etc.) to decide whether to run. The merge restriction is the security
  boundary. Use technical criteria: fork detection, loop prevention, trigger
  phrases.
- **Adding `allowed_non_write_users`** to a workflow with user-controlled
  prompts requires security review.
- **All Claude workflows** must include
  `--append-system-prompt "You are operating in a GitHub Actions CI environment. Use /tend-running-in-ci before starting work."`.
- **Token choice**: All Claude workflows use the bot token for consistent
  identity.
- **`permissions:` block**: Set `contents: read` for read-only workflows.
- **Sensitive secrets** must be in protected environments, never repo-level.
