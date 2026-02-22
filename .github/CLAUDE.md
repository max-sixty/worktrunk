# CI Automation Security Model

Five Claude-powered workflows automate issue triage, PR review, CI fixes, dependency updates, and `@worktrunk-bot` mentions.

## Security layers

Two layers protect the repository, in order of importance:

1. **Merge restriction** — only the repo admin (`@max-sixty`) can merge to `main`, enforced by a ruleset. The bot has `write` role (not admin) and cannot merge regardless of review status.
2. **Environment protection** — release secrets (`CARGO_REGISTRY_TOKEN`, `AUR_SSH_PRIVATE_KEY`) are in a protected GitHub Environment (`release`) requiring deployment approval from `@max-sixty`, preventing exfiltration via modified workflows.

Token scoping (principle of least privilege) is a secondary practice, not a security boundary.

## Branch protection on `main`

### Classic branch protection (current)

- **Required reviews**: none (the ruleset is the merge restriction, not approvals)
- **Required status checks**: `test (linux)`, `test (macos)`, `test (windows)`
- **Enforce admins**: off

### Ruleset: "Merge access"

- **Rule**: Restrict updates — only bypass actors can push to or merge into `main`
- **Bypass**: Repository Admin role → **exempt** mode (silent, no checkbox)

The admin/write role distinction is the merge restriction. `worktrunk-bot` has `write` role (`admin: false`, `maintain: false`). Only the repo owner (`@max-sixty`, admin) can merge. This is enforced by the "Restrict updates" rule — GitHub treats merging a PR as a push to the base branch, so restricting updates blocks both direct pushes and PR merges.

The "exempt" bypass mode (added September 2025) silently skips the rule for the admin — no "bypass branch protections" checkbox. This differs from the older "always" bypass mode, which showed a disruptive checkbox on every merge.

**Why not CODEOWNERS?** CODEOWNERS creates a deadlock for solo maintainers: the code owner can't approve their own PRs (GitHub blocks self-approval), and there's no other code owner. Solving this requires `enforce_admins=off` which also bypasses CI checks, or a separate bypass ruleset which adds complexity. The "Restrict updates" ruleset is simpler: one rule, one bypass actor, CI remains enforced for everyone.

**Why not "Restrict who can push" (classic branch protection)?** That feature is only available for organization-owned repositories. This is a personal repo (`max-sixty/worktrunk`).

## Environment protection for release secrets

`CARGO_REGISTRY_TOKEN` and `AUR_SSH_PRIVATE_KEY` are stored in a protected GitHub Environment (`release`) requiring deployment approval from `@max-sixty`. The environment has a deployment branch policy restricting to `v*` tags.

**Why this matters:** If BOT_TOKEN leaks, an attacker can push a branch with a modified workflow that references `${{ secrets.CARGO_REGISTRY_TOKEN }}`. For same-repo PRs (not forks), all repo-level secrets are available to workflows. The modified workflow runs from the PR branch at CI time — before any merge. Branch protection doesn't prevent this because the secret leaks during CI, not at merge time.

Environment protection prevents this: secrets in a protected environment are only available to jobs that reference that environment AND pass the protection rules (manual approval).

## What each workflow needs to do

| Capability | Triage | Mention | Review | CI Fix | Renovate |
|------------|:---:|:---:|:---:|:---:|:---:|
| Read issues/PRs | Yes | Yes | Yes | Yes | — |
| Comment on issues | Yes | Yes | — | — | — |
| Create branches | Yes | Yes | — | Yes | Yes |
| Push commits | Yes | Yes | — | Yes | Yes |
| Create PRs | Yes | Yes | — | Yes | Yes |
| Post PR reviews | — | — | Yes | — | — |
| Resolve review threads | — | — | Yes | — | — |
| Monitor CI | Yes | Yes | — | Yes | Yes |
| **Pushes must trigger CI** | **Yes** | **Yes** | — | **Yes** | **Yes** |

The last row matters for CI: `GITHUB_TOKEN` pushes don't trigger downstream workflows (GitHub prevents infinite loops). Workflows that push code and need CI to run **must** use a PAT. All Claude workflows use `WORKTRUNK_BOT_TOKEN` for consistent identity.

## Token assignment

| Token | Used by | Why |
|-------|---------|-----|
| `WORKTRUNK_BOT_TOKEN` | all Claude workflows | Consistent identity (`worktrunk-bot`). The merge restriction (ruleset) is the security boundary, not token scoping. |
| `CLAUDE_CODE_OAUTH_TOKEN` | all | Authenticates Claude Code to the Anthropic API. |

### Why one token

BOT_TOKEN is equally safe in any workflow: the merge restriction (ruleset) caps the blast radius regardless of which token is used. Using BOT_TOKEN everywhere gives a consistent identity for reviews and comments, and avoids the `github-actions[bot]` branding.

### If a token leaks

| Token | Lifetime | If leaked, attacker can... | ...but cannot |
|-------|----------|---------------------------|---------------|
| `BOT_TOKEN` | Long-lived PAT | Push to unprotected branches, create PRs, approve non-own PRs, impersonate `worktrunk-bot` — **indefinitely** | Merge PRs (ruleset restricts updates to admin), push to `main`, access release secrets (environment-protected). |
| `CLAUDE_CODE_OAUTH_TOKEN` | Long-lived | Run Claude sessions billed to the account | Access GitHub |

`GITHUB_TOKEN` is ephemeral (single job) and automatically scoped by each workflow's `permissions:` block. Not a meaningful leak target.

