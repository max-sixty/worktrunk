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

## Applying GitHub Suggestions

Apply the literal suggestion only — change the lines it covers, nothing more.
If surrounding lines also need updating, note that in your reply.

## Issue Triage

When a bug may already be fixed, ask the reporter: `wt --version`

### Suggesting Aliases for Feature Requests

When a feature request can be addressed with a shell alias or `wt step` alias,
compose and test one before responding. This lets users try workflows
immediately rather than waiting for a native flag.

**When to suggest an alias:**
- The request is for a behavioral variant of an existing command (e.g.,
  idempotent create-or-switch, auto-push after merge)
- The behavior can be composed from existing `wt` commands
- A shell one-liner or `wt step` alias covers the use case

**How to respond:**
1. Draft the alias (shell function or `wt step` alias, whichever fits better)
2. Test it in a scratch repo — verify it works for both the happy path and the
   fallback case
3. Post the tested alias in the issue with usage examples
4. Link to the [aliases docs](https://worktrunk.dev/step/#aliases) for context

## Per-Workflow References

- **PR review**: `@references/review-pr.md` — Rust idioms, documentation accuracy, duplication search
- **Nightly sweep**: `@references/nightly-cleaner.md` — survey script, branch naming
