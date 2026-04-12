---
name: running-tend
description: Worktrunk-specific guidance for tend CI workflows. Adds codecov polling, Rust test commands, labels, and review criteria on top of the generic tend-* skills. Use when operating in CI.
metadata:
  internal: true
---

# Worktrunk Tend CI

Project-specific guidance for tend workflows running on worktrunk (a Rust
CLI for managing git worktrees). The generic skills (`tend-running-in-ci`,
`tend-review`, `tend-triage`, etc.) provide the workflow framework;
this skill adds worktrunk conventions.

## Codecov Monitoring

After required CI checks pass, poll `codecov/patch` — it is mandatory despite
being marked non-required:

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
```

If codecov fails, investigate with `task coverage` and
`cargo llvm-cov report --show-missing-lines | grep <file>`.

## Test Commands

```bash
cargo run -- hook pre-merge --yes   # full suite + lints
cargo test --lib --bins             # unit tests only
cargo test --test integration       # integration tests only
```

CI runs on Linux, Windows, and macOS.

## Session Log Paths

Artifact paths: `-home-runner-work-worktrunk-worktrunk/<session-id>.jsonl`

## Labels

- `automated-fix` — fix PRs from triage and ci-fix workflows
- `nightly-cleanup` — nightly sweep issues and PRs

## CI Fix: Prefer Rerun for Transient Infrastructure Failures

Before opening a `fix/ci-*` PR, classify the failure:

- **Transient infrastructure** (link-check timeouts, apt-get flakes, GitHub
  outages, runner disk issues, codecov upload blips) — do **not** create a
  PR. The maintainer will rerun CI. Comment on the run or exit silently; a
  permanent config change for a one-off timeout is churn the maintainer will
  close.
- **Flaky test** (known-flaky or first-seen PTY/shell test) — exit without a
  PR (same behavior as prior test-flake ci-fix runs).
- **Real regression** — proceed with a fix PR.

**Lychee link-check timeouts are always transient** unless the same URL has
failed on at least two separate runs within the last few days. `.config/lychee.toml`
already sets `max_retries = 6` and lists known-unreliable hosts; one timeout
is not enough evidence to extend that list. Signals you have a transient
failure, not a broken link:

- The previous CI run on the same or a nearby commit passed.
- Only `[TIMEOUT]` is reported (not `404`/`403`/`410`).
- The URL is reachable from a local `curl`.

When in doubt, post a comment on the failed run summarizing the diagnosis and
wait — don't open a PR.

## Applying GitHub Suggestions

Apply the literal suggestion only — change the lines it covers, nothing more.
If surrounding lines also need updating, note that in your reply.

## Issue Triage

When a bug may already be fixed, ask the reporter: `wt --version`

When an issue involves config, shell integration, completions, or unexpected
behavior that could stem from user setup, ask the reporter for
`wt config show` output. This reveals installed shells, config paths, and
active settings — essential context for diagnosing config-related problems.

### Closing Duplicates

When an issue is clearly a duplicate, close it after commenting. Use
`gh issue close <number>` and tell the reporter: if they believe this was
closed in error, they can let us know and we'll reopen it.

### Suggesting Aliases for Niche Feature Requests

Deflect narrow feature requests to aliases rather than native flags — this
keeps the CLI surface small while giving users the behavior immediately.
Suggest an alias when:

- The request benefits a small subset of users or a single reporter's workflow
  (e.g., idempotent create-or-switch, auto-push after merge)
- The behavior can be composed from existing `wt` commands or shell primitives
- A shell one-liner or `wt step` alias covers the use case

**How to respond:**
1. Draft the alias (shell function or `wt step` alias, whichever fits better)
2. Test it in a scratch worktree — verify it works for the happy path and edge
   cases (e.g., branch already exists, dirty worktree, missing remote)
3. Post the tested alias in the issue with usage examples
4. Link to the [aliases docs](https://worktrunk.dev/step/#aliases) and
   [tips & patterns](https://worktrunk.dev/tips-patterns/) for further recipes

## Weekly Maintenance: MSRV & Toolchain

Bump both MSRV and the development toolchain to **latest stable − 1**. When
Rust 1.N is the current stable release, set both to 1.(N−1).

Files to update:

| File | Field | Example (if stable is 1.94) |
|------|-------|----|
| `Cargo.toml` | `rust-version` | `"1.93"` |
| `tests/helpers/wt-perf/Cargo.toml` | `rust-version` | `"1.93"` |
| `rust-toolchain.toml` | `channel` | `"1.93.0"` |
| `flake.nix` | MSRV comment | `1.93` |

After bumping, run the full test suite (`cargo run -- hook pre-merge --yes`)
and verify `cargo msrv verify` passes.

## README Date Check

The README blockquote opens with a month+year (e.g., "**April 2026**"). During daily
maintenance, verify the month matches the current month and update it if stale.

## Per-Workflow References

- **PR review**: `@references/review-pr.md` — Rust idioms, documentation accuracy, duplication search
- **Nightly sweep**: `@references/nightly-cleaner.md` — branch naming
