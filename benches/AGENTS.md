# Benchmark guidelines

Bench groups and examples are documented at the top of their Rust files. Criterion takes a positional substring filter or --exact; it has no --skip. For example, cargo bench --bench list skeleton/warm runs one warm group. Large imported fixtures are opt-in and must not run on hosted CI.

## Fixtures and benches

Generated fixtures vary linked worktrees, branchless branches, remote-tracking refs, and how many linked worktrees are on a detached HEAD. Imported fixtures copy the pinned corpus in benches/imported-fixture. Prune candidates and backdrop are overlays on either base, not new fixture identities. wt-perf setup --help lists recipes. Each benchmark uses FixtureRepo for lifecycle and wt_command for subprocess isolation.

## Cache handling

A benchmark that populates persistent caches must choose warm or cold state explicitly through wt_perf::bench_wt. Destructive samples build fresh fixtures before timing and check postconditions after. invalidate_caches_auto clears Worktrunk caches, the cached default branch, and commit graphs; it preserves packed refs, indexes, and user state. Removing an index changes Git state and is not a cache invalidation. Prune probe-cold samples use CacheState::ProbeCold.

The fake remote supplies origin/HEAD locally. A cold-cache benchmark therefore measures a configured remote, not the first-run git ls-remote path.

WORKTRUNK_FIRST_OUTPUT stops selected commands at first visible output. WORKTRUNK_PREVIEW_BENCH measures the picker prelude through PreviewOrchestrator::wait_for_idle without launching skim. Their exact phase boundaries are in the benchmark and picker module docs.

## Recording wt remove / wt step prune staging

Use Criterion for repeatable cadence and wt-perf timeline for phase attribution. Live prune consumes its candidates; build a fresh fixture for each run. Imported fixtures are I/O bound, so compare shape and phase timings under similar machine load, not absolute thresholds. The prune-scan span includes concurrent prune-check and prune-remove spans; internal-sweep covers remove's final janitor.

## Analyzing a trace

Run cargo run -p wt-perf -- timeline -- list --progressive for a text timeline, or cargo run -p wt-perf -- timeline --chrome -- list --progressive for Perfetto JSON. --progressive enables TTY-gated milestones while stdout is piped. wt config state logs profile reports subprocess totals, slow jobs, concurrency, cache duplicates, and phases from an existing trace.jsonl.

Ask three questions of a trace: where subprocess time goes (by_type, by_context, slowest), how much work overlaps (parallelism and peak_concurrency), and which same-context commands repeat (cache). Use phases and the Chrome trace to inspect the critical path.

Verbose tracing itself resolves the Git common directory before the subscriber starts, skipping prewarm's normal rev-parse batch. Treat trace startup ordering as perturbed; time the ordinary binary for startup regressions. The trace's wall value covers spawn to exit, while traced covers only its first to last record.

Results are in target/criterion/; imported source and temporary runs are under target/wt-perf/bench-repos/.
