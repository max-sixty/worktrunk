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

## Filing issues in other repos

Standing exception granted: file directly in agent-equipped targets (per
**Filing Issues in Other Repos** in the bundled `running-in-ci` skill) without
asking permission here first. The default rule (open an issue here asking
permission first) still applies when the target shows no agent signals.

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

If codecov fails **locally**, investigate with `task coverage` and
`cargo llvm-cov report --show-missing-lines | grep <file>`.

### Investigating codecov failures in CI

`task` and `cargo-llvm-cov` are not installed in the `claude-setup` action.
Don't try to `cargo install` them in the sandbox — past attempts at
source-compiling installs cascaded into bash-tool interrupts that blocked
even `pwd` and `echo`. (Pre-built single-script installers like Determinate
Nix's are fine — see **Weekly Maintenance: MSRV & Toolchain** for the one we
use. The block is specifically about long-running cargo compiles.) Instead,
query Codecov directly:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
curl -sL "https://api.codecov.io/api/v2/gh/${REPO%/*}/repos/${REPO#*/}/compare/?pullid=<N>" > /tmp/codecov.json

# Patch-level summary per file:
jq '.files[] | {name: .name.head, patch: .totals.patch}' /tmp/codecov.json

# Uncovered added lines in a specific changed file
# (coverage.head is a LineType enum: 0=hit, 1=miss, 2=partial — filter on 1=miss):
jq '.files[] | select(.name.head == "<path>") | .lines[] | select(.is_diff and .added and .coverage.head == 1) | {line: .number.head, code: .value}' /tmp/codecov.json
```

If the Codecov API markers aren't enough, download the `code-coverage-report`
artifact from the PR head's `ci` workflow run — it contains a `cobertura.xml`
with per-line hit counts:

```bash
# Find the ci run on the PR head SHA:
CI_RUN=$(gh api "repos/$REPO/commits/<sha>/check-runs" --jq '.check_runs[] | select(.name == "code-coverage") | .details_url | capture("runs/(?<id>[0-9]+)") | .id')
# List artifacts, then download the coverage one:
gh api "repos/$REPO/actions/runs/$CI_RUN/artifacts" --jq '.artifacts[] | {name, id}'
gh api "repos/$REPO/actions/artifacts/<id>/zip" > /tmp/coverage.zip
unzip -q /tmp/coverage.zip -d /tmp/coverage
```

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

**Non-required ≠ transient.** A non-required job (e.g. `collect affected coverage`, `affected tests (linux, advisory)`) can fail from a real regression. The required/non-required distinction is about merge-blocking, not about how the failure is classified. If a deterministic build error (`error[E...]`, "binary not found", "ambiguous candidates", missing target) repeats across consecutive runs of the same shape, it's a real regression even when the job is advisory. Reserve "transient" for non-deterministic causes: `BrokenPipe`, `connection reset`, runner disk full, GitHub API timeouts, host-availability blips.

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

## PR Review: Don't Self-Dismiss Over Unrelated Test Flakes

If a clearly-unrelated test fails after you've already approved a PR, leave
the approval in place and post a comment noting the flake. Do **not** dismiss
your own approval to "gate" on a rerun.

GitHub blocks both `gh run rerun --failed` and per-job rerun
(`POST /repos/{owner}/{repo}/actions/jobs/{id}/rerun`) with HTTP 403 while
*any* job in the same workflow run is still `in_progress`. The non-required
`benchmarks` job routinely runs 80+ minutes after `test (linux|macos|windows)`
finish, so dismiss-then-wait-then-rerun cascades into a long session for no
benefit — the maintainer can rerun the failed job directly once `benchmarks`
clears, or merge regardless if the failure is clearly a flake.

The codecov-failure dismissal pattern is different and remains correct:
`CLAUDE.md` requires explicit user approval before merging with failing
`codecov/patch`, so dismissing the approval until the coverage gap is
addressed is intentional.

## Issue Triage

When you need more information to diagnose a reported bug, the **primary
ask is `wt -vv <command>`**. Re-running the failing command with `-vv`
writes `.git/wt/logs/diagnostic.md` — a single report containing wt/git/OS
versions, shell integration, `wt config show`, `git worktree list
--porcelain`, and a `trace.log` of every git invocation with its output —
and prints a `gh gist create --web <path>` hint. One gist URL pasted into
the issue gives us most of what we'd otherwise ask for piecemeal, so lead
with this for unexplained failures rather than chaining version/config/repro
questions across multiple round-trips.

Reach for narrower asks only when the diagnostic is overkill:

- `wt --version` — when the only question is whether a fix has landed.
- `wt config show` — when the suspicion is purely config/shell-integration
  and you already have the command + repro.

### Don't ship fixes you can't verify

When the bug or proposed fix turns on runtime state the bot can't observe from CI — plugin hooks firing inside an agent CLI (Claude Code, Codex, Gemini), shell-integration side effects, interactive prompt rendering, signal forwarding into a TTY — do **not** open a PR premised on the hypothesis. Signals to stop:

- The proposed transition fires inside a running agent session the bot can't drive from a test (`PostToolUse`, `Stop`, `Notification`, statusline redraws).
- The "analysis" in the issue is an LLM-written trace pasted by the reporter, not a verified observation. Treat that as a starting hypothesis, not ground truth — a Claude-written explanation of why X is broken is no more trustworthy than the bot's own first guess.
- The repro requires an interactive shell or `claude` running in a tmux that the bot can't spin up.

Comment on the issue with what's known, ask the reporter for the concrete symptom they observe ("which marker shows where, when") rather than for a fix to validate, and exit without a PR. The bar for opening a fix PR is *the failure mode is reproducible and the fix is testable*, not *the hypothesis seems plausible*. If you post a fix despite limited testability (rare — usually only when the reporter has confirmed the exact symptom and the code change is obviously correct from inspection), explicitly flag what wasn't verified in the PR body.

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

### Don't fix tests by adding skip guards

When a test fails because production code or test setup can't handle some
scenario, fix the production code or rework the test setup. Don't add an
early-return skip — that removes the safety net while looking like a fix.
If a triage fix reaches for `let Ok(_) = ... else { return };`, a newly-added
`if !path.exists() { return; }`, or a fresh `#[ignore]`, stop and ask what
production behavior is actually broken.

