# Benchmark Guidelines

See `list.rs` and `time_to_first_output.rs` headers for benchmark groups and run examples.

## Quick Start

Criterion's CLI takes a positional `FILTER` (substring inclusion) and `--exact`. There's no `--skip`; pick a filter that *includes* what you want instead.

```bash
# Fast iteration (one generated group, warm cache only)
cargo bench --bench list skeleton/warm

# Run specific group (all variants)
cargo bench --bench list full

# GH #461 scenario (50 branches at varying depths in the imported corpus)
cargo bench --bench list large_repository

# All list benchmarks (~1 hour)
cargo bench --bench list

# Time-to-first-output benchmarks
cargo bench --bench time_to_first_output         # all commands
cargo bench --bench time_to_first_output remove  # just remove

# wt step prune (scan + removal on the squash-merged fixture)
cargo bench --bench prune                        # generated variants
cargo bench --bench prune --features large-repository-benches prune_large_repository  # multi-gigabyte temporary fixture

# Picker preview pre-compute (wt switch preview workload)
cargo bench --bench picker_preview               # all variants
cargo bench --bench picker_preview warm          # warm only

# Shell completion (COMPLETE=$SHELL wt -- wt switch <Tab>) — one variant, no filter
cargo bench --bench completion
```

## Fixtures and Benches

Bench groups name measurements; fixture recipes name corpus provenance. Both
bases produce ordinary Git repositories: `Generated` builds the corpus locally,
while `Imported` copies a pinned upstream corpus. Preserve the state dimensions
needed for coverage and fidelity, not historical recipes or the ability to
recreate an old setup. A property does not get its own fixture identity merely
because one benchmark studies it.
Worktree, branch, and remote-ref counts are controlled population parameters;
prune candidates are an overlay. Add another base only when the corpus itself
cannot serve one of these roles.

| Canonical base | `wt-perf setup` | Bench group(s) |
|---|---|---|
| `Generated { linked_worktrees: W, branchless_branches: B, remote_tracking_refs: R }` | `setup generated W B R --path <path>` | all generated benchmarks |
| `Imported` | `setup imported --path <path>` | `large_repository` and `prune_large_repository` |

`linked_worktrees` excludes the primary worktree. `branchless_branches` have
no linked worktree. `remote_tracking_refs` excludes the `origin/main` and
`origin/HEAD` pair every generated fixture has. The generated defaults are 7,
50, and 0 respectively, giving ordinary benchmarks eight total worktrees plus
branches spread across its history. The imported base always has the same
eight total worktrees and fifty history-spread branches.

Add prune state with `--prune-candidates M --prune-backdrop U`. Each count is
a pair: one linked worktree and one branchless branch. The benchmark catalog
applies it to generated and imported fixtures. This is an overlay on the
canonical base, not a separate fixture recipe.

Every benchmark recipe returns `FixtureRepo`, the owner of the temporary root
plus the canonical primary/linked-worktree paths. Repo-bound benchmark
subprocesses start through `wt_command`, and warm/cold matrices use
`CacheState::WARM_AND_COLD`; keep lifecycle, environment isolation, and cache
labels in those shared APIs.

## Imported corpus

`benches/imported-fixture` pins the corpus and commit.
The first matching acquisition clones that revision; later runs reuse the
revision-keyed source under `target/wt-perf/bench-repos/imported/`.
Source construction is locked across processes, built in a temporary sibling,
validated, then atomically renamed into place. Mutable benchmark fixtures are
always fresh and have no shared lock or repair state.

## Faster Iteration

Criterion has no exclusion flag — narrow the run by picking a substring that matches only the variants you want. Benchmark IDs look like `<group>/<label>/<param>`, e.g. `skeleton/cold/1`, `worktree_scaling/warm/8`, `full/cold`, `large_repository/branches/warm`.

**Pattern matching (positional `FILTER`):**
```bash
cargo bench --bench list scaling             # All worktree_scaling/* variants
cargo bench --bench list warm                # Every benchmark whose ID contains "warm"
cargo bench --bench list skeleton/warm       # Just skeleton's warm variants
cargo bench --bench list full                # Both cache states of the combined fixture
cargo bench --bench list -- --exact full/cold   # One exact ID
```

To skip the slow large-repository and divergent groups, target `skeleton`, `worktree_scaling`, or `full`. Run them sequentially if you want more than one.

The `full` group is the place to start when `wt list` regresses on a real mix of worktrees and branches: the cold/warm split says whether the cost is the persistent-cache fill (cold) or the per-process re-fork (warm). A `full` wall time can't be split by side (the git subprocesses overlap on the rayon pool), so to localize a regression, trace one invocation and read the profile's BY CONTEXT table ("Analyzing a trace" below); `worktree_scaling` and `divergent_branches` track the worktree side and branch side respectively at criterion cadence.

