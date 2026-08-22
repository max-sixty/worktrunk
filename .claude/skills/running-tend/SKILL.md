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

`task` and `cargo-llvm-cov` are not installed in the `tend-setup` action.
Don't try to `cargo install` them in the sandbox — past attempts at
source-compiling installs cascaded into bash-tool interrupts that blocked
even `pwd` and `echo`. (Pre-built single-script installers like Determinate
Nix's are fine — see **Weekly Maintenance: MSRV & Toolchain** for the one we
use. The block is specifically about long-running cargo compiles.) Instead,
query Codecov directly, following `tests/CLAUDE.md` → **Coverage
Investigation** for the endpoints and their traps.

If the Codecov API markers aren't enough, download the `code-coverage-report`
artifact from the PR head's `coverage` workflow run — it contains a
`cobertura.xml` with per-line hit counts:

```bash
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
# Find the coverage run on the PR head SHA:
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
- **Flaky test** (known-flaky or first-seen PTY/shell test) — try to fix it.
- **Real regression** — proceed with a fix PR.

**Non-required ≠ transient.** A non-required job (e.g. `collect affected coverage`, `affected tests (linux, advisory)`) can fail from a real regression. The required/non-required distinction is about merge-blocking, not about how the failure is classified. If a deterministic build error (`error[E...]`, "binary not found", "ambiguous candidates", missing target) repeats across consecutive runs of the same shape, it's a real regression even when the job is advisory. Reserve "transient" for non-deterministic causes: `BrokenPipe`, `connection reset`, runner disk full, GitHub API timeouts, host-availability blips.

**Lychee link-check timeouts are always transient** unless the same URL has
failed on at least two separate runs within the last few days. The check runs
as the `link-check` job in the `nightly` workflow. `.config/lychee.toml`
already sets `max_retries = 6` and lists known-unreliable hosts; one timeout
is not enough evidence to extend that list. Signals you have a transient
failure, not a broken link:

- The previous run on the same or a nearby commit passed.
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
writes a diagnostic bundle — a single report containing wt/git/OS versions,
shell integration, `wt config show`, `git worktree list --porcelain`, and a
`trace.log` of every git invocation with its output. The `-vv` output prints
the bundle's exact absolute path (`Diagnostics and performance profile
saved @ …`) followed by a ready-to-run `gh gist create --web
<path>` line — **point the user at those printed lines; don't hand them a
hardcoded path**. In particular, never tell them to `cat
.git/wt/logs/diagnostic.md`: inside a linked worktree `.git` is a gitdir
*file*, not a directory, so that path fails with `Not a directory (os error
20)` (`ENOTDIR`). The bundle actually lives under the git *common* dir —
`"$(git rev-parse --git-common-dir)/wt/logs/diagnostic.md"` resolves from any
worktree, and the printed absolute path already points there. One gist URL
pasted into the issue gives us most of what we'd otherwise ask for piecemeal,
so lead with this for unexplained failures rather than chaining
version/config/repro questions across multiple round-trips.

When the report is about a slow `wt` command, read its **Performance profile**
section first. It renders the same breakdown as `wt config state logs profile`
(subprocess time by command type, slowest calls, repeated `(command, context)`
pairs) directly from the bundled `trace.log`, so you can spot redundant git
calls and slow commands without parsing the raw trace by hand. The same
report run against a statusline capture is the weekly per-render cache
check — see **Weekly Maintenance: Statusline Cache-Check**.

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

### Check `--config-set` before calling a setting unpinnable

`wt --config-set '<toml>'` overrides any user config key for one invocation,
above both config files and `WORKTRUNK_SECTION__KEY` env vars ([inline config
overrides](https://worktrunk.dev/config/#inline-config-overrides-config-set)).
The resolver just reads `repo.config().<key>` and shows no sign of that layer,
so before writing "there's no way to do X per invocation", ask whether X is a
config key: the ask is usually already met, and the reply is a `--config-set`
recipe plus a docs fix where the docs don't connect the flag to the setting.
Pinning is all that covers — weigh the request's residual asks on their own
merits.

### Suggesting Aliases for Niche Feature Requests

worktrunk deliberately limits flag and config growth, so a `wt` alias is the
standing answer to a narrow feature request rather than a new native flag.
Suggest one when the request serves a single reporter's workflow or a small
subset of users (idempotent create-or-switch, auto-push after merge) and
composes from existing `wt` commands.

Answer with a `wt` `[aliases]` entry. Defined in user config, it resolves
`wt <name>` to the alias whenever no built-in matches, so the same alias works
across every repo. It's the project's preferred extension point.

**How to respond:**
1. Search open and closed issues for the same request and link the prior
   thread; these asks recur, and the link lets the deflection read as a
   considered position rather than a brush-off.
2. Draft the alias.
3. Test it in a scratch worktree against the happy path and edge cases (branch
   already exists, dirty worktree). When a surprising behavior turns up, note
   it in the reply instead of building the alias around it. For example, a
   create-or-switch wrapper inherits `wt switch <name>`'s habit of
   materializing a *remote* branch of the same name.
4. Post the tested alias with usage examples.
5. Link to the [aliases docs](https://worktrunk.dev/extending/#aliases) and
   [tips & patterns](https://worktrunk.dev/tips-patterns/).

### Weigh the root-cause fix before shipping a config/docs workaround

When a mismatch or false-positive report has an obvious configurable
workaround (a template change, a config value, an alias), don't stop at
documenting it. First check whether the workaround is **lossy or
foot-gunny**, and weigh a proportionate **root-cause code fix** before
opening a docs-only PR. A "docs-only, no risk" framing is not the same as
good guidance — a zero-code-risk change can still steer users toward a
collision-prone or lossy config. If you do recommend a config change,
surface its downsides in the PR body up front, not only when challenged.

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

- **`baptiste0928/cargo-install@v3` blocks** in `.github/workflows/{affected,ci,coverage,nightly}.yaml` and `.github/actions/{test,tend}-setup/action.yaml` — every `version: "=X.Y.Z"` against `cargo info <crate>`. Today: `cargo-affected`, `cargo-insta`, `cargo-nextest`, `cargo-llvm-cov`, `cargo-msrv`, `cargo-udeps`, `lychee`, `worktrunk`. `cargo-affected` is pinned twice in `affected.yaml`; move both together. Verify each crate's `rust-version` against the pinned toolchain and note compatibility in the PR body (see PR #1657 for the format).
- **`hustcer/setup-nu@v3`** `version:` input — latest from `gh api repos/nushell/nushell/releases/latest --jq '.tag_name'`. Four call sites: `coverage.yaml` (`code-coverage`), `nightly.yaml` (`feature-powerset`), `benchmarks.yaml` (`benchmarks`), and `actions/test-setup/action.yaml`.
- **Codex Cloud tools** — `dev/codex.sh` pins pre-commit, cargo-insta, cargo-nextest, Nushell, and PowerShell; `setup-web` pins Nushell and PowerShell. Keep cargo-insta, cargo-nextest, and Nushell level with `.github/actions/test-setup/action.yaml`, which pins the same three — the gate runs `--all-features`, so Nushell's version moves PTY snapshots. Nothing under `.github/` pins PowerShell (CI runs whatever the runner image ships), so bump that one on its own.
- **Docs site packages** in `docs/package.json` are covered by Dependabot's
  `/docs` npm entry. Do not duplicate those bumps in this manual pin pass.
- **Runner images** — `ubuntu-24.04`, `macos-15`, `windows-2022`. Keep `windows-2022` pinned (actions/runner-images#12677 — windows-2025 lacks the D: drive).

Discovery shortcut: a recent green CI run on `main` flags cargo-install drift directly via workflow annotations. `gh run view <run-id> --json jobs --jq '.jobs[].databaseId' | xargs -I{} gh api repos/<owner>/<repo>/check-runs/{}/annotations` returns one warning per outdated pin.

## Weekly Maintenance: Statusline Cache-Check

Detect new in-process cache-miss duplicates introduced by recent changes by
profiling a real `wt list statusline --format=claude-code` trace. The render
runs on every Claude Code prompt redraw, so duplicate git subprocesses there
compound into measurable fseventsd / IPC load.

```bash
# Run from any worktree of this repo. `jq -n` builds the stdin JSON so the
# recipe is portable (the weekly job runs on ubuntu-24.04, whose GNU sed
# rejects BSD's `sed -i ''`) and so a path with a quote can't corrupt it.
jq -n --arg cwd "$PWD" '{
  hook_event_name: "Status",
  workspace: {current_dir: $cwd},
  model: {display_name: "Opus"},
  context_window: {used_percentage: 42.0}
}' > /tmp/statusline-input.json

