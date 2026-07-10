# Design: canonicalize the benchmark / wt-perf system

Status: proposal (no production code). This answers one question:

> The perf tooling has grown five layers: trace emission, the analysis library
> in `src/trace/`, the `wt-perf` helper crate, seven Criterion bench targets
> with a daily CI run, and `benches/CLAUDE.md`. Where do they duplicate each
> other, and what can be canonicalized or cut without losing meaningful
> capability?

Short answer: the emission side (`src/trace/emit.rs` and the `CommandTrace`
chokepoint) is already canonical and stays untouched. The duplication is on the
*consumption* side: two CLIs expose the same analysis, the timeline renderer
re-implements label formatting the library already owns, the one analysis the
library doesn't compute keeps an 80-line SQL section alive in
`benches/CLAUDE.md`, the warm/cold iteration pattern is copy-pasted across five
bench files, and three bench groups guard the same regression shape. Each
proposal below deletes one of those parallel paths.

## Summary of recommendations

1. **Delete `wt-perf cache-check`.** Its output is the `cache` field of
   `wt config state logs profile --format=json`: the same `CacheReport`,
   built from the same entries.
2. **Move the timeline renderer into `worktrunk::trace`** and render it with
   the library's existing label/table/duration helpers. `wt-perf` keeps the
   harness work (spawn `wt -vv`, external wall clock, cold invalidation) and
   delegates all rendering.
3. **Add a `by_context` table to `Profile`, then cut the trace_processor SQL
   section from `benches/CLAUDE.md`.** Per-worktree subprocess totals are the
   only documented query `logs profile` doesn't already answer.
4. **One shared warm/cold bench runner in `wt_perf`.** Hoist `run_benchmark`
   from `benches/list.rs` into the library; the `PerIteration` rationale is
   written once, on the helper, instead of five times across bench files.
5. **Prune the bench matrix**: `real_repo` keeps only the 8-worktree variants,
   `cow_copy` is deleted, and the `remove_e2e/first_output` arm (a duplicate of
   `first_output/remove`) is deleted. Saves roughly 15–20 minutes of the ~80
   minute daily run.
6. **Move the `wt-perf` CLI tests into the `wt-perf` package.** In-package
   integration tests get `CARGO_BIN_EXE_wt-perf` natively, which retires the
   dummy `builds.rs` and drops `wt-perf` from the nextest setup script.
7. **Fold `mixed-W-B` into `parse_config`** so `wt-perf setup` has one config
   parser instead of a parser plus a special case.

Items 1–4 and 6–7 are pure consolidation (no signal lost). Item 5 trades named
benchmark series for CI time; the losses are itemized below and each has a
covering twin or a documented fallback.

## The system today

| Layer | Where | Size | Role |
|-------|-------|------|------|
| Emission | `src/trace/emit.rs` + `src/logging.rs` layers | 435 | `CommandTrace`/`Span`/`instant` → `trace.jsonl` / `trace.log` |
| Analysis library | `src/trace/{parse,profile,chrome}.rs` | ~2,060 | parse back; `Profile`/`CacheReport`; Chrome Trace export |
| wt-perf crate | `tests/helpers/wt-perf/` | ~1,530 | fixture builders (lib) + CLI: `setup`, `invalidate`, `trace`, `cache-check`, `timeline` |
| Benches | `benches/*.rs` (7 targets) | 1,255 | Criterion; daily `benchmarks.yaml` run (~80 min) feeding the gist time series |
| Docs | `benches/CLAUDE.md` | 352 | run examples, cache handling, fixture notes, trace_processor SQL |

Analysis consumers: `wt config state logs profile` (text + JSON), `wt diagnose`
(embeds the rendered profile), and the `wt-perf` CLI.

The canonicalization principle the proposals converge on: **capture and
harness work lives in `wt-perf`; analysis lives in `worktrunk::trace` with
`wt config state logs profile` as its one CLI surface.** `wt-perf timeline`
earns its place because it does harness work a passive reader can't (spawn
under `-vv`, measure spawn→wait wall externally, invalidate for cold runs);
`cache-check` doesn't, because it's a passive reader of the same file
`logs profile` reads.

