# Benchmark Guidelines

See `list.rs` and `time_to_first_output.rs` headers for benchmark groups and run examples.

## Quick Start

Criterion's CLI takes a positional `FILTER` (substring inclusion) and `--exact`. There's no `--skip`; pick a filter that *includes* what you want instead.

```bash
# Fast iteration (one synthetic group, warm cache only)
cargo bench --bench list skeleton/warm

# Run specific group (all variants)
cargo bench --bench list full

# GH #461 scenario (200 branches on rust-lang/rust)
cargo bench --bench list real_repo_many_branches

# All list benchmarks (~1 hour)
cargo bench --bench list

# Time-to-first-output benchmarks
cargo bench --bench time_to_first_output         # all commands
cargo bench --bench time_to_first_output remove  # just remove

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
warm/cold strategy (plain `iter` warm; invalidate-per-iteration cold — its doc
comment carries the `BatchSize::PerIteration` rationale). A cold benchmark whose
setup is not plain invalidation (e.g. `remove_e2e` recreating the removed
worktree) uses `iter_batched` with `BatchSize::PerIteration` directly.

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
drop every worktree candidate). A benchmark exercising such a gate must pair
`invalidate_caches_auto` with `wt_perf::restore_worktree_indexes`, which
`git reset -q`s every worktree back to a clean `git status` while leaving the
integration probes cold. It's a separate call, not folded into
`invalidate_caches_auto`, because `git reset --mixed` discards
staged-but-uncommitted index state that some fixtures plant on purpose (and
that a real repo targeted by `wt-perf invalidate` / `timeline --cold` may hold
as genuine work in progress) — pair it only with fixtures whose dirt is
untracked files.

**Which commands populate `.git/wt/cache/`:**

| Command | Populates? | Notes |
|---------|------------|-------|
| `wt list` | Yes | Post-skeleton tasks. Exits early under `WORKTRUNK_SKELETON_ONLY=1` / `WORKTRUNK_FIRST_OUTPUT=1` — those skip the writing phase. |
| `wt remove` | Yes | `prepare_worktree_removal` → `compute_integration_lazy` writes `is-ancestor` / `has-added-changes` / `merge-add-probe` whenever `BranchDeletionMode` is not `ForceDelete` (CLI `--force` is `force_worktree`, not `--force-delete`). |
| `wt switch <branch>` | No | No sha_cache writers on the direct-switch path. |
| `wt switch` (picker) | Yes | Preview pre-compute writes `picker-preview/{log,branch-diff,upstream-diff}-…` entries. Exercised under `WORKTRUNK_PREVIEW_BENCH=1` / `WORKTRUNK_PICKER_DRY_RUN=1`. |
| `wt` (completion via `COMPLETE=$SHELL`) | No | Only `for-each-ref` + worktree list. |

Default-branch cache contribution is ~17ms per iteration on a typical-8 synthetic repo
(measured: 166ms with default-branch cached → 183ms fully cold). Small enough that
always clearing it is simpler than introducing a "warm default-branch" bench mode.

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

## Output Locations

- Results: `target/criterion/`
- Cached rust repo: `target/bench-repos/rust/`
- HTML reports: `target/criterion/*/report/index.html`

## Performance Investigation with wt-perf

Use `wt-perf` to set up benchmark repos and generate Chrome Trace Format for visualization.

### Setting up benchmark repos

```bash
# Set up a repo with 8 worktrees (persists at /tmp/wt-perf-typical-8)
cargo run -p wt-perf -- setup typical-8 --persist

# Available configs:
#   typical-N       - 500 commits, 100 files, N worktrees
#   branches-N      - N branches, 1 commit each
#   branches-N-M    - N branches, M commits each
#   divergent       - 200 branches × 20 commits (GH #461 scenario)
#   mixed-W-B       - W worktrees + B branches in varied states (the `full` fixture)
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

# Cold-cache run
cargo run -p wt-perf -- timeline --cold --repo /tmp/wt-perf-typical-8 -- \
  -C /tmp/wt-perf-typical-8 list --progressive

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