If the test relies on inherited environment (process CWD, ambient env
vars), rework it to set up its own — most worktrunk tests already do this
via `TestRepo::with_initial_commit()` plus a tempdir.

### Same-root-cause-class triage

The "work on the existing PR if it addresses the same problem" rule keys
on the same test. It doesn't catch a different test failing for the same
underlying reason. Group failing tests by root-cause class before writing
a fix; if an outstanding PR addresses any test in the class, wait for it
to merge and re-run, then mirror its approach for any sites still failing
rather than opening a parallel PR with a weaker fix.

## Weekly Maintenance: MSRV & Toolchain

Bump both MSRV and the development toolchain to **latest stable − 1**. When
Rust 1.N is the current stable release, set both to 1.(N−1).

Files to update:

| File | Field | Example (if stable is 1.94) |
|------|-------|----|
| `Cargo.toml` | `rust-version` | `"1.93"` |
| `tests/helpers/wt-perf/Cargo.toml` | `rust-version` | `"1.93"` |
| `rust-toolchain.toml` | `channel` | `"1.93.0"` |

`flake.nix` reads the channel from `rust-toolchain.toml`, so no separate bump
is needed. After updating the toolchain, refresh `flake.lock` so the locked
`rust-overlay` revision knows about the new version. Nix isn't installed in
the tend sandbox by default — install it with the Determinate Systems
installer (single script, daemon-mode, no prompts), then update:

```bash
curl -fsSL https://install.determinate.systems/nix -o /tmp/nix-installer.sh
sh /tmp/nix-installer.sh install --no-confirm --determinate
. /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
nix flake update --extra-experimental-features 'nix-command flakes'
```

Verify the new lock evaluates with the channel bump before committing:

```bash
nix eval --extra-experimental-features 'nix-command flakes' \
  .#devShells.x86_64-linux.default.name
```

Commit `flake.lock` alongside the other toolchain changes. After bumping, run
the full test suite (`cargo run -- hook pre-merge --yes`) and verify
`cargo msrv verify` passes.

## Weekly Maintenance: CI Pin Bumps

Pinned third-party versions in CI are invisible to Dependabot — it follows `Cargo.toml` deps and `uses: foo@vN` action refs, not inline `version:` strings. They drift unless this step bumps them.

For each weekly run, check upstream and bump:

