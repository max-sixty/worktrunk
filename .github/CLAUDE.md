# CI Automation Security Model — Worktrunk

See [tend-security-model.md](tend-security-model.md) for the generic security model
(security layers, token management, prompt injection, event types, workflow
modification rules). This file documents worktrunk-specific configuration.

## Bot identity

`worktrunk-bot` is a regular GitHub user account (PAT-based), not a GitHub App.
Workflows check `user.login == 'worktrunk-bot'` directly.

## Tokens

| Token | Used by |
|-------|---------|
| `WORKTRUNK_BOT_TOKEN` | All Claude workflows — consistent identity (`worktrunk-bot`) |
| `CLAUDE_CODE_OAUTH_TOKEN` | All — authenticates Claude Code to the Anthropic API |

## Merge restriction

### Ruleset: "Merge access"

- **Rule**: Restrict updates — only bypass actors can push to or merge into `main`
- **Bypass**: Repository Admin role → **exempt** mode (silent, no checkbox)

`worktrunk-bot` has `write` role (`admin: false`, `maintain: false`). Only the
repo owner (`@max-sixty`, admin) can merge. GitHub treats merging a PR as a push
to the base branch, so restricting updates blocks both direct pushes and PR
merges.

The "exempt" bypass mode silently skips the rule for the admin — no "bypass
branch protections" checkbox.

### Classic branch protection

- **Required reviews**: none (the ruleset is the merge restriction)
- **Required status checks**: `test (linux)`, `test (macos)`, `test (windows)`
- **Enforce admins**: off

**Why not CODEOWNERS?** Deadlock for solo maintainers: the code owner can't
approve their own PRs. The "Restrict updates" ruleset is simpler: one rule, one
bypass actor, CI remains enforced for everyone.

**Why not "Restrict who can push"?** Only available for org-owned repos. This
is a personal repo (`max-sixty/worktrunk`).

## Environment protection

`CARGO_REGISTRY_TOKEN` and `AUR_SSH_PRIVATE_KEY` are in a protected GitHub
Environment (`release`) requiring deployment approval from `@max-sixty`. The
environment has a deployment branch policy restricting to `v*` tags.

## Triage ↔ mention handoff

New issues are always handled by `tend-triage` — `tend-mention` only
triggers on issue **edits** (not opens) to avoid two workflows racing to create
fix PRs for the same bug.

- **New issue** (opened) → triage, regardless of `@worktrunk-bot` mentions
- **Issue edited** to add `@worktrunk-bot` → mention
- **Comment** on an issue/PR → mention (via `issue_comment` trigger)

The mention workflow runs for any user who includes `@worktrunk-bot` — the merge
restriction is the safety boundary, not access control on the workflow.

## Bot-engaged auto-response

**Triggers a response:**
- Non-draft PR opened or updated → automatic code review (`tend-review`)
- Formal review submitted on a `worktrunk-bot`-authored PR, with body or non-approval → `tend-review` responds
- `@worktrunk-bot` mentioned in a new issue body → `tend-triage` handles it
- `@worktrunk-bot` mentioned via issue edit → `tend-mention` responds
- `@worktrunk-bot` mentioned in any comment → `tend-mention` responds
- Comment on a PR/issue where `worktrunk-bot` has engaged → `tend-mention` runs, responds only if helpful
- Editing a comment or issue body re-triggers the mention workflow

**Does not trigger:**
- `worktrunk-bot`'s own comments (workflow-level loop prevention)
- Empty approvals on `worktrunk-bot` PRs
- Comments on issues/PRs where `worktrunk-bot` hasn't engaged and no mention
- Draft PRs

**Loop prevention for bot reviews:** Bot-authored reviews and review comments
are no longer filtered at the workflow level (removed in tend#168 to allow the
bot to apply its own review suggestions). Instead, the prompt includes
self-conversation loop prevention: the bot exits silently unless there is a
distinct role boundary (e.g., reviewer on its own PR).

**Routing:** Formal reviews (`pull_request_review`) → `tend-review`. Inline
comments (`pull_request_review_comment`) and conversation comments
(`issue_comment`) → `tend-mention`.