## WORKTRUNK_FIRST_OUTPUT

Setting `WORKTRUNK_FIRST_OUTPUT=1` causes commands to exit at the point where first
user-visible output would appear. Used by `time_to_first_output` benchmarks to measure
startup latency without output rendering or post-output work (mismatch warnings, hooks).

Supported commands: `switch`, `remove`, `list`.

`wt step prune` deliberately has no `WORKTRUNK_FIRST_OUTPUT` hook: its first
output is data-dependent (the dry-run path collects and sorts every check
result before printing anything, so e2e ≈ time-to-first-output already; the
live path streams whichever check lands first). Use `benches/prune.rs` for
cadence-tracked numbers and the `prune-*` spans (below) for phase attribution.

## WORKTRUNK_PREVIEW_BENCH

Setting `WORKTRUNK_PREVIEW_BENCH=1` runs `wt switch`'s interactive picker prelude
end-to-end — collect, speculative spawn, skeleton, initial pre-compute, deferred
pre-compute — and exits immediately after `PreviewOrchestrator::wait_for_idle()`,
before skim launches and before any JSON serialization or stderr drain. Used by
`picker_preview` benchmarks to measure the skeleton-time preview pool workload
without standing up a PTY. Off-screen worktrees are demand-loaded only after skim
starts and are intentionally outside this benchmark. Bypasses the picker's TTY
check, like `WORKTRUNK_PICKER_DRY_RUN=1`.

The hot path inside the env-gated block is identical to the dry-run path; only the
post-drain output (cache JSON dump + stashed-warning drain) is conditional. Keep new
post-drain work out of the bench path unless it's part of the workload being
measured.

## Cache Handling

Worktrunk maintains a persistent SHA-keyed cache at `.git/wt/cache/` plus a git-config
cache of the default branch at `worktrunk.default-branch`. Both survive process exits,
so bench iterations read from prior iterations unless invalidated.

**Rule:** explicitly choose cache state for every benchmark that runs a `wt`
subcommand which populates these caches. Run ordinary warm/cold variants through
`wt_perf::bench_wt`, the one home of the shared strategy. Destructive variants
use `iter_custom`: each iteration builds a fresh fixture before starting the
timer, measures only the command, and checks the postcondition before returning
the accumulated duration. `remove_e2e` uses warm/cold no-hooks neighbors for
cache cost and a warm with-hooks neighbor for hook cost;
`prune_e2e/live` clears probe caches to preserve its probe-cold scenario.

`invalidate_caches_auto` clears:

- `.git/objects/info/commit-graph*`
- `.git/wt/cache/` (all sha_cache kinds + ci-status + summaries)
- `worktrunk.default-branch` (git config)