## Proposals

### 1. Delete `wt-perf cache-check`

`cache_check()` in `tests/helpers/wt-perf/src/main.rs` is
`CacheReport::from_entries` + `serde_json::to_string_pretty`. `handle_logs_profile`
with `--format=json` serializes `Profile`, whose `cache` field is the same
struct from the same entries. Two documented entry points for one analysis;
`jq .cache` closes the gap. Delete the subcommand and its `after_long_help`.

### 2. One renderer per view: move the timeline into `worktrunk::trace`

`render_timeline`/`describe` in `wt-perf/src/main.rs` (~120 lines plus
snapshots) duplicate the library:

- `describe()` re-implements `profile.rs::command_label` (the `cmd [ctx]` +
  `(ok=false)` / `(err: …)` shape), and the two have already drifted: the
  timeline pads two spaces before the failure marker, the profile one.
- The timeline aligns columns with `tabwriter` while `profile.rs` has its own
  `render_table`; durations render via `Duration`'s `Debug` (`4.5ms`, `1.5s`)
  in one and fixed-point `fmt_dur` (`4.50ms`) in the other.

Move the renderer into `src/trace/` as
`render_timeline(&[TraceEntry], wall: Duration) -> String`, built on
`command_label`, `render_table`, and `fmt_dur`. `wt-perf timeline` keeps the
spawn/invalidate/measure logic and calls it. The `tabwriter` dependency and the
duplicated label code go away; timeline durations switch to the fixed-point
format (a cosmetic change to a dev tool's output).

Also under this item: the `logs profile` help currently points users at
`cargo run -p wt-perf -- timeline`, a tool that exists only in this repo. Move
that pointer to `benches/CLAUDE.md`, where every reader can actually run it.

### 3. `by_context` in `Profile`; retire the SQL section

`benches/CLAUDE.md` carries ~80 lines of trace_processor SQL for three
questions. `Profile` already answers nearly all of it:

| CLAUDE.md query | Profile field |
|-----------------|---------------|
| #1 slowest individual commands | `slowest` |
| #1 total time by command type (hand-rolled `CASE` buckets) | `by_type` (via `command_type`, which the SQL approximates) |
| #2 parallelism factor | `parallelism`, `peak_concurrency` |
| #3 phase durations from milestones | `phases`, `key_intervals` |
| #3 per-worktree totals (`EXTRACT_ARG(args.context)`) | **missing** |

Add `by_context: Vec<ContextStat>` (context, count, total; busiest first) to
`Profile` — a few lines in `from_entries`, one more table in `render_text`,
one more array in the JSON. Then the SQL section reduces to one line: open the
Chrome JSON in Perfetto for visual critical-path inspection, which is the one
thing SQL-over-slices was never good at anyway (the section itself says so).
The `command_type` bucketing also stops having a hand-maintained SQL shadow
that must track new command shapes.

`chrome.rs` stays: Perfetto visualization is the remaining consumer and has no
substitute.

### 4. One warm/cold bench runner

`benches/list.rs` has `run_benchmark` (binary, repo, cold flag, args, env);
`remove.rs`, `picker_preview.rs`, `alias.rs`, and `time_to_first_output.rs`
re-inline the same warm/cold split, and each copy carries its own multi-line
retelling of why `BatchSize::PerIteration` beats `SmallInput`. Hoist the
helper into the `wt_perf` library (it already owns `invalidate_caches_auto`,
the other half of the pattern), document the `PerIteration` rationale once on
it, and add the success assertion most call sites bolt on. Bench files keep
only what's genuinely per-bench: fixtures, args, and setup closures like
`recreate_worktree`.

`benches/CLAUDE.md`'s "Cache Handling" section then references the helper
instead of prescribing the pattern for hand-rolling.

### 5. Prune the bench matrix

The daily run costs ~80 minutes. Three cuts where cost × redundancy is
highest:

- **`real_repo`: keep `{warm,cold}/8`, drop the 1- and 4-worktree variants.**
  Each variant clones rust-lang/rust locally and the cold ones rebuild
  59k-entry indexes per iteration; this group dominates the run. The
  worktree-count scaling *shape* is already tracked at criterion cadence by
  `worktree_scaling` (1/4/8, synthetic); what's unique to `real_repo` is
  real-repo magnitude and the cold penalty, which the 8-worktree endpoints
  keep. Lost: the real-repo scaling curve's interior points.
- **Delete `cow_copy`.** It compares production `copy_dir_recursive` against a
  40-line serial copy that exists only inside the bench — a shadow
  implementation maintained to validate a settled design choice (rayon
  parallel copy). Six size/shape variants × 2 impls at daily cadence guard a
  code path that changes only when `reflink_copy` or the copy loop does.
  Lost: a throughput series for `wt step copy-ignored`. If that loss bites,
  the fallback is re-adding a single parallel-only variant.
- **Delete the `remove_e2e/first_output` arm.** Its own comment says it is
  "the same as time_to_first_output"; two series measure one quantity from two
  files. The cross-group comparison (`remove_e2e/no_hooks` vs
  `first_output/remove`) still works — both land in the same gist.

Considered and skipped: thinning `skeleton`/`worktree_scaling` to 1-and-8.
They're cheap (modest fixture, 15s budgets), and the interior point is what
distinguishes linear from superlinear drift.

### 6. Move the wt-perf CLI tests into the wt-perf package

`tests/integration_tests/analyze_trace.rs` (four tests of `wt-perf trace`)
lives in the main integration suite, so it reaches the binary through
`workspace_bin()` — which is why the nextest setup script pre-builds `wt-perf`
and why the crate carries a dummy `tests/builds.rs` whose only job is forcing
binary compilation. Integration tests *inside* the `wt-perf` package get
`CARGO_BIN_EXE_wt-perf` from cargo directly, and building them builds the
binary. Move the four tests there; delete `builds.rs` (the real tests now do
its job); drop `wt-perf` from `.config/nextest.toml`'s setup script (mock-stub
still needs the layer, so the script itself stays).

