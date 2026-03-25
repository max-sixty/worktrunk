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

### Feature requests addressable with aliases

When triaging a feature request, check whether the requested behavior can be
composed from existing commands using a `wt step` alias or a shell
function/alias. Worktrunk intentionally keeps its flag surface small — aliases
let users test workflows before features are added natively.

If the feature can be addressed with an alias:

1. **Draft the alias** — Write a `wt step` alias (in `[aliases]` config) or
   shell function that achieves the requested behavior.
2. **Test it** — Set up a temporary repo, configure the alias, and verify it
   works for both the happy path and edge cases (e.g., branch exists vs.
   doesn't exist).
3. **Post the alias in your comment** — Include both the config snippet and
   usage example. Link to the [aliases documentation](https://worktrunk.dev/step/#aliases)
   so the user can learn more.
4. **Mention the shell alternative** — A shell function is often more ergonomic
   for commands that need positional arguments (aliases use `--var name=value`).

Example comment structure:

> This can be done today with a `wt step` alias. Add to `~/.config/worktrunk/config.toml`:
>
> ```toml
> [aliases]
> sw = "wt switch -c {{ name }} 2>/dev/null || wt switch {{ name }}"
> ```
>
> Then: `wt step sw --var name=my-branch`
>
> Or as a shell function: `wts() { wt switch -c "$1" 2>/dev/null || wt switch "$1"; }`
>
> See [Aliases](https://worktrunk.dev/step/#aliases) for more details.

## Per-Workflow References

- **PR review**: `@references/review-pr.md` — Rust idioms, documentation accuracy, duplication search
- **Nightly sweep**: `@references/nightly-cleaner.md` — survey script, branch naming