`.git/packed-refs` is deliberately preserved: fixture setup runs `git gc`
at the end, which packs every loose ref into `packed-refs`
and prunes the loose copies. Deleting that file post-gc leaves the repo with
no resolvable refs, so any bench that resolves a branch (e.g. the `with_vars`
alias's `{{ commit }}` template var) blows up partway through warm-up.

User state — `worktrunk.history`, `worktrunk.hints.*`, `worktrunk.state.<branch>.*`,
`.git/wt/logs/`, `.git/wt/trash/` — is intentionally preserved. It doesn't affect
read-path performance and benches may depend on it (e.g., branch markers set during
setup).

Worktree indexes survive every cache state. Git treats a missing index as every
tracked file being staged for deletion, which changes the repository rather
than cooling it. `invalidate_probe_caches` clears only `.git/wt/cache/`; the
prune benches use it for the first-scan-after-fetch shape while git metadata
stays warm.

**Which commands populate `.git/wt/cache/`:**

| Command | Populates? | Notes |
|---------|------------|-------|
| `wt list` | Yes | Post-skeleton tasks. Exits early under `WORKTRUNK_SKELETON_ONLY=1` / `WORKTRUNK_FIRST_OUTPUT=1` — those skip the writing phase. |
| `wt remove` | Yes | `prepare_worktree_removal` → `compute_integration_lazy` writes `is-ancestor` / `has-added-changes` / `merge-add-probe` whenever `BranchDeletionMode` is not `ForceDelete` (CLI `--force` is `force_worktree`, not `--force-delete`). |
| `wt step prune` | Yes | Every scanned worktree/branch runs `integration_reason` → the same probe writes as `wt remove`. First scan after new commits is cold; re-runs are warm (`prune_e2e/dry_run_probe_cold` vs `dry_run_warm`). |
| `wt switch <branch>` | No | No sha_cache writers on the direct-switch path. |
| `wt switch` (picker) | Yes | Preview pre-compute writes `picker-preview/{log,branch-diff,upstream-diff}-…` entries. Exercised under `WORKTRUNK_PREVIEW_BENCH=1` / `WORKTRUNK_PICKER_DRY_RUN=1`. |
| `wt` (completion via `COMPLETE=$SHELL`) | No | Only `for-each-ref` + worktree list. |

Clearing the default-branch cache as part of full invalidation is simpler than
introducing a separate "warm default branch" mode.
`invalidate_probe_caches` leaves it warm, like everything else outside
`.git/wt/cache/`.

**Bench fixtures don't exercise the wire path.** `setup_fake_remote` writes
`refs/remotes/origin/HEAD` directly into every repo, so a cold-cache iteration
falls through to the local `<r>/HEAD` lookup, never to `git ls-remote`. The
cold cost we benchmark is the *configured-remote* cold cost, not the
*fresh-clone* cold cost. A
`cold_no_remote` mode (extending `invalidate_caches_auto` to also wipe
`refs/remotes/origin/HEAD`) would close the gap if the wire-path cost is
worth measuring at CI cadence.

## Expected relationships

- The 1- and 8-worktree rows track the endpoint cost of adding linked
  worktrees under both cache modes.
- Cold rows should be slower than their warm neighbors because they rebuild
  the persistent caches named above.
- Large-repository rows amplify history-walk and working-tree costs that generated
  content cannot model faithfully.

## Recording `wt remove` / `wt step prune` staging

The removal commands interleave per-target work with parallel scans and
detached background processes; a single e2e number hides which phase moved.
Record them in two layers:

**Criterion cadence** — `benches/remove.rs` and `benches/prune.rs`. The
generated prune overlay adds 4 squash-merged worktrees and 4 squash-merged
branches as candidates, plus 8 two-sided-diverged worktrees and 8 branches as
backdrop. `prune_large_repository` layers 12 candidate pairs and 24 backdrop pairs
onto the canonical imported base. Its source clone is cached, but the mutable
multi-gigabyte fixture is built fresh for the benchmark process and removed
when it exits. That group is opt-in via `--features large-repository-benches` and
must never build on a hosted CI runner.

| Variant | What it measures |
|---------|------------------|
| `prune_e2e/dry_run_probe_cold` | full parallel scan with `.git/wt/cache/` cleared; git's own caches stay warm, matching the first prune after fetching the default branch |
| `prune_e2e/dry_run_warm` | steady-state re-scan with integration probes served from sha_cache |
| `prune_e2e/dry_run_cold` | first scan with worktrunk caches, the default-branch cache, and git's commit graph cleared |
| `prune_e2e/live` | probe-cold scan plus parallel removal of the 8 candidates |
| `prune_large_repository/dry_run_warm` | steady-state scan at large-repository history and working-tree scale |
| `prune_large_repository/dry_run_probe_cold` | the same scan with integration probes rerun; statuses stay stat-warm |
| `remove_e2e/{warm,cold}/no_hooks` | full removal with and without persistent caches |
| `remove_e2e/warm/with_hooks` | warm-cache neighbor isolating hook approval and spawning |
| `first_output/remove` | single-target validation up to first output (`benches/time_to_first_output.rs`) |

Full-cold and live at large-repository scale are **one-shot timelines, not
criterion groups**. Full-cold adds commit-graph rebuilding; live consumes the
candidates. Live removals run concurrently inside the `prune-scan` window,
while packed candidate refs may serialize briefly on `packed-refs.lock`.

An actual first checkout can be slower because every worktree status is
stat-cold. The cache helpers preserve indexes because removing one changes
staged state; use a fresh fixture when investigating checkout-cold status.
The generated fixture can't show it — its statuses are milliseconds — so
scale-sensitive changes need a one-shot on a fresh imported fixture (or
`wt-perf timeline -- -C <repo> step prune --dry-run` on a real
repository) alongside the criterion cadence. All large-repository numbers are
I/O-bound and move with ambient machine load (sibling builds, Spotlight):
treat them as shape, not thresholds, and compare Criterion verdicts with
`uptime`.

The probe-cold prune benches run through `bench_wt` with
`CacheState::ProbeCold`. Fixture correctness is checked outside the timed
loops: a post-setup dry-run pins the candidate count, and every live sample
must consume its fresh fixture's candidate refs while retaining the backdrop.

**Phase attribution** — `wt-perf timeline` plus the removal spans. Prune emits
`prune-gather` (worktree+branch enumeration), `prune-scan` (the whole parallel
check region), one `prune-check:<ref>` per scanned item, and one
`prune-remove:<label>` per removed candidate; `wt remove` emits
`internal-sweep` around its end-of-command janitor. The `prune-remove` spans
sit *inside* the `prune-scan` window on the live path and overlap each other —
removals execute concurrently on the worker pool, holding the scan lock's read
side. A span covers any wait for the lock plus the removal itself; the
exceptional candidates that take the write side (hook-bearing, `--foreground`,
metadata-pruning — `removal_needs_write` in `src/commands/step/prune.rs`) also
wait for every in-flight check and removal to drain first.

```bash
cargo run -p wt-perf -- setup generated 0 0 --prune-candidates 4 --prune-backdrop 8 --path target/wt-prune-generated
# A freshly built fixture is already probe-cold (empty sha_cache).
cargo run -p wt-perf -- timeline -- -C target/wt-prune-generated step prune --dry-run --min-age 0s
cargo run -p wt-perf -- timeline -- -C target/wt-prune-generated step prune --min-age 0s
```

**Live prune at large-repository scale is a one-shot timeline, not a criterion group** —
each live run consumes the candidates, and constructing the multi-gigabyte fixture
costs minutes. Give setup a new explicit path; it refuses to overwrite an
existing destination. The pinned source clone is reused, while the mutable
fixture has no cache state to validate or repair. The probe-cold timeline keeps
git metadata warm:

```bash
cargo run -p wt-perf -- setup imported --prune-candidates 12 --prune-backdrop 24 --path target/wt-prune-imported
cargo run -p wt-perf -- timeline -- -C target/wt-prune-imported step prune --min-age 0s
# For another run, remove target/wt-prune-imported explicitly or choose a new path.
```

**`wt remove` keeps working after its last message.** The in-process sweep
(`run_internal_sweep`) runs before exit, while the shell wrapper waits on the
process: one `pgrep` for `git fsmonitor--daemon` processes plus one `lsof` over
the whole PID set. Two spawns whatever the machine-wide daemon count, so
`remove_e2e` misses a fixed tail rather than one that grows with the machine.
To observe it, run `wt-perf timeline -- remove <branch>` on a real machine and
read the `internal-sweep` span; the `fsmonitor sweep: resolving sockets for N
daemon(s) via one lsof` debug line gives the count.

## Output Locations

Ephemeral generated fixtures use the system temporary directory. Manual
`wt-perf setup` fixtures live at the explicit `--path` and are never
overwritten. The revision-keyed imported source and temporary
imported runs live under `target/wt-perf/bench-repos/`; that directory is
per-worktree and reaped by `cargo clean`.

- Results: `target/criterion/`
- Cached corpus source: `target/wt-perf/bench-repos/imported/source-<revision>/`
- Temporary imported runs: `target/wt-perf/bench-repos/imported/runs/`
- HTML reports: `target/criterion/*/report/index.html`

## Performance Investigation with wt-perf

Use `wt-perf` to set up benchmark repos and generate Chrome Trace Format for visualization.

### Setting up benchmark repos

```bash
# Set up the default generated repo: 8 total worktrees and 50 branches.
cargo run -p wt-perf -- setup generated --path target/wt-generated

# `wt-perf setup --help` lists every recipe and its semantic count names.
# Reproduce the completion fixture, including its remote-tracking-ref population:
cargo run -p wt-perf -- setup generated 24 120 1400 --path target/wt-generated-completion

# Build a fresh imported prune fixture from the cached pinned source:
cargo run -p wt-perf -- setup imported --prune-candidates 12 --prune-backdrop 24 --path target/wt-prune-imported
```

### Generating traces

`wt-perf timeline` runs a `wt` invocation with `-vv` (which writes the
machine `trace.jsonl`), reads that back, and renders. Default mode is a
sorted text timeline; `--chrome` emits Chrome Trace Format JSON for
Perfetto/chrome://tracing. `--cold` invalidates caches first.

```bash
# Text timeline of one wt invocation
cargo run -p wt-perf -- timeline -- list --progressive

# Cold-cache run (invalidates the traced repo — the `-C` arg, else cwd)
cargo run -p wt-perf -- timeline --cold -- -C target/wt-generated list --progressive

# Chrome Trace Format JSON for Perfetto
cargo run -p wt-perf -- timeline --chrome -- list --progressive > trace.json
# Open in https://ui.perfetto.dev or chrome://tracing
```

`--progressive` is still required: `wt-perf timeline` runs wt with stdout
piped to /dev/null, so TTY-gated events (`Skeleton rendered`, `First
result received`) won't fire without it.

For Chrome JSON from a `trace.jsonl` already captured to disk (e.g. a CI
artifact), feed it to `wt-perf trace` instead:

```bash
wt -vv list --progressive --branches
cargo run -p wt-perf -- trace .git/wt/logs/trace.jsonl > trace.json
```

The text-timeline summary reports `traced` (first → last record, what the
spans actually cover) and `wall` (externally-measured
spawn → wait, the true process duration). The gap between them is
prelude/epilogue not visible to the trace — process spawn, dyld, code
that runs before `init_logging` registers the trace epoch, and the exit
path after the last span drops.

### Analyzing a trace

`wt config state logs profile [FILE]` answers the three questions below from a
captured `trace.jsonl` without leaving the terminal: subprocess time by command
type and by worktree (BY COMMAND TYPE / BY CONTEXT), the slowest individual
jobs, the parallelism factor and peak concurrency, same-context duplicate
commands (CACHE), and the collect milestones (KEY INTERVALS / PHASES).
`--format=json` emits the same data for scripting.

```bash
wt -vv list --progressive
wt config state logs profile             # human report
wt config state logs profile --format=json | jq .cache
```

For visual critical-path inspection — the one thing the aggregate report can't
show — open the Chrome Trace JSON (`wt-perf timeline --chrome`, or `wt-perf
trace` on an existing `trace.jsonl`) in <https://ui.perfetto.dev> or
chrome://tracing.

**A traced run skips prewarm's rev-parse batch, so its startup ordering is
not quite the run users get.** At `-vv`, `logging::init` opens the log sinks
through `log_files::try_create`, which calls `Repository::current()` and so
resolves the git common dir in a fork the trace never sees (the subscriber
isn't installed yet). `Repository::prewarm` gates each of its threads on the
cache it populates: the git-config and user-config preloads still run — their
spans appear in the trace — but the rev-parse batch is skipped because its
product is already cached, so the per-worktree discovery (`WORKTREE_ROOTS`,
`GIT_DIRS`, `CURRENT_BRANCHES`) happens on demand via `prewarm_info` instead
of overlapped at startup.

Three consequences when reading one:

- The run's first fork (the common-dir rev-parse) is invisible — it lands in
  the untraced gap before the first span.
- Don't conclude the per-worktree discovery is on-demand in production because
  the trace shows a `prewarm_info` refork mid-command; in production the
  rev-parse batch covers it at startup.
- Don't measure a startup change by trace alone. Time the real binary
  (`hyperfine` on the shipped path) and, for a fork inventory that doesn't perturb
  the run, put a logging shim named `git` at the front of `PATH` — a two-line
  `sh` script that appends `"$*"` to a file and `exec`s the real git. That counts
  every spawn with no verbosity flag set.

### Performance questions

Three questions drive `wt list` performance work:

1. **Where does time go?** Which subprocess types consume the most total time? The category with the highest total is where optimization effort has the most impact — `by_type` and `slowest` in the profile.

2. **How parallel are we?** Total subprocess time divided by wall time gives a parallelism factor. A factor of 4.0 means 4 commands running concurrently on average. Close to 1.0 means mostly serial execution with headroom to parallelize — `parallelism` and `peak_concurrency` in the profile.

3. **What's on the critical path?** The critical path passes through serial phases (setup, finalization) plus the slowest work item in the parallel phase. The profile's `phases` (milestone gaps) and `by_context` (per-worktree totals — the worktree with the highest total is the likely parallel bottleneck) bound it, but the trace format doesn't capture task dependencies, so visualizing the trace in Perfetto is more useful here.

### Generating traces from benchmark repos

```bash
# Trace the canonical imported fixture
cargo run -p wt-perf -- setup imported --path target/wt-imported
cargo run --release -q -- -vv -C target/wt-imported list --progressive --branches
cargo run -p wt-perf -- trace target/wt-imported/.git/wt/logs/trace.jsonl > imported-trace.json
```

## Key Performance Insights

**`git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs)**

This command walks the commit graph to compute divergence. Its cost on the
imported corpus is driven primarily by history depth, not the number of
rendered rows.

**Branch-row costs** (rust-lang/rust with 50 branch-only rows):
- Cold rows pay merge-base/merge-tree work per branch.
- Warm rows reuse merge-tree, integration-probe, diff-stat, and ancestry entries.
- Default rows still capture refs but skip branch-only integration and rendering tasks.

The persistent SHA-keyed cache (`.git/wt/cache/`) amortizes the first-run cost across
subsequent invocations. Cache entries are eternally valid since they're keyed on commit
SHAs.