**BOT_TOKEN is the high-value target.** Mitigations:
- "Restrict updates" ruleset blocks merging by non-admins
- Environment protection blocks exfiltration of release secrets
- Rotate `BOT_TOKEN` periodically

### How tokens interact with `permissions:` and `actions/checkout`

Two independent authentication paths exist in every workflow:

1. **Git CLI** (`git push`): authenticates with the token from `actions/checkout`. When no explicit token is passed, this defaults to `GITHUB_TOKEN` scoped by the `permissions:` block. When an explicit token is passed (e.g. `token: ${{ secrets.WORKTRUNK_BOT_TOKEN }}`), the PAT's scopes apply instead.
2. **GitHub API** (`gh pr create`, `gh api`): `claude-code-action` overwrites the `GITHUB_TOKEN` env var with its `github_token` input (BOT_TOKEN for all Claude workflows).

The review workflow is the only one where these paths diverge: checkout uses the default `GITHUB_TOKEN` (`contents: read`), so git CLI is read-only, while API calls use BOT_TOKEN. All other workflows pass BOT_TOKEN to both paths.

## Prompt injection threat model

| Workflow | Injection surface | Attacker control | Mitigations |
|----------|-------------------|-------------------|-------------|
| **review** | PR diff content | Full (any external PR) | Fixed prompt, merge restriction, `contents: read` on checkout |
| **triage** | Issue body | Partial (structured skill) | Fixed prompt, merge restriction, environment protection |
| **mention** | Comment body | Full | Fixed prompt, merge restriction, fork check on inline review comments |
| **ci-fix** | Failed CI logs | Minimal (must break CI on main) | Fixed prompt, automatic trigger |
| **renovate** | None | None | Fixed prompt, scheduled trigger |

### Secret exfiltration via modified workflows

The most dangerous attack from a leaked BOT_TOKEN is not merging malicious code — it's exfiltrating other secrets:

1. Push a branch with a modified workflow that references `${{ secrets.CARGO_REGISTRY_TOKEN }}`
2. Create a PR — the modified workflow runs from the PR branch
3. For same-repo PRs, all **repo-level** secrets are available
4. Environment-protected secrets are NOT available (require deployment approval)

This is why release secrets must be in a protected environment, not repo-level secrets.

## TODO

### Immediate

- [x] Create "Merge access" ruleset on `main`: "Restrict updates" rule, bypass actor = Repository Admin with **exempt** mode. This prevents `worktrunk-bot` (write role) from merging while giving `@max-sixty` (admin) a frictionless merge button.
- [x] Create `release` GitHub Environment with deployment protection (require `@max-sixty` approval). Add deployment branch/tag allowlist restricting to `v*` tags so arbitrary branches can't request the environment. Leave "prevent self-approvals" off (otherwise the tag pusher can't approve their own release).
- [x] Move `CARGO_REGISTRY_TOKEN` and `AUR_SSH_PRIVATE_KEY` from repo secrets to the `release` environment. Update `release.yaml` `publish-cargo` and `publish-aur` jobs to add `environment: release`.
- [x] Delete `WORKTRUNK_REVIEW_TOKEN` from repo secrets

### Future hardening

- [ ] Consider migrating from PAT to GitHub App for ephemeral tokens (~1 hour lifetime vs indefinite PAT)
- [ ] Consider workflow dispatch isolation: split triage/mention into analysis (GITHUB_TOKEN) + push (separate workflow with BOT_TOKEN) so the token never touches untrusted input
- [ ] Consider disabling "Allow GitHub Actions to create and approve pull requests" in repo settings to prevent GITHUB_TOKEN from ever approving PRs

## Triage ↔ mention handoff

These two workflows explicitly exclude each other to avoid double-processing:
- Issue body contains `@worktrunk-bot` → triage skips, mention handles it
- Issue body does not contain `@worktrunk-bot` → triage handles it, mention ignores it

The mention workflow runs for any user who includes `@worktrunk-bot` — the merge restriction (ruleset) is the safety boundary, not access control on the workflow itself.

## GitHub API: issue_comment vs pull_request_review_comment

GitHub treats PRs as a superset of issues. Comments on a PR arrive via two different event types depending on where they're posted:

- **Conversation tab** → `issue_comment` event. The PR is at `github.event.issue.pull_request` (a truthy object). The PR number is `github.event.issue.number`.
- **Files changed (inline)** → `pull_request_review_comment` event. The PR is at `github.event.pull_request`. There is no `github.event.issue`.

The `claude-mention` workflow handles both with separate checkout steps.

## Rules for modifying workflows

- **No role-based gating**: Workflows should not check `author_association` (OWNER, MEMBER, etc.) to decide whether to run. The merge restriction (ruleset) is the security boundary — Claude cannot merge regardless of who triggers it. Use technical criteria instead: fork detection, loop prevention (exclude the bot's own comments), `@worktrunk-bot` trigger phrase.
- **Adding `allowed_non_write_users`** to a workflow with user-controlled prompts requires security review.
- **All Claude workflows** must include `--append-system-prompt "You are operating in a GitHub Actions CI environment. Use /running-in-ci before starting work."`.
- **Token choice**: All Claude workflows use `BOT_TOKEN` for consistent identity. The merge restriction (ruleset) is the security boundary.
- **`permissions:` block**: Set `contents: read` for read-only workflows.
- **Sensitive secrets** must be in protected environments, never repo-level.
- **Rotate `BOT_TOKEN`** periodically.
