# Benchmark Guidelines

See `list.rs` and `time_to_first_output.rs` headers for benchmark groups and run examples.

## Quick Start

Criterion's CLI takes a positional `FILTER` (substring inclusion) and `--exact`. There's no `--skip`; pick a filter that *includes* what you want instead.

```bash
# Fast iteration (one synthetic group, warm cache only)
cargo bench --bench list skeleton/warm

# Run specific group (all variants)
cargo bench --bench list full

# GH #461 scenario (50 branches at varying history depths on rust-lang/rust)
cargo bench --bench list real_repo_many_branches

# All list benchmarks (~1 hour)
cargo bench --bench list

# Time-to-first-output benchmarks
cargo bench --bench time_to_first_output         # all commands
cargo bench --bench time_to_first_output remove  # just remove

# wt step prune (scan + removal on the squash-merged fixture)
cargo bench --bench prune                        # synthetic variants
cargo bench --bench prune --features real-repo-benches prune_real_repo  # rust scale (~15 GiB cached fixture)

# Picker preview pre-compute (wt switch preview workload)
cargo bench --bench picker_preview               # all variants
cargo bench --bench picker_preview warm          # warm only
```

## Rust Repo Caching

Real repo benchmarks clone rust-lang/rust on first run (~2-5 minutes). The clone is cached in `target/bench-repos/` and reused. Corrupted caches are auto-recovered.

## Faster Iteration

Criterion has no exclusion flag — narrow the run by picking a substring that matches only the variants you want. Benchmark IDs look like `<group>/<label>/<param>`, e.g. `skeleton/cold/4`, `worktree_scaling/warm/8`, `full/cold`, `real_repo_many_branches/warm`.

**Pattern matching (positional `FILTER`):**
```bash
cargo bench --bench list scaling             # All worktree_scaling/* variants
cargo bench --bench list warm                # Every benchmark whose ID contains "warm"
cargo bench --bench list skeleton/warm       # Just skeleton's warm variants
cargo bench --bench list full                # Both cache states of the combined fixture
cargo bench --bench list -- --exact full/cold   # One exact ID
```

To skip the slow real-repo and divergent groups, target the synthetic groups directly: `cargo bench --bench list skeleton`, `cargo bench --bench list worktree_scaling`, or `cargo bench --bench list full`. Run them sequentially if you want more than one.

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
`picker_preview` benchmarks to measure the preview pool workload without standing
up a PTY. Bypasses the picker's TTY check, like `WORKTRUNK_PICKER_DRY_RUN=1`.

The hot path inside the env-gated block is identical to the dry-run path; only the
post-drain output (cache JSON dump + stashed-warning drain) is conditional. Keep new
post-drain work out of the bench path unless it's part of the workload being
measured.

## Cache Handling

Worktrunk maintains a persistent SHA-keyed cache at `.git/wt/cache/` plus a git-config
cache of the default branch at `worktrunk.default-branch`. Both survive process exits,
so bench iterations read from prior iterations unless invalidated.

**Rule:** if a benchmark runs a `wt` subcommand that populates these caches, every
iteration must start cold — otherwise iter 1 measures the real cost and iter 2+ measure
a cache hit. Run the iteration through `wt_perf::bench_wt`, the one home of the
warm/cold strategy (plain `iter` warm; invalidate-per-iteration full- or
probe-cold, picked by `CacheState` — its doc comment carries the
`BatchSize::PerIteration` rationale). A benchmark whose setup is not plain
invalidation (e.g. `remove_e2e` recreating the removed worktree, prune's
`live` re-creating its candidates) uses `iter_batched` with
`BatchSize::PerIteration` directly. Timed iterations assert only the child's
exit status; anything stronger (candidate counts, fixture shape) runs once
outside the timed loop (see `verify_candidates` in `benches/prune.rs`).

`invalidate_caches_auto` clears:

- `.git/index` (main and linked worktrees)
- `.git/objects/info/commit-graph*`
- `.git/wt/cache/` (all sha_cache kinds + ci-status + summaries)
- `worktrunk.default-branch` (git config)

