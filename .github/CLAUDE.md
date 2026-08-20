# CI Automation — Worktrunk

See [tend's security model](https://github.com/max-sixty/tend/blob/main/docs/security-model.md)
for the generic security model. This file documents worktrunk-specific
configuration.

## Bot identity

`worktrunk-bot` — a regular GitHub user account (PAT-based, not a GitHub App).
Workflows check `user.login == 'worktrunk-bot'` directly.

## Tokens

| Token | Purpose | Stored in |
|-------|---------|-----------|
| `TEND_BOT_TOKEN` | Every workflow acting as `worktrunk-bot`, including the winget and Homebrew publish jobs | `tend` and `release` environments |
| `CLAUDE_CODE_OAUTH_TOKEN` | Authenticates Claude Code to the Anthropic API | `tend` environment |
| `CODECOV_TOKEN` | Uploads coverage from `ci.yaml` and `coverage.yaml` | repo level, allowlisted in `.config/tend.yaml` |

## Merge restriction

Only the repo owner (`@max-sixty`, admin) can merge to `main`.
`worktrunk-bot` has `write` role only. Enforced by a "Merge access" ruleset
(restrict updates, admin bypass in exempt mode). Required status checks:
`test (linux)`, `test (macos)`, `test (windows)`, `fast-checks`.

## Environment protection

Every secret except `CODECOV_TOKEN` lives in a GitHub Environment, where no
other workflow the repo runs can read it. A coverage upload grants nothing
worth gating. Each environment holds what one phase needs, since a job that
joins an environment can read every secret in it:

| Environment | Admits | Secret | Read by |
|-------------|--------|--------|---------|
| `tend` | `main` | `CLAUDE_CODE_OAUTH_TOKEN`, `TEND_BOT_TOKEN` | every `tend-*.yaml` job except `relay`; `append-gist`; both `create-issue-on-*-failure` jobs |
| `release` | any tag | `AUR_SSH_PRIVATE_KEY`, `TEND_BOT_TOKEN` | `publish-aur`, `publish-winget`, `publish-homebrew` |
| `signing` | any tag | `SIGNPATH_API_TOKEN` | `build-local-artifacts` |
| `github-pages` | `main` | none — OIDC only | `deploy-docs` |

The deployment branch policy is the gate: a job naming an environment runs
only from a ref the policy admits, so a workflow pushed to a feature branch is
refused before its first step. `worktrunk-bot` can push branches but cannot
update `main` (the "Merge access" ruleset) or move tags (the "Tag operations"
ruleset, admin-only), which is what makes both admitted sets unreachable to
it. No environment has a reviewer rule, so joining a job to one costs no
approval step.

The tag policies admit every tag rather than a `v*` pattern. "Tag operations"
covers `~ALL` tags, so the pattern carries no part of the gate — it only
duplicates `release.yaml`'s own tag filter, and the two are easy to drift
apart. They already had: the workflow fires on `**[0-9]+.[0-9]+.[0-9]+*`,
which matches an unprefixed `1.2.3` that a `v*` policy would then refuse. A
release cut under that name would have stopped at `build-local-artifacts`,
which names `signing` and waits only on `plan` — before an artifact was built,
let alone published.

`TEND_BOT_TOKEN` is stored in two environments rather than shared, because a
policy admits branches or tags but the token is needed under both: the bot's
own workflows run on `main`, the publish jobs on a tag. Adding the tag to
`tend` is not an option — `tend check` pins that policy to exactly the
protected branches and its `--fix` deletes anything else.

Jobs name `tend` as `{name: tend, deployment: false}`. GitHub files a
deployment record for every job that names an environment, against whatever
ref the run belongs to — under `pull_request_target` that is the pull request
itself, so an omission posts a "worktrunk-bot deployed to tend" line on every
push to every PR. `deployment: false` drops the record and keeps the gate. The
release jobs keep their records: a tag-push deployment lands in no PR
timeline, and it reads as what it is.

The generated `tend-*.yaml` files carry the same `{name: tend, deployment:
false}`, written by tend's generator rather than edited here — every `uvx
tend@latest init` overwrites them, so a hand edit would not survive one. tend
0.1.14 added the field and #3749 landed the regen, which is what cleared `tend
check`'s `environment-deployments`.

crates.io publishing holds no stored token — it uses Trusted Publishing.
crates.io mints a short-lived one only for an OIDC claim from `release.yaml`
running in that same `release` environment.

## Build environment

`Swatinem/rust-cache` hashes `CARGO*` and `RUST*` env vars into the cache key.
All workflows sharing a cache must set the same env vars, or they'll get
different keys and miss each other's caches.

It hashes the vars **visible at its own step**, so a var exported by a later
step is invisible and a var the writers don't set poisons the key. ci.yaml and
nightly.yaml carry theirs in a workflow-level `env:` block, always in place
first; the generated `tend-*.yaml` files can't, so `tend-setup` sets the same
three vars in a step above its cache step. A miss is silent — the step
succeeds having restored nothing — so drift here shows up only as slow jobs.