- **`baptiste0928/cargo-install@v3` blocks** in `.github/workflows/ci.yaml`, `.github/workflows/nightly.yaml`, and `.github/actions/{test,claude}-setup/action.yaml` — every `version: "=X.Y.Z"` against `cargo info <crate>`. Today: `cargo-insta`, `cargo-nextest`, `cargo-llvm-cov`, `cargo-msrv`, `cargo-udeps`, `lychee`, `worktrunk`. The `cargo-affected` install has no version pin (follows default branch) — leave it alone. Verify each crate's `rust-version` against the pinned toolchain and note compatibility in the PR body (see PR #1657 for the format).
- **`hustcer/setup-nu@v3`** `version:` input — latest from `gh api repos/nushell/nushell/releases/latest --jq '.tag_name'`. Three call sites: `ci.yaml` (`code-coverage`), `nightly.yaml` (`benchmarks`), and `actions/test-setup/action.yaml`.
- **`taiki-e/install-action@v2.x`** `tool: zola@<ver>` in the `check-docs` job — latest from `gh api repos/getzola/zola/releases/latest --jq '.tag_name'`.
- **Runner images** — `ubuntu-24.04`, `macos-15`, `windows-2022`. Keep `windows-2022` pinned (actions/runner-images#12677 — windows-2025 lacks the D: drive).

Discovery shortcut: a recent green CI run on `main` flags cargo-install drift directly via workflow annotations. `gh run view <run-id> --json jobs --jq '.jobs[].databaseId' | xargs -I{} gh api repos/<owner>/<repo>/check-runs/{}/annotations` returns one warning per outdated pin.

## Weekly Maintenance: Statusline Cache-Check

Detect new in-process cache-miss duplicates introduced by recent changes by
running `wt-perf cache-check` against a real `wt list statusline --claude-code`
trace. The render runs on every Claude Code prompt redraw, so duplicate git
subprocesses there compound into measurable fseventsd / IPC load.

```bash
# Run from any worktree of this repo
cat > /tmp/statusline-input.json <<'EOF'
{"hook_event_name":"Status","workspace":{"current_dir":"REPLACE_WITH_CWD"},
 "model":{"display_name":"Opus"},"context_window":{"used_percentage":42.0}}
EOF
sed -i '' "s|REPLACE_WITH_CWD|$PWD|" /tmp/statusline-input.json

RUST_LOG=debug cargo run --release -- list statusline --claude-code \
  < /tmp/statusline-input.json 2>&1 \
  | cargo run -p wt-perf -- cache-check
```

The report flags commands invoked more than once with the same context.
Triage each duplicate:

- **Legitimate** (different cwd, different ref form that can't be normalized,
  intentional double-call across phases) — note in the response and move on.
- **Cache miss** (same logical operation should hit cache but doesn't) —
  open an issue or fix it. Common shapes: `merge_base("main", "<sha>")` vs
  `merge_base("main", "branch")` keying separately;
  `worktree_at(cwd)` vs `worktree_at(porcelain_path)` not canonicalizing.

Baseline: ~29 git subprocesses per render on a clean tree; a jump above
~32 warrants investigation.

## Weekly Maintenance: LLM Model Names in Docs

Grep for current Claude and Codex pins across every tracked file:

```bash
git grep -niE "claude|codex"
```

Check the latest IDs at <https://docs.anthropic.com/en/docs/about-claude/models> and <https://developers.openai.com/codex/models>. The recommended commit-message commands should use the most recent fastest model from each vendor (Haiku for Anthropic, the smallest current Codex variant for OpenAI).

**On drift, open a PR — don't file an issue.** The source of truth is `after_long_help` in `src/cli/mod.rs`; edit it and let `cargo test --test integration test_docs_are_in_sync` regenerate the mirrors under `docs/content/` and `skills/worktrunk/reference/`. The "smallest current variant" call is a judgment — pick the one the vendor's models page currently positions as fastest/smallest, and explain the choice in the PR body. Verifying the new model name with an installed CLI (`codex -m <name>`, etc.) isn't possible in this CI sandbox; the PR is the right output anyway, and the maintainer tests on merge.

## Weekly Maintenance: Agent App Integration Surfaces

Worktrunk ships a plugin for each agent CLI it integrates with, and those CLIs
change their integration surfaces without notice. Each week, scan the upstream
changelogs and flag changes that affect what Worktrunk consumes or produces.

| App | Source to check | Integration surface |
|-----|-----------------|---------------------|
| Claude Code | `gh api repos/anthropics/claude-code/contents/CHANGELOG.md -H 'Accept: application/vnd.github.raw'`, plus `curl -sL https://code.claude.com/docs/en/statusline.md` for the statusline JSON schema | statusline stdin JSON, `WorktreeCreate`/`WorktreeRemove` hooks, plugin marketplace, `/wt-switch-create` |
| Codex | `gh release list -R openai/codex -L 10` | plugin marketplace |
| Gemini CLI | `gh release list -R google-gemini/gemini-cli -L 10` | native extension loading |
| OpenCode | `gh release list -R sst/opencode -L 10` | plugins API in `~/.config/opencode/plugins/` |

What to flag:

- **New statusline JSON fields** — `src/commands/statusline.rs` parses `workspace.current_dir`, `model.display_name`, and `context_window.used_percentage`. A newly added field (rate limits, session cost, PR review state) may be worth surfacing in `wt list statusline`.
- **Renamed or removed hook events** — `WorktreeCreate`/`WorktreeRemove` route agent worktree creation through `wt`; a renamed event silently disables isolation rather than erroring.
- **Changed plugin install mechanisms** — `wt config plugins {claude,codex,opencode} install` and the Gemini extension manifest break if the marketplace or plugins-directory contract changes.

Don't open a PR speculatively. File one issue per relevant change, linking the upstream entry and noting what Worktrunk would need to do. If nothing changed, say so and move on.

## README Date Check

The README blockquote opens with a month+year (e.g., "**April 2026**"). During daily
maintenance, verify the month matches the current month and update it if stale.

## Per-Workflow References

- **PR review**: `@references/review-pr.md` — Rust idioms, documentation accuracy, duplication search
- **Nightly sweep**: `@references/nightly-cleaner.md` — branch naming