# Debug build on purpose. `tend-weekly` installs no `wt`, and its rust-cache
# step is `save-if: false` under a key no workflow writes, so `--release`
# means a cold optimized build of the whole dependency graph before the first
# render. What this check reads — duplicate `(command, context)` pairs and the
# subprocess count — is profile-independent; only the timing columns, which
# this section doesn't triage, would be worth a release build.
cargo run -- -vv list statusline --format=claude-code \
  < /tmp/statusline-input.json > /dev/null
cargo run -- config state logs profile --format=json | jq .cache
```

The `.cache` report flags commands invoked more than once with the same context.
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

**On drift, open a PR — don't file an issue.** The source of truth is `after_long_help` in `src/cli/mod.rs`; edit it and let `cargo test --test integration test_docs_are_in_sync` regenerate the mirrors under `docs/src/content/docs/` and `skills/worktrunk/reference/`. The "smallest current variant" call is a judgment — pick the one the vendor's models page currently positions as fastest/smallest, and explain the choice in the PR body. Verifying the new model name with an installed CLI (`codex -m <name>`, etc.) isn't possible in this CI sandbox; the PR is the right output anyway, and the maintainer tests on merge.

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

- **New statusline JSON fields** — `src/commands/statusline.rs` parses `workspace.current_dir`, `model.display_name`, `context_window.used_percentage`, and `rate_limits.{five_hour,seven_day}.{used_percentage,resets_at}`. A newly added field (session cost, PR review state) may be worth surfacing in `wt list statusline`.
- **Renamed or removed hook events** — `WorktreeCreate`/`WorktreeRemove` route agent worktree creation through `wt`; a renamed event silently disables isolation rather than erroring.
- **Changed plugin install mechanisms** — `wt config plugins {claude,codex,opencode} install` and the Gemini extension manifest break if the marketplace or plugins-directory contract changes.

Don't open a PR speculatively. File one issue per relevant change, linking the upstream entry and noting what Worktrunk would need to do. If nothing changed, say so and move on.

## README Date Check

The README blockquote opens with a month+year (e.g., "**April 2026**"). During daily
maintenance, verify the month matches the current month and update it if stale.

## Per-Workflow References

- **PR review**: `@references/review-pr.md` — Rust idioms, documentation accuracy, duplication search
- **Nightly sweep**: `@references/nightly-cleaner.md` — survey checklist, branch naming, CI-breakage ownership