Behavior change: `cargo test --test integration` no longer runs these four
tests; full-workspace runs (the gate, CI, `cargo nextest run`) still do, since
`wt-perf` is in `default-members`.

### 7. One config parser in `wt-perf setup`

`parse_config` handles `typical-N`/`branches-N[-M]`/`divergent`/`picker-test`;
`mixed-W-B` is parsed by a separate `parse_mixed` in `main.rs` with its own
`match` arm and an `unreachable!`. Return an enum
(`Flat(RepoConfig)` / `Mixed { worktrees, branches }`) from `parse_config` and
the special case collapses. The fixture builders themselves
(`create_repo_at` vs `create_mixed_repo_at`) share only trivial plumbing (init,
auto-maintenance config, gc) — worth a small shared `init_repo` helper, but no
deeper unification: their fixture shapes are deliberately different, and
reshaping fixtures moves every benchmark's level in the gist time series.

## What stays as is

- **Emission** (`emit.rs`, the `CommandTrace` chokepoint, the two-rendering
  split in `logging.rs`): already single-path by design.
- **The wt-perf binary as a separate package.** Folding its commands into `wt`
  would ship fixture generators to users, and cargo-dist ships every `[[bin]]`
  in the main crate (the documented reason the helper packages exist).
- **Seven separate `[[bench]]` targets.** Merging them saves link time but
  costs per-target compile selectivity (`cargo bench --bench list` builds one
  target), which matters more during iteration.
- **`wt-perf trace`** (jsonl → Chrome JSON for an already-captured file, e.g. a
  CI artifact): the file-input twin of `timeline --chrome`, five lines of CLI
  over the library.
- **Fixture shapes and surviving benchmark IDs**: the gist time series keys on
  them.

## Sequencing

Items 1–4, 6, 7 are one consolidation PR each (or one combined); no series is
affected. Item 5 ends three series and should land as its own PR so the
discontinuity in the gist has a single date and commit to point at.
`benches/CLAUDE.md` shrinks in the same PRs that obsolete its sections (the
SQL section with item 3, the hand-rolled cache-handling pattern with item 4).