`.git/packed-refs` is deliberately preserved: `create_repo_at` runs `git gc`
at the end of fixture setup, which packs every loose ref into `packed-refs`
and prunes the loose copies. Deleting that file post-gc leaves the repo with
no resolvable refs, so any bench that resolves a branch (e.g. the `with_vars`
alias's `{{ commit }}` template var) blows up partway through warm-up.

User state — `worktrunk.history`, `worktrunk.hints.*`, `worktrunk.state.<branch>.*`,
`.git/wt/logs/`, `.git/wt/trash/` — is intentionally preserved. It doesn't affect
read-path performance and benches may depend on it (e.g., branch markers set during
setup).

**Deleting a worktree's index isn't a cold cache.** Git treats a missing index
as empty, so `git status` reports every tracked file as a staged deletion — a
*different repo state*, which flips any clean-worktree gate a benchmarked
command exercises (e.g. `wt step prune`'s removability check would silently
drop every worktree candidate). A benchmark exercising such a gate must
invalidate with `wt_perf::invalidate_probe_caches` instead, which clears only
`.git/wt/cache/`: the integration probes re-run while git's own state stays
warm — indexes with stat data, commit graph, `worktrunk.default-branch` —
exactly the state a real repo is in right after a fetch. The prune benches use
it (via `bench_wt`'s `ProbeCold` state) for their `dry_run_probe_cold`
variants; a one-time post-setup dry-run (`verify_candidates`) pins the
candidate count, so a gate-flipping invalidation can't silently shrink the
scanned workload. (`ensure_prune_real_repo` heals a
fixture whose indexes were deleted by a stray `wt-perf invalidate` /
`timeline --cold`, rebuilding them via `git reset -q` — safe there because the
fixture's only dirt is untracked files.)

**Which commands populate `.git/wt/cache/`:**

| Command | Populates? | Notes |
|---------|------------|-------|
| `wt list` | Yes | Post-skeleton tasks. Exits early under `WORKTRUNK_SKELETON_ONLY=1` / `WORKTRUNK_FIRST_OUTPUT=1` — those skip the writing phase. |
| `wt remove` | Yes | `prepare_worktree_removal` → `compute_integration_lazy` writes `is-ancestor` / `has-added-changes` / `merge-add-probe` whenever `BranchDeletionMode` is not `ForceDelete` (CLI `--force` is `force_worktree`, not `--force-delete`). |
| `wt step prune` | Yes | Every scanned worktree/branch runs `integration_reason` → the same probe writes as `wt remove`. First scan after new commits is cold; re-runs are warm (`prune_e2e/dry_run_probe_cold` vs `dry_run_warm`). |
| `wt switch <branch>` | No | No sha_cache writers on the direct-switch path. |
| `wt switch` (picker) | Yes | Preview pre-compute writes `picker-preview/{log,branch-diff,upstream-diff}-…` entries. Exercised under `WORKTRUNK_PREVIEW_BENCH=1` / `WORKTRUNK_PICKER_DRY_RUN=1`. |
| `wt` (completion via `COMPLETE=$SHELL`) | No | Only `for-each-ref` + worktree list. |

Default-branch cache contribution is ~17ms per iteration on a typical-8 synthetic repo
(measured: 166ms with default-branch cached → 183ms fully cold). Small enough that
clearing it as part of the full invalidation is simpler than introducing a "warm
default-branch" bench mode. (`invalidate_probe_caches` leaves it warm, like
everything else outside `.git/wt/cache/`.)

**Bench fixtures don't exercise the wire path.** `setup_fake_remote` writes
`refs/remotes/origin/HEAD` directly into every repo, so a cold-cache iteration
falls through to the local `<r>/HEAD` lookup (~17 ms above), never to
`git ls-remote` (100 ms–2 s in the wild). The cold cost we benchmark is the
*configured-remote* cold cost, not the *fresh-clone* cold cost. A
`cold_no_remote` mode (extending `invalidate_caches_auto` to also wipe
`refs/remotes/origin/HEAD`) would close the gap if the wire-path cost is
worth measuring at CI cadence.

## Expected Performance

**Modest repos** (500 commits, 100 files):
- Cold cache penalty: ~5-16% slower
- Scaling: Linear with worktree count

**Large repos** (rust-lang/rust):
- Cold cache penalty: ~4x slower for single worktree
- Scaling: Warm cache shows superlinear degradation, cold cache scales better

## Recording `wt remove` / `wt step prune` staging

The removal commands interleave serial per-target work with parallel scans and
detached background processes; a single e2e number hides which phase moved.
Record them in two layers:

**Criterion cadence** — `benches/remove.rs` and `benches/prune.rs`. Expected
numbers on an M-series Mac (`prune-4-8` fixture: 4 squash-merged worktrees +
4 squash-merged branches as candidates, 8 two-sided-diverged worktrees + 8
diverged branches as backdrop, 200 commits, 100 files; `prune_real_repo` runs
warm and probe-cold dry-runs on the `prune-real` fixture — a rust-lang/rust clone with 12
squash-merged candidate pairs + 24 diverged worktrees and 24 diverged
branches, i.e. 36 linked worktrees, cached and self-repairing in
`target/bench-repos/rust-prune-12-24/`. That group is opt-in —
`cargo bench --bench prune --features real-repo-benches prune_real_repo` —
because its ~15 GiB fixture must never build on a hosted CI runner):

| Variant | Expected | What it measures |
|---------|----------|------------------|
| `prune_e2e/dry_run_probe_cold` | ~160 ms | full parallel scan, probes re-run (`.git/wt/cache/` cleared; git's own caches stay warm — the "first prune after fetching main" shape) |
| `prune_e2e/dry_run_warm` | ~90 ms | steady-state re-scan, probes hit sha_cache |
| `prune_e2e/live` | ~620 ms | probe-cold scan + serial removal of the 8 candidates (~60 ms each, under the scan write lock) |
| `prune_real_repo/dry_run_warm` | ~0.25–0.8 s | steady-state scan of 72 items (36 worktrees + 36 branches) at 331k-commit scale |
| `prune_real_repo/dry_run_probe_cold` | ~0.6–1 s | the same 72-item scan with probes re-running at real cost (statuses stay stat-warm) |
| `first_output/remove` | ~86 ms | single-target validation up to first output (`benches/time_to_first_output.rs`) |

Full-cold and live at rust scale are **one-shot timelines, not criterion
groups** (a full-cold criterion iteration costs ~1 min in re-hashing statuses
alone; a live one consumes the candidates). Expected one-shots on the
`prune-real` fixture:

- **full-cold dry-run ~5.5 s wall** (~46 s CPU over 472 subprocesses absorbed
  by the rayon pool) — the fresh-fixture shape, dominated by stat-cold
  `git status` at ~4.5 s per fresh worktree; the probes are `merge-base
  --is-ancestor` ~40 ms and `merge-tree --write-tree` ~130 ms (vs 4–25 ms
  synthetic, where shallow history walks bottom out at subprocess-spawn cost)
- **live ~12 s wall** — all 24 removals serialize under the scan write lock
  inside the `prune-scan` window: each of the 12 worktree candidates takes
  ~0.5–1.7 s (pre-remove re-checks plus drain waits), branch-only candidates
  ~50 ms

This is the "prune takes many seconds" experience users report: worktree
count × stat-cold statuses bounds the scan, and removals extend it serially.
The synthetic fixture can't show it — its statuses are milliseconds — so
scale-sensitive changes need a one-shot on `prune-real` (or
`wt-perf timeline -- -C <repo> step prune --dry-run` on a real repo) alongside
the criterion cadence. All rust-scale numbers are I/O-bound and move with
ambient machine load (sibling builds, Spotlight): treat them as shape, not
thresholds, and read criterion "regressed" verdicts on this bench against
`uptime` before believing them.

The probe-cold prune benches run through `bench_wt` with
`CacheState::ProbeCold`, never full invalidation (see "Deleting a worktree's
index isn't a cold cache" under Cache Handling). Fixture correctness is
checked once, outside the timed loops: a post-setup dry-run must list the
expected candidate count, and a live run that removed nothing trips the next
iteration's candidate re-creation (branch-name collision) instead of being
silently timed.

**Phase attribution** — `wt-perf timeline` plus the removal spans. Prune emits
`prune-gather` (worktree+branch enumeration), `prune-scan` (the whole parallel
check region), one `prune-check:<ref>` per scanned item, and one
`prune-remove:<label>` per removed candidate; `wt remove` emits
`internal-sweep` around its end-of-command janitor. The `prune-remove` spans
sit *inside* the `prune-scan` window on the live path — each removal takes the
scan lock's write side, so a span covers the wait for in-flight checks to
drain *plus* the removal itself: read it as "how long this removal stalled the
run", not as pure removal work.

```bash
cargo run -p wt-perf -- setup prune-4-8 --path /tmp/prune-repo
# A freshly built fixture is already probe-cold (empty sha_cache); don't use
# `timeline --cold` here — it deletes worktree indexes, which flips prune's
# clean-worktree gate and drops the worktree candidates from the run.
cargo run -p wt-perf -- timeline -- -C /tmp/prune-repo step prune --dry-run --min-age 0s
cargo run -p wt-perf -- timeline -- -C /tmp/prune-repo step prune --min-age 0s
```

**Live prune at real scale is a one-shot timeline, not a criterion group** —
each live run consumes the candidates, and re-creating them on rust costs
minutes per iteration. The `prune-real[-M-U]` fixture is built for this
workflow: it lives in `target/bench-repos/` (no `--path`), and after a live
prune the next `wt-perf setup prune-real` or `prune_real_repo` bench run
detects the consumed candidates and re-creates just them (~1 min) instead of
rebuilding the ~15 GiB fixture. Don't run two builders concurrently (a bench
and a setup racing can wipe each other's half-built tree), and don't
`wt-perf invalidate` it — deleting worktree indexes flips prune's
clean-worktree gate; the next setup/bench call heals that, but the run you
invalidated for measures a degraded scan:

```bash
cargo run -p wt-perf -- setup prune-real     # build or validate/repair the cache
cargo run -p wt-perf -- timeline -- -C target/bench-repos/rust-prune-12-24/repo step prune --min-age 0s
cargo run -p wt-perf -- setup prune-real     # repair the consumed candidates
```

**The `wt remove` exit-delay is machine-dependent and invisible to benches.**
After its last message, `wt remove` runs an in-process sweep
(`run_internal_sweep`) that enumerates `git fsmonitor--daemon` processes
*machine-wide* and resolves each one's socket with a ~50 ms `lsof` call —
sequential, before exit, while the shell wrapper waits on the process. On a
machine with N live daemons that appends roughly `N × 50 ms` of post-output
latency (measured: 115 daemons → 5.8 s after 0.4 s of actual removal output);
on daemon-free bench/CI machines it costs nothing, so `remove_e2e` never sees
it. To observe it, run `wt-perf timeline -- remove <branch>` on a real machine
and read the `internal-sweep` span and its `lsof -a -p …` children; the
`fsmonitor sweep: resolving sockets for N daemon(s)` debug line gives the
count.

## Output Locations

- Results: `target/criterion/`
- Cached rust repo: `target/bench-repos/rust/`
- Cached rust prune fixture: `target/bench-repos/rust-prune-<M>-<U>/` (repo +
  sibling worktrees + `round` counter for candidate re-creation)
- HTML reports: `target/criterion/*/report/index.html`

## Performance Investigation with wt-perf

Use `wt-perf` to set up benchmark repos and generate Chrome Trace Format for visualization.

### Setting up benchmark repos

```bash
# Set up a repo with 8 worktrees (persists at /tmp/wt-perf-typical-8; the
# next `setup typical-8` run wipes and rebuilds it)
cargo run -p wt-perf -- setup typical-8

# Available configs:
#   typical-N       - 500 commits, 100 files, N worktrees
#   branches-N      - N branches, 1 commit each
#   branches-N-M    - N branches, M commits each
#   divergent       - 200 branches × 20 commits (GH #461 scenario)
#   mixed-W-B       - W worktrees + B branches in varied states (the `full` fixture)
#   prune-M-U       - M squash-merged candidates + U two-sided-diverged
#                     worktrees/branches (the `wt step prune` workload; see
#                     benches/prune.rs)
#   prune-real[-M-U] - rust-lang/rust clone + M squash-merged candidate pairs
#                     + U diverged worktrees/branches (default 12-24), cached
#                     and self-repairing in target/bench-repos/ (no --path);
#                     first run clones from the network
#   picker-test     - Config for wt switch interactive picker testing

# Invalidate caches for cold run
cargo run -p wt-perf -- invalidate /tmp/wt-perf-typical-8
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
cargo run -p wt-perf -- timeline --cold -- -C /tmp/wt-perf-typical-8 list --progressive

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

### Performance questions

Three questions drive `wt list` performance work:

1. **Where does time go?** Which subprocess types consume the most total time? The category with the highest total is where optimization effort has the most impact — `by_type` and `slowest` in the profile.

2. **How parallel are we?** Total subprocess time divided by wall time gives a parallelism factor. A factor of 4.0 means 4 commands running concurrently on average. Close to 1.0 means mostly serial execution with headroom to parallelize — `parallelism` and `peak_concurrency` in the profile.

3. **What's on the critical path?** The critical path passes through serial phases (setup, finalization) plus the slowest work item in the parallel phase. The profile's `phases` (milestone gaps) and `by_context` (per-worktree totals — the worktree with the highest total is the likely parallel bottleneck) bound it, but the trace format doesn't capture task dependencies, so visualizing the trace in Perfetto is more useful here.

### Generating traces from benchmark repos

```bash
# Trace on rust-lang/rust (must run benchmark first to clone)
cargo run --release -q -- -vv -C target/bench-repos/rust list --progressive --branches
cargo run -p wt-perf -- trace target/bench-repos/rust/.git/wt/logs/trace.jsonl > rust-trace.json
```

## Key Performance Insights

**`git for-each-ref %(ahead-behind:BASE)` is O(commits), not O(refs)**

This command walks the commit graph to compute divergence. On rust-lang/rust:
- Takes ~2s regardless of how many refs are queried
- Only way to avoid it is to not enumerate branches at all

**Branch enumeration costs** (rust-lang/rust with 50 branches):
- First run (cold persistent cache): ~15-18s (expensive merge-base/merge-tree per branch)
- Subsequent runs (warm persistent cache): ~2-3s (cache hits on merge-tree / integration probes / diff stats / ancestry)
- Worktrees only: ~600ms (no branch enumeration)

The persistent SHA-keyed cache (`.git/wt/cache/`) amortizes the first-run cost across
subsequent invocations. Cache entries are eternally valid since they're keyed on commit
SHAs.
