# CI Automation — Worktrunk

See [tend's security model](https://github.com/max-sixty/tend/blob/main/docs/security-model.md)
for the generic security model. This file documents worktrunk-specific
configuration.

## Bot identity

`worktrunk-bot` — a regular GitHub user account (PAT-based, not a GitHub App).
Workflows check `user.login == 'worktrunk-bot'` directly.

## Tokens

| Token | Purpose |
|-------|---------|
| `WORKTRUNK_BOT_TOKEN` | All Claude workflows — consistent bot identity |
| `CLAUDE_CODE_OAUTH_TOKEN` | Authenticates Claude Code to the Anthropic API |

## Merge restriction

Only the repo owner (`@max-sixty`, admin) can merge to `main`.
`worktrunk-bot` has `write` role only. Enforced by a "Merge access" ruleset
(restrict updates, admin bypass in exempt mode). Required status checks:
`test (linux)`, `test (macos)`, `test (windows)`.

## Environment protection

`CARGO_REGISTRY_TOKEN` and `AUR_SSH_PRIVATE_KEY` are in a protected GitHub
Environment (`release`) requiring deployment approval from `@max-sixty`,
restricted to `v*` tags.

## Build environment

All workflows must set `CARGO_INCREMENTAL=0`. `Swatinem/rust-cache` hashes
`CARGO*` env vars into the cache key (via the `env-vars` input, default prefix
`CARGO`). If one workflow sets `CARGO_INCREMENTAL=0` and another doesn't, they
get different cache keys and won't share cached artifacts. CI does clean builds
anyway, so incremental state is pure overhead.
