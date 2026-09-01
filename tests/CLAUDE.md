# Testing Guidelines

## Running the Suite

```bash
cargo run -- hook pre-merge --yes                                  # all tests + lints
pre-commit run --all-files                                         # lints only
cargo nextest run --all-features                                   # full suite, fastest runner
cargo nextest run --all-features -E 'test(/^integration_tests::list_layout::/)' # one module
cargo test --lib --bins                                            # unit tests
cargo test --test integration                                      # integration (no shell tests)
cargo test --test integration --features shell-integration-tests   # + shell tests
```

Every binary the suite spawns is `wt` itself — the mock commands are the same binary linked under other names, dispatching on argv[0] (`testing::mock_stub`) — so no run can spawn missing or stale code: cargo rebuilds a package's own binaries whenever its integration tests build, under every runner and filter, and `wt_bin()` resolves `CARGO_BIN_EXE_wt` — naming that just-built binary — into a hardlink pinned under `target/debug/wt-test-bin/`, which a concurrent `cargo build`'s uplift can't unlink mid-run (see No Retries); outside a cargo runner the suite panics ("CARGO_BIN_EXE_wt not set") rather than guessing a path. `cargo build --bin wt` recompiling right after a test run is the bin-only build being a separate cached unit (a different feature graph), not evidence the tests ran stale code.

**Claude Code web:** `task setup-web` installs zsh, fish, Nushell, PowerShell, `jq`, `lsof`, `gh`, pre-commit, and the Cargo dev tools. Install `task` first if needed: `sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b ~/bin` then `export PATH="$HOME/bin:$PATH"`. Tests that need an unprivileged uid skip automatically when running as root, which both this environment and Codex Cloud do; that covers the permission tests and the `wt remove` stuck-directory pair, the only automated coverage of that path.

**Shell/PTY tests** (`shell-integration-tests` feature): approval prompts, picker, progressive rendering, shell wrappers.

**The gate runs one platform, so `#[cfg(unix)]` hides dead code from it.** A helper, const, or import whose every use sits behind `#[cfg(unix)]` is live locally and dead on Windows, where `-D warnings` fails `test (windows)` with "never used". Gate the item with the same predicate as its uses. Cross-compiling to check locally doesn't work: the C build scripts (tree-sitter, libmimalloc-sys) fail before the Rust lint runs.

## One Result Per Test, Whatever Runs It

Five runners execute this suite — `cargo test`, `cargo nextest run`, `cargo llvm-cov nextest`, `cargo bench`, and the Nix `worktrunk-tests` derivation — and CI uses several of them on the same commit. **A test's result must not depend on which one started it.**

So `.config/nextest.toml` carries no setting that changes what a test observes: no `[env]`, no setup scripts. Anything load-bearing goes where every runner sees it — the harness-latched floor in `shell_exec` for git environment (see Git Config Isolation), `.cargo/config.toml` for `COLUMNS`, the fixture for behavior.

A nextest-only knob doesn't fail loudly when another runner misses it. It yields a *different result*, and the runner that disagrees is typically `cargo llvm-cov` — whose numbers gate a merge, and whose disagreement therefore reads as a coverage regression rather than as missing configuration.

## Profiling the Suite

```bash
task profile-tests   # CPU accounting plus per-test timings
```

The integration binary dominates: ~2,200 tests averaging ~1s, each spawning `wt` and `git` against a fresh fixture copy. Two thirds of the suite's CPU is kernel time, so the cost is process creation and filesystem churn rather than computation, and it sits in a broad middle rather than in a few outliers. Track `user`/`sys` from the `time` line; wall time is unreliable whenever a sibling worktree is building or testing, which on this project is most of the time. Per-test durations land in `target/nextest/default/junit.xml`.

The fixtures put their temp directories under `test_temp_root()` (`$TMPDIR/wt`) rather than directly in the system temp dir, and `test_tempdir()` is the fixture-side replacement for `TempDir::new()`. Entries in the shared temp root are cheap to ignore but expensive to enumerate, and `git::recover::recover_from_path` reads every ancestor directory of a deleted CWD — its own unit test ran 14.2s against a temp root holding 454k leaked entries, 0.06s once the leak stopped. That root's short name is load-bearing: a unix socket path can't exceed 104 bytes on macOS, and `test_copy_ignored_skips_non_regular_files` binds one 89 bytes in.

Process-scoped scratch space belongs in a fixed directory, not a `TempDir` in a `static`: statics don't run destructors at process exit, so under nextest's process-per-test model that leaks one directory per test into a temp root nothing reliably sweeps (macOS clears it only at boot). To check for a recurrence, run the suite with `TMPDIR` pointed at a fresh directory and see what survives.

## Coverage Investigation

`task coverage` runs the suite through nextest and writes an HTML report to `target/llvm-cov/html/index.html`. Coverage uses the same process isolation as the regular suite: PTY tests must not share one crowded test process. Both CI (the `coverage` workflow) and local `task coverage` pass `--features shell-integration-tests`, so code behind that flag is compiled and measured.

When `codecov/patch` fails, investigate before declaring ready (the merge gate itself is in the root `CLAUDE.md` → Coverage):

```bash
task coverage
cargo llvm-cov report --show-missing-lines | grep <file>   # authoritative miss list; same lines codecov counts
```

Since cargo-llvm-cov 0.9.0 that column collapses consecutive misses into ranges (`12-18`, not `12, 13, …`), so expand a range before comparing it against a codecov line list.

For each uncovered function, either write a test (integration tests via `assert_cmd_snapshot!` do capture subprocess coverage) or document why it's intentionally untested.

**Querying codecov directly** serves two cases the local report can't: disputing a posted check, and running in CI, where `task coverage` isn't installed. Prefer measuring everywhere else.

```bash
API=https://api.codecov.io/api/v2/github/max-sixty/repos/worktrunk
# Full SHAs throughout; an abbreviation 404s. `?pullid=N` compares the PR's
# *current* head, so name both SHAs to ask about an earlier commit.
curl -sL "$API/compare/?base=<base-sha>&head=<head-sha>" > /tmp/codecov.json

# Patch coverage per file. `.name` is an object, and the files carrying patch
# lines are the ones with `has_diff`:
jq '.files[] | select(.has_diff) | {name: .name.head, patch: .totals.patch}' /tmp/codecov.json

# The missed patch lines in one file. `.coverage.head` is a LineType enum
# (0=hit, 1=miss, 2=partial), and `.added` keeps context lines inside a hunk
# from reading as patch misses:
jq '.files[] | select(.name.head == "<path>") | .lines[]
    | select(.is_diff and .added and .coverage.head == 1) | {line: .number.head, code: .value}' /tmp/codecov.json

# Whole-file line coverage at one commit. No trailing slash after the path —
# the route swallows it and answers 404 "coverage info not found":
curl -sL "$API/file_report/<path>?sha=<sha>"
```

Per-file `.totals.patch` (equivalently `.totals.head.diff`, `[files, lines, hits, misses, …]`) holds that file's patch numbers, and the posted percentage aggregates them over the files in the PR's own diff. The top-level `totals.base.diff` is a different quantity: the base's coverage of those lines. Commit messages arrive with raw newlines in them, so the JSON is strictly invalid — `jq` copes, Python needs `json.loads(…, strict=False)`.

**A compare listing files the PR never touched** means the merge-base has no report, so codecov walked back to the newest ancestor that does and diffed from there. The posted patch check still scopes to the PR's own diff; it's the API object that widens. Every main commit uploads from the `coverage` workflow, so this points at a failed or missing run on the base commit.

**`skim` fails with E0554 (`#![feature]` on stable):** the local `cargo-llvm-cov` predates 0.7.0, which stopped putting the coverage flags in global `RUSTFLAGS` and started instrumenting only workspace crates. Older versions leak `--cfg=coverage` into every dependency, and `skim` gates a nightly feature on it. Install the version the `code-coverage` job pins rather than working around it (`--no-cfg-coverage` also avoids it; `--no-rustc-wrapper` reinstates it).

**Moved and re-indented lines:** codecov counts every line the diff touches as part of the patch, including one the change only relocated — a `git mv`, or a body re-indented because it moved inside a new wrapper. Pre-existing uncovered lines then count against a patch that changed no behavior. Verify against `main` (under the old path, for a rename): if the lines are identical there, the misses predate the change, and the fix is to say so to the user rather than undo the move.

**"N functions have mismatched data" warning:** `cargo llvm-cov` merges profiles from multiple compilation targets with minor codegen differences (typically 5–20 functions). Expected, harmless, no suppression flag exists ([LLVM #97574](https://github.com/llvm/llvm-project/issues/97574)).

PTY tests need extra setup to be measured at all — they `env_clear()` the subprocess, so LLVM env vars must be passed through explicitly. See "Coverage in PTY Tests" below.

## Running `wt` Commands in Tests

**Use the correct helper to ensure test isolation.** Tests that spawn `wt` must
be isolated from the host environment to prevent:

- **Directive leakage**: Test commands writing to the user's shell directive file
- **Config pollution**: Tests reading/writing the user's real config
- **Git interference**: Host GIT_* environment variables affecting test behavior (the developer's *config* is denied a layer lower — see Git Config Isolation below)
- **Network access**: a fixture's `https://` remote URL becoming a real connect, with no timeout bounding it (`GIT_ALLOWED_PROTOCOLS` in `src/testing/mod.rs`)

### With a TestRepo fixture (most tests)

Use `repo.wt_command()` which returns a pre-configured Command:

```rust
// ✅ GOOD: Simple case
let output = repo.wt_command()
    .args(["switch", "--create", "feature"])
    .output()?;

// ✅ GOOD: With additional configuration (piped stdin, etc.)
let mut cmd = repo.wt_command();
cmd.args(["switch", "--create", "feature"])
    .stdin(Stdio::piped());
```

```rust
// ❌ BAD: Missing isolation - inherits host environment
let output = Command::new(wt_bin())
    .args(["switch", "--create", "feature"])
    .current_dir(repo.root_path())
    .output()?;
```

### Without a TestRepo (e.g., readme_sync tests)

Use the free function `wt_command()`:

```rust
use crate::common::wt_command;

// ✅ GOOD: Isolated from host environment
let output = wt_command()
    .args(["--help"])
    .current_dir(project_root)
    .output()?;
```

`wt_command()`'s default `current_dir` is a process-scoped empty tempdir
(outside any git repo, no project config) — so a bare `wt_command()` won't
pick up the test process's inherited CWD. Tests that need a specific CWD
(e.g., the worktrunk repo root for `readme_sync` help-text capture) must
call `.current_dir(...)` explicitly.

### Method reference

| Method | Returns | Use when |
|--------|---------|----------|
| `repo.wt_command()` | `Command` | Running wt commands with a TestRepo |
| `wt_command()` | `Command` | Running wt without a TestRepo (free function) |
| `repo.git_command()` | `Cmd` | Running git commands (use `.run()` not `.output()`) |

### Where a new environment variable goes

A test child's environment is four layers, each with one home in
`src/testing/mod.rs`: `STATIC_TEST_ENV_VARS` for a determinism knob every child
needs, `git_test_env` for git isolation (config, timestamps, the transport
deny), `PTY_TEST_ENV_VARS` for a knob only a terminal triggers, and
`pty_env_vars` for a path that varies per fixture. `configure_cli_command` and
`configure_pty_command` apply them by transport, so a variable added to the
right layer reaches every test that spawns `wt`. A per-builder copy reaches only
the tests that happen to use that builder, and the ones it misses fail later,
somewhere else.

**Name it `WORKTRUNK_TEST_*`.** `isolate_subprocess_env` scrubs the parent
environment by prefix — `GIT_*` and `WORKTRUNK_*` — so the prefix is what makes
a variable hermetic; an unprefixed name inherits from whatever shell ran the
suite. The rule covers the test-only protocol between the harness and the mock
playback, not just knobs that change wt's own behavior:
`WORKTRUNK_TEST_MOCK_CONFIG_DIR` is read only by the playback dispatch
(`testing::mock_stub`). A variable `wt` reads in production drops `TEST` and
keeps `WORKTRUNK_`.

## Git Config Isolation

**No `git` the suite runs reads the developer's `~/.gitconfig`**, whatever the test drives it through and wherever the fixture lives. The guarantee is one environment set, `shell_exec::HERMETIC_TEST_GIT_ENV` — the deny pair pointing `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` at a path that does not exist, plus the settings the suite needs in the denied config's place — applied to every child at its spawn site. There is no git-config file anywhere in the repo.

**Production spawn sites get the floor from a latch, not from the test.** Most test variables are set on a *child*: `git_test_env` on a git command, `configure_cli_command` on a `wt` subprocess. In-process git is not a child the test configures. `TestRepo` exposes a `repo` field of the production `Repository` type, and `Repository::run_command` builds a plain `Cmd::new("git")` — the test never holds the command, so there is no place to set env on it, and a test cannot set its own process environment instead (under `cargo test` the tests sharing the process run in parallel threads, and `std::env::set_var` beside a thread that spawns a process is the race that makes it `unsafe`). What a test *can* do safely is flip an atomic: the harness latches `shell_exec::enable_hermetic_test_env` in the fixture constructors and `configure_git_*`, and `Cmd` applies the floor to every child it spawns while the latch is set. Production code never latches it, so a user's `wt` inherits their real config; the latch is a test-serving switch in `shell_exec` accepted deliberately — see the `TODO(hermetic-env)` there for the structural alternative (threading an explicit env value through `Repository`).

The harness applies the same floor at the spawn sites it does own:

- `git_test_env` — reaching git through `configure_git_env` / `configure_git_cmd` (`TestRepo::git_command()`, `run_git()`, `run_git_in()`, `git_output()`, `commit_in()`) and a PTY child through `pty_env_vars` — adds the test identity, pinned dates, and locale; `configure_git_cmd` also applies the floor, since a plain `Command` child bypasses the `Cmd` latch.
- `isolate_subprocess_env` scrubs every inherited `GIT_*` from `wt` children — a host-exported `GIT_CONFIG_*` included — then re-applies the floor explicitly, so a subprocess denies host config just as this process does.
- a PTY child is `env_clear`ed and inherits nothing, so the floor is carried by hand at the PTY choke point every spawn routes through — `configure_pty_command` in `tests/common/mod.rs` — and again by `pty_env_vars`, whose vector declares a PTY `wt` child's complete environment. Each copy is pinned by its own test (`configure_pty_command_carries_the_git_config_floor`, `pty_env_vars_carry_the_git_config_floor`), because `useConfigOnly` fires only where an identity is missing — no PTY assertion would notice the floor going missing.

**The floor carries two settings.** `user.useConfigOnly` is a backstop: denial alone leaves git *guessing* an identity from the OS username and hostname rather than failing, which is the one way a hermetic suite could still author a commit as the developer. Nothing exercises it, because every path sets an identity, and that is the reason to keep it — without it a future gap goes silent. `rerere.enabled = false` is *set* rather than left unset because git enables rerere on its own whenever `$GIT_DIR/rr-cache` exists, so leaving it unset makes the suite's rerere state depend on what a fixture happens to carry. `commit.gpgsign` is gone, since denial already leaves git on its own default, and so are `advice.mergeConflict` / `advice.resolveConflict` — the snapshot layer strips gutter-prefixed `hint:` lines because they vary across git versions (the filter in `tests/common/mod.rs`), so nothing depends on quieting them at the source.

**A local run can measure a stale fixture.** The standard fixture is built once into `target/debug/wt-test-fixtures/standard-v<N>/` and copied per test, so whatever a git-config change alters *during fixture construction* survives in that copy until the cache is rebuilt. A floor change can therefore pass locally and fail on CI, which always builds fresh. Removing `rerere.enabled` did exactly that: the cached fixture already held an `rr-cache` directory from when the floor enabled rerere, git turns rerere on whenever that directory exists, and so every local test kept the behavior the change had just removed. Before trusting a local measurement of a git-config change, delete `target/debug/wt-test-fixtures/` or bump `STANDARD_FIXTURE_VERSION`.

Three things follow that are easy to get wrong:

- **`GIT_CONFIG_COUNT` is `-c`, so it outranks a repository's own config**, where a global *file* would yield to it. A key belongs in the floor only when no test needs to override it locally. `init.defaultBranch` is the one that doesn't qualify — `default_branch.rs` sets it in a repo to prove `wt` reads it — so every `git init` in the harness names its branch instead.
- **Identity is not in the floor.** A harness-built git gets it from `git_test_env`; an in-process git gets it from the fixture repo's local config, which every `TestRepo` constructor writes. The floor carries only the denial and the two `-c` settings; identity has those per-command homes already, and a copy in the floor could only drift from them, with `useConfigOnly` failing loudly if a path misses both.
- **Where fixtures live carries no isolation weight.** A conditional `includeIf "gitdir:<home>"` can't reach them wherever they sit, so `test_temp_root()`'s location is a question of ancestor-walk cost alone.

What the hole cost before this: any key in the developer's config applied to fixture repos, so `commit.gpgsign` failed their commits, `core.hooksPath` ran their hooks, and `core.fsmonitor` / `credential.helper` / `filter.*` ran programs of their choosing. A conditional `includeIf` made which of those happened depend on where `$TMPDIR` sat — the suite passed by accident of the temp dir being outside `$HOME`.

`cargo run -- <cmd>` is untouched: nothing in production latches the floor, so a developer's own invocations keep their aliases, credential helper, and identity.

`GIT_AUTHOR_DATE` / `GIT_COMMITTER_DATE` are **not** part of this floor: `git_test_env` pins them per command, so a commit made through `Repository::run_command` still gets the wall clock. Snapshot the author, not the date.

## Config Isolation for In-Process Unit Tests

`repo.wt_command()` / `wt_command()` isolate *subprocess* tests (above). An
in-process unit test that calls library functions directly gets no such
isolation: it runs in the test process, which inherits the real environment.

`Approvals::approve_commands` and the `UserConfig` mutation methods take an
explicit `&Path`, so a unit test passes a tempdir-backed path and the write stays
isolated. The global resolvers do not isolate: `Approvals::load()`,
`approvals_path()`, `config_path()`, and `system_config_path()` all fall back to
the real `~/.config/worktrunk/`.

<example>
<bad reason="Approvals::load() reads the real ~/.config/worktrunk/approvals.toml">

Bad:

```rust
let mut approvals = Approvals::load().unwrap();
approvals.approve_commands(project, vec![command], &approvals_path).unwrap();
```

</bad>
<good reason="Default state plus a tempdir path touches no real config">

Good:

```rust
let temp_dir = tempfile::tempdir().unwrap();
let approvals_path = temp_dir.path().join("approvals.toml");
let mut approvals = Approvals::default();
approvals.approve_commands(project, vec![command], &approvals_path).unwrap();
```

</good>
</example>

Git needs the same care, for a narrower reason. An in-process
`Repository::run_command()` spawns git with the test process's environment, so
none of `configure_git_env`'s per-command variables apply — no
`GIT_ALLOW_PROTOCOL`, no per-test `GIT_CONFIG_GLOBAL`. Host *config* is not
among the gaps: the latched floor above already denies it. The
repo's own config is the one layer such a command still reads, so every
`TestRepo` constructor appends `LOCAL_TEST_CONFIG` (identity
and the `protocol.allow` transport deny) to it. The identity is required rather
than convenient — the floor sets `user.useConfigOnly` and carries no name, so a
repo without one fails its commit instead of authoring from the host's username.
`TestRepo::assemble` is the single call site; a new constructor routes through it
and inherits the settings, and the bare fixtures (`BareRepoTest`,
`NestedBareRepoTest`) get them by composing over `TestRepo`.

`approvals_path()` and `config_path()` refuse the developer's real
`~/.config/worktrunk/` rather than resolving it. `config_path()` is the one that
matters most: `set_skip_shell_integration_prompt` and
`set_skip_commit_generation_prompt` reach it to **write**, so an unguarded
fall-through edits the config the developer is using. `approvals_path()` panics;
`config_path()` returns `None`, because its absent state is already meaningful —
`require_config_path()` turns it into an error, so a write still fails loudly
while a best-effort read like `prewarm_user_config` simply preloads nothing.

`#[cfg(test)]` makes both guards fire for `worktrunk` lib-crate tests only. A
bin-crate test (anything compiled into the `wt` binary — `src/commands/`,
`src/output/`, and the other `main.rs` modules) links the lib in non-test mode,
so the guard is compiled out there and a global read hits the real config
silently: it passes wherever `$HOME` is writable and fails only in a sandbox
that forbids it. Nothing exercises that today — no bin-crate test creates a
config under a scratch `$HOME` — so it's a live requirement on new tests, not a
known leak.

`system_config_path()` is deliberately unguarded: it resolves a machine-wide
file rather than the developer's own, and `config::deprecation`'s
`PendingDefault` rules need the lookup.

## Timing Tests: Polling and Absence Windows

**The assertion's polarity picks the tool.** A *presence* assertion waits for something that will happen (a file appears, a counter ticks): poll it, so the test returns the instant the event lands. An *absence* assertion proves something did NOT happen (a hook stays silent, a marker never appears): there's no event to wait for, so hold a fixed window with `SLEEP_FOR_ABSENCE_CHECK`, then assert. The most common timing flake is pairing a fixed sleep with a presence assertion: it guesses how long the work takes and fails when load overruns the guess.

### Presence: long timeouts, fast polling

Use long timeouts (5+ seconds) for reliability on slow CI, but poll frequently (10-50ms) so tests complete quickly when things work:
- **No flaky failures** on slow machines - generous timeout accommodates worst-case
- **Fast tests** on normal machines - frequent polling means no unnecessary waiting

```rust
// ✅ GOOD: Long timeout, fast polling
let timeout = Duration::from_secs(5);
let poll_interval = Duration::from_millis(10);
let start = Instant::now();
while start.elapsed() < timeout {
    if condition_met() { break; }
    thread::sleep(poll_interval);
}

// ❌ BAD: fixed sleep before a presence assertion (always slow, races under load)
thread::sleep(Duration::from_millis(500));
assert!(condition_met());

// ❌ BAD: Short timeout (flaky on slow CI)
let timeout = Duration::from_millis(100);
```

Use the helpers in `tests/common/mod.rs`:

```rust
use crate::common::{wait_for_file, wait_for_file_count, wait_for_file_content};

// ✅ Poll for file existence (60-second default timeout)
wait_for_file(&log_file);

// ✅ Poll for multiple files
wait_for_file_count(&log_dir, "log", 3);

// ✅ Poll for file with non-empty content
wait_for_file_content(&marker_file);
```

These use exponential backoff (10ms → 500ms cap) for fast initial checks that back off on slow CI. The 60-second default timeout is generous enough to avoid flakiness under CI load.

### Event-driven code: drive the scenario from the callback

When the system under test exposes a callback, channel, or event hook, drive the scenario **causally** through that hook instead of racing wall-clock timers. The callback gives you a happens-before edge into the loop — use it to inject inputs and terminate the run, so the test's timing depends on the event ordering, not on CPU scheduling.

```rust
// ✅ GOOD: causally driven — first Stall event injects a result; a Stall
// observed after the result drops tx to end the drain via Disconnected.
// Runs at threshold speed on any hardware; the 5s deadline is only a
// safety net.
let mut sender = Some(tx);
let mut saw_result = false;
let outcome = drain_results_with_timings(
    rx, /* ... */,
    Instant::now() + Duration::from_secs(5),
    StallTimings { threshold: ms(20), tick: ms(10) },
    |event| match event {
        DrainEvent::Stall { .. } if !saw_result => {
            sender.as_ref().unwrap().send(result).unwrap();
        }
        DrainEvent::Stall { .. } => { sender.take(); } // end drain
        DrainEvent::Result { .. } => { saw_result = true; }
        _ => {}
    },
);

// ❌ BAD: producer sleeps to land a result "partway through" a window
// whose size is itself a wall-clock deadline. Every extension of the
// deadline just makes the race wider, not correct.
std::thread::spawn(move || {
    std::thread::sleep(Duration::from_millis(80));
    tx.send(result).unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    drop(tx);
});
let outcome = drain_results_with_timings(
    rx, /* ... */,
    Instant::now() + Duration::from_millis(1000),
    /* ... */,
);
```

**Rule of thumb:** if your producer thread needs `thread::sleep` to line up with a deadline in the code under test, you're racing the scheduler. Reach for the callback, a channel, or a condvar instead. Fixed deadlines belong only in the safety-net role — "stop if something has truly hung" — not in the assertion path.

### Absence: hold a fixed window

When the assertion proves a negative (`assert!(!marker.exists())`), polling can't help, because there's no event to wait for. Hold a window long enough that the thing would have happened if it were going to, then assert it didn't. Use the shared constant (defined in `src/testing/mod.rs`, re-exported via `tests/common`) so absence sleeps stay greppable and self-documenting:

```rust
use crate::common::SLEEP_FOR_ABSENCE_CHECK;

thread::sleep(SLEEP_FOR_ABSENCE_CHECK); // 500ms, the floor for an absence window
assert!(!marker_file.exists(), "Command should NOT have run");
```

Two traps:

- **Give each half its own wait.** Sleeping once and then asserting both "X happened" and "Y didn't" makes the presence half flaky. Poll for X, then hold the window for Y.
- **Structural absence needs no window at all.** When the event is gated on a condition the test never sets up, it can't fire regardless of timing. Drop the sleep: poll the positive precondition and the absence holds by construction. A watchdog whose escalation is gated on `command.is_some()` can't escalate with no command, so the test polls for the first render and asserts `!escalated` with no window.

### Time-thresholded output: suppress it at the source

Output that appears only once an operation runs past a threshold is a function of machine load, not of behavior: the `Progress` and `Watchdog` spinners (`src/progress.rs`), `Cmd::delayed_stream`'s progress line, the picker's placeholder reveal. A PTY test captures the raw byte stream, so it keeps every in-place redraw frame a terminal would have erased, elapsed-second counter and all — frames that show up when the whole suite runs together and not when the test runs alone.

Each threshold has an env override pinning it, so the output is present or absent by construction rather than by timing. A new one goes in the baseline that matches its scope — `PTY_TEST_ENV_VARS` when only a terminal triggers it (`WORKTRUNK_TEST_SPINNERS=0`), `STATIC_TEST_ENV_VARS` when a pipe does too (`WORKTRUNK_TEST_DELAYED_STREAM_MS=-1`) — and reaches every PTY child from there, rather than being added per builder. Filtering the frames out of the capture afterwards is the weaker fix: the filter has to model cursor movement, and a block that redraws with a cursor-up spans lines a line-scoped filter can't follow.

## No Retries

Tests run once. Worktrunk configures no nextest `retries`, and no test re-runs its own failed body: a test that passes only on a second attempt is a bug report, and retrying it discards the report while leaving the bug. A green suite has to mean the code is green, not that the run's flakes stayed under a retry budget. Fix the flake at its root:

- A racy assertion is a timing bug. Make it deterministic, per Timing Tests above: poll for the event, or drive it causally through a callback.
- Resource pressure is a concurrency bug. Windows process creation intermittently fails with STATUS_DLL_INIT_FAILED (exit `-1073741502`) when many tests spawn git/wt children at once. Bound how many heavy tests run together; that removes the pressure instead of retrying past it.
- A kill at the 180s slow-timeout is a duration symptom with two causes, told apart by the durations around it: many stretched durations alongside high machine load is CPU starvation, usually a sibling worktree's concurrent build; one test pinned at the timeout in an otherwise-normal run is a blocked call inside it, usually network. Starvation's only lever here would be nextest `threads-required` bounds on the heavy PTY tests, deliberately unset while these stay rare one-offs: the bound taxes every healthy run, and can't see the sibling build that caused the starvation.
- A shared channel with more than one producer is an attribution bug. Counting events drained from a channel between steps only measures the step that produced them if nothing *else* can produce them: a background task's event landing after its step's drain is charged to the next step, so the assertion that fails names the wrong step and the wrong cause. The events are usually indistinguishable (skim's `Event::RunPreview` carries no payload), so identity assertions aren't available — quiesce the other producers instead, before the step sequence arms whatever they'd match. `on_update_pokes_run_preview_only_when_the_visible_pane_changes` is the worked example: `PreviewOrchestrator::wait_for_idle()` before each `note_awaiting`, so the skeleton precompute lands while no key matches it.
- A spawn that fails `NotFound` is a concurrent-build bug. Cargo uplifts `target/debug/wt` by removing the path and recreating it, so a second `cargo` against the same target directory leaves the binary absent for a fraction of a millisecond per rebuild, failing whatever spawn is in flight with `Os { code: 2, kind: NotFound }` — anywhere: an `insta_cmd` snapshot, a PTY wrapper's `wt config shell init`, a hook whose marker file then never appears — and passing on re-run, which is the signature. `wt_bin()` closes the window by spawning a hardlink pinned under `target/debug/wt-test-bin/` instead of the uplifted path (`testing::pin_test_binary`), so route every `wt` spawn through it or a helper that does; `test_wt_spawns_are_pinned` makes a direct `CARGO_BIN_EXE_wt` spawn fail the suite.
- A shared namespace is a collision bug. **Never `NamedTempFile::new()`** for a file a test needs by name: `tempfile` retries a name collision only when it surfaces as `AlreadyExists`, and on Windows `create_new` against a name already held by a *directory* — or by a file in delete-pending state — comes back `PermissionDenied`, which it hands straight back to the caller. A full suite run leaves the temp directory full of `.tmpXXXXXX` entries (every `TestRepo` makes one), and under that load the call fails ~1% of the time with `Access is denied.` — a panic that has nothing to do with what the test asserts. Take a `TempDir` and give the files fixed names inside it (`worktrunk::testing::directive_file` is the pattern); a directory collision surfaces as `AlreadyExists`, which tempfile retries.

A bounded poll that rides out one identified `ErrorKind` whose window is understood is itself a root fix, and the doctrine leaves it alone: `pin_test_binary` polls `NotFound` across cargo's uplift window, and `forward_with_etxtbsy_retry` (`src/completion.rs`) polls `ExecutableFileBusy` while a concurrently-forked child holds the just-written script's write fd open. Both fail immediately on any other error, which is what keeps a poll from drifting into a retry.

**Reproducing a Windows-only flake** means reproducing its *neighbours*, not starving it: run the suspect tests in a loop on a Windows runner with a full `cargo nextest run` going alongside. Pinning the CPU instead models the wrong thing whenever the failing run's own timing was normal (compare its duration against the same test passing — nextest prints it, and the `test` job uploads `junit.xml` with every test's time). Measured both ways on the same five tests: 80 iterations under a concurrent full suite reproduced a 1-in-100 Windows failure that no local run had ever shown, while pinning all four cores with busy loops instead just pushed nearly every iteration past the 30s waits — artificial failures that say nothing about the flake.

## Testing with --execute Commands

Use `--yes` to skip interactive prompts in tests. Don't pipe input to stdin.

## Feature Flags, Not Runtime Skipping

**Never skip tests based on runtime availability checks.** Use Cargo feature flags instead.

```rust
// ❌ BAD: Runtime skip - test silently passes when tool unavailable
#[test]
fn test_fish_integration() {
    if !shell_available("fish") {
        eprintln!("Skipping: fish not available");
        return;
    }
    // test code...
}

// ✅ GOOD: Feature flag - test excluded from compilation
#[cfg(feature = "shell-integration-tests")]
#[test]
fn test_fish_integration() {
    // test code...
}
```

**Why:**
- Runtime skips hide missing test coverage in CI logs
- Feature flags make dependencies explicit in `Cargo.toml`
- `cargo test` output clearly shows which tests ran vs were compiled out
- CI can enable features when dependencies are installed

**Existing feature flags:**
- `shell-integration-tests` — Tests that drive real shells over a PTY: bash,
  zsh, fish, nushell, and pwsh, plus the `jq` the Claude hook commands in
  `plugins/worktrunk/hooks/` pipe their payload through

## PTY Tests and README Examples

Use `insta_cmd` by default. It is faster and lets a test assert stdout, stderr,
and exit status separately. Use a PTY only when the contract actually depends
on a terminal: interactive prompts, shell functions and directives, pager
selection, or the temporal interleaving of stdout and stderr. A README label by
itself is not a reason to use a PTY.

For README output where interleaving matters, use `build_pty_command` with
`exec_cmd_in_pty` (or `exec_cmd_in_pty_prompted` for prompt-driven input) from
`tests/common/pty.rs`. These return the combined stream in terminal order. The
shell-wrapper-specific equivalent is `exec_in_pty_interactive` in
`shell_wrapper.rs`.

PTY tests are conformance tests for the shell boundary, not another place to
retest every command. Keep one representative workflow per distinct shell
implementation, then test command semantics through ordinary integration
tests. Add a command × shell case only when that interaction is itself the
contract.

## Coverage in PTY Tests

`configure_pty_command` clears the child's environment, so an instrumented
binary would lose the LLVM vars that tell it where to write coverage data. It
passes them back through, which is one more reason every PTY test starts there:

```rust
crate::common::configure_pty_command(&mut cmd);
// ... test-specific env (USER, SHELL, ZDOTDIR, the fixture's paths) ...
```

## No Global State Mutations in Tests

**Never mutate process-global state in tests.** Rust's test runner executes tests in parallel within the same process, so global mutations leak across tests and cause non-deterministic behavior.

Forbidden patterns:
- `log::set_max_level()` — affects all concurrent and subsequent tests
- `std::env::set_var()` — process-wide, races with other tests
- Setting global `static` variables without synchronization

If coverage tools flag uncovered `log::debug!()` format args, accept the gap — it's not meaningful coverage and not worth global side effects.

```rust
// ❌ BAD: Global mutation leaks across parallel tests
#[test]
fn test_something() {
    log::set_max_level(log::LevelFilter::Debug);
    // ...
}

// ❌ BAD: Environment variable race condition
#[test]
fn test_config_loading() {
    std::env::set_var("MY_CONFIG", "test_value");
    // ...
}
```

For environment-dependent tests, use `Command::new()` with `.env()` to set variables in a subprocess, or use the test isolation helpers (`repo.wt_command()`, `wt_command()`).

## Snapshot Filters

### Bold codes around redacted paths

Source code may wrap a path in `<bold>` for terminal styling (e.g., `cformat!("{label} @ <bold>{path}</> failed")`). Setup-side path filters in `tests/common/mod.rs` substitute the path to a placeholder like `[TEST_CONFIG]` or `[PROJECT_ID]`, and a follow-up filter strips ANSI codes immediately wrapping those placeholders so the snapshot reads as a clean `[PLACEHOLDER]`.

The strip filter only fires on placeholders established **before** it. It runs at the end of `setup_snapshot_settings*`, so any path-redaction filter the test adds *after* setup escapes it.

If a test introduces its own placeholder for a path (e.g., `_REPO_/system-config.toml` → `[TEST_SYSTEM_CONFIG_FILE]`), use `add_path_placeholder_filter` so the filter consumes any styling wrappers around the path:

```rust
// ✅ GOOD: helper wraps the pattern with optional ANSI consumption
common::add_path_placeholder_filter(
    &mut settings,
    r"_REPO_/system-config\.toml",
    "[TEST_SYSTEM_CONFIG_FILE]",
);

// ❌ BAD: bare add_filter substitutes only the path, so a `<bold>{path}</>`
// source leaves `\x1b[1m[TEST_SYSTEM_CONFIG_FILE]\x1b[22m` in the snapshot.
settings.add_filter(r"_REPO_/system-config\.toml", "[TEST_SYSTEM_CONFIG_FILE]");
```

The helper wraps the pattern in `(?:\x1b\[\d+m)*` brackets, which eat only the bold open/close immediately adjacent to the path — surrounding color spans (yellow warning, etc.) are preserved.

Setup-side path-redaction placeholders in the strip list (`add_placeholder_ansi_strip_filter` in `tests/common/mod.rs`): `[TEST_CONFIG]`, `[TEST_CONFIG_NEW]`, `[TEST_APPROVALS]`, `[PROJECT_ID]`, `[TEMP_HOME]`, `[TEMP]`. Placeholders that hold a real value (`[VERSION]`, `[HASH]`, `[BUILD_MODE]`, `[BINARY_PATH]`) keep their bold codes so the snapshot still asserts the user-visible styling. The strip pass is invoked at the end of every `setup_*_snapshot_settings` helper, so the contract holds uniformly across `setup_snapshot_settings*`, `setup_home_snapshot_settings`, and `setup_temp_snapshot_settings`.

## Test Style

### Choose the cheapest boundary

Put a belief at the lowest layer that can prove it:

| Belief | Test boundary |
|--------|---------------|
| Parsing, formatting, allocation, state transitions | Direct unit/module test |
| Git/filesystem/process wiring | Integration test with the smallest fitting fixture |
| TTY behavior, shell syntax, stream interleaving | PTY or real-shell test |

Exercise boundary values and input matrices exhaustively at the direct layer.
Keep one representative integration case to prove the pieces are wired
together. Do not repeat the same matrix end-to-end: a command × shell × width
cross-product usually measures fixture and process setup, not another product
behavior.

Equivalent coverage or identical snapshots are useful duplication signals, not
automatic deletion rules. Before removing a test, identify the belief it owns
and make sure another test proves that belief at an equal or better boundary.

A test's setup is part of its proof. For topology, absence, and error paths,
assert the precondition named by the test; fixture names and comments are not
evidence. If an earlier guard can produce the observed result, the test proves
that guard rather than the intended condition.

Configuring a subprocess mock is likewise not evidence that production used
that route. When route selection is the belief, assert the recorded call with
`mock_calls` and make unexpected calls fail (for example, through `_default`);
otherwise a fallback or blank response can satisfy the oracle.

Assert semantics through state, structured values, and exit status; snapshot
the pragmatic user experience when the complete rendering is the contract. A
custom verifier must fail for every violation it claims to check—diagnostic
`println!` output is not an oracle.

### Guards that scan source text

Several guards read `src/`, or the snapshot corpus, as text rather than
compiling it: no stray `println!` outside the allowlist, no `eprintln!` that
resolves to std's macro instead of anstream's, no bare `env!("VERGEN_…")`, no
`include_str!` of a path the package won't ship, no host path in a `.snap`, and
nothing but `src/testing/mod.rs` spawning `wt` through `CARGO_BIN_EXE_wt`.
Each one asserts *absence* over what its walk handed it.

That polarity is what makes the failure mode silent. A file that drops out of
the walk is indistinguishable from a file with nothing wrong in it: the
violation it carried is never looked for, `violations` stays empty, and the
test passes green over less than it claims.

So that walk lives once, in `tests/common/source_scan.rs`, and panics on every
read rather than skipping; its module docstring carries the rest. Other tests
recurse through directories for their own reasons, and this is about the ones
whose assertion is absence. A read that returns early is the shape to look
for. `Err(_) => return` and `let Ok(..) else { return }` are the obvious two,
and `entries.flatten()` is the one that hides, because it drops a per-entry
`io::Error` without looking like a `return` at all.

Each guard also proves its walk reached something, since a walk that reads
cleanly and yields nothing passes just as green. `visit_files` returns the
number of files it visited and is `#[must_use]`, so forgetting to answer for
coverage is a compile error rather than a rule someone has to remember — which
is what the swallowed reads it replaced had going for them. A guard that
already asserts something an empty walk cannot satisfy discards the count with
`let _ =`, and that discard is the claim: its proof is the assertion below it,
and a count beside it would be a second mechanism for one guarantee.

A guard walking several roots counts per root. A layout change moves one root
and leaves the others, and an aggregate count survives that.

### Snapshot env drift: cosmetic vs. a leak

`insta_cmd` snapshots record the test's environment variables in an `env:`
block. New or reordered env lines split into two cases — check the *value*
before dismissing:

- **Cosmetic (accept silently):** value is identical on every machine — a
  deterministic literal (`"0"`, `C`) or an already-redacted placeholder
  (`[TEST_HOME]`).
- **A leak (must fix):** value is host/platform/run-specific — a temp path
  (`/var/folders/…`, `/tmp/…`), `$HOME`/`$USER`, a PID, a timestamp. It will
  diff spuriously when the snapshot is regenerated elsewhere. Redact it with
  `add_redaction(".env.VAR_NAME", "[VAR_NAME]")` in
  `add_standard_env_redactions` (bound by the `repo` rstest fixture). Note
  `add_filter` does **not** work on the `env:` block — it only substitutes on
  captured snapshot content; use a redaction.

Removed vars never appear: insta-cmd (≥0.7, hence the `insta-cmd = "0.7"` dep
floor) drops every `Command::env_remove` from the recorded block. `get_envs()`
yields `None` for a removal and `Some("")` for a deliberate set-to-empty, and
insta-cmd keeps only the latter — so a removed var leaves no trace, while a var
a test sets to `""` is recorded faithfully as `KEY: ""`. This matters because
`isolate_subprocess_env` removes whichever `GIT_*` / `WORKTRUNK_*` keys exist
in the *parent* environment (plus `NO_COLOR` / `SHELL` / `PSModulePath`), so
which removals happen depends on the host (CI has `GIT_EDITOR`; a contributor's
box might have `GIT_PAGER`, neither, or both). Dropping removals at the source
means regenerating on any machine produces the same block — you don't have to
match CI's `GIT_*` environment.

A var a test affirmatively sets to `""` as its subject does show up:
`test_list_config_env_override_validation_failure` sets
`WORKTRUNK_WORKTREE_PATH=""` to trigger the validation warning, and the block
records it as `WORKTRUNK_WORKTREE_PATH: ""`.

The `args:` block has the same property: a repo path passed as a CLI argument
(`wt -C <root>`) is covered by the `.args[]` redaction in
`add_repo_and_worktree_path_filters`, which rewrites it to `_REPO_…` like the
body filters; any other run-specific argument needs its own redaction.

Path leaks fail `test_no_host_specific_paths_in_snapshots`
(`snapshot_formatting_guard.rs`), which scans every committed `.snap` for
host-specific path markers (other run-specific values — PIDs, timestamps —
still need review-time vigilance). The test exists because insta never *compares* the
`info:` block — a missing redaction passes on the machine that generated the
snapshot and only churns when regenerated elsewhere.

A runner caveat: the `repo` fixture leaks its settings binding (rstest has no
teardown), so under libtest's reused threads a test that binds no settings of
its own — or clones them via `Settings::clone_current()` without
`add_standard_env_redactions` — can still appear redacted. nextest (process
per test) is authoritative — regenerate snapshots with
`cargo insta test --test-runner nextest` when in doubt.

### Inline snapshots over multi-assert

When a test checks formatted output, use `insta::assert_snapshot!` with an
inline snapshot instead of multiple `assert!(x.contains(...))` calls. Snapshots
capture the complete output, so a single snapshot replaces many contains checks
and catches regressions that spot-checks miss.

```rust
use insta::assert_snapshot;

// ✅ GOOD: One snapshot captures all formatting
assert_snapshot!(format_message("hello"), @"  │ hello");

// ❌ BAD: Spot-checks that miss structural regressions
assert!(result.contains("│"));
assert!(result.contains("hello"));
assert!(!result.contains("error"));
```

Import `assert_snapshot` directly (`use insta::assert_snapshot;`) rather than
using the qualified `insta::assert_snapshot!` form.

For first-time snapshot creation, leave the inline value empty (`@""`), then
run `cargo insta test --accept` to fill it.

To update existing file-based snapshots (e.g., after editing CLI help text),
use `cargo insta test --accept`:

```bash
cargo insta test --accept --test integration -- test_help
```

Do not manually edit `.snap` files — they contain ANSI escape sequences that
are difficult to reproduce by hand.

### One test per belief

Group related inputs into a single test when they verify the same belief about
the code. A test named `test_wrap_text_at_width` that exercises short text, long
text, single words, and edge cases is better than five separate test functions
testing each input individually.

Use the minimum sufficient contrasts: one anchor case, then one case that
changes only the factor needed for each additional claim. A new test earns a
separate function when it has a distinct setup, oracle, or failure diagnosis—
not merely another input label or production branch.

```rust
// ✅ GOOD: One test for the belief "wrapping respects word boundaries"
#[test]
fn test_wrap_text_at_width() {
    assert_eq!(wrap_text_at_width("short text", 20), vec!["short text"]);
    assert_eq!(wrap_text_at_width("hello world foo bar", 10), vec!["hello", "world foo", "bar"]);
    assert_eq!(wrap_text_at_width("superlongword", 5), vec!["superlongword"]);
    assert_eq!(wrap_text_at_width("", 20), vec![""]);
}
```

Table-driven tests work well for functions that map inputs to expected outputs:

```rust
#[test]
fn test_bash_token_styles() {
    let cases = [
        ("function", AnsiColor::Blue),
        ("keyword", AnsiColor::Magenta),
        ("string", AnsiColor::Green),
    ];
    for (name, expected) in cases {
        let style = bash_token_style(name).expect(name);
        assert_eq!(style.get_fg_color(), Some(Color::Ansi(expected)), "{name}");
    }
}
```

### Don't test constructors or dependencies

Tests that verify `Style::new().bold()` produces a bold style, or that
`StyledString::raw("x")` stores `"x"`, are testing the dependency — not our
code. Delete these. Test the behavior that uses these types instead.

## Deterministic Time in Tests

Tests use `TEST_EPOCH` (2025-01-02) for reproducible timestamps. The constant is defined in `src/testing/mod.rs`, re-exported via `tests/common/mod.rs`, and automatically set as `WORKTRUNK_TEST_EPOCH` in the test environment.

**For test data with timestamps** (cache entries, etc.), use the constant:

```rust
use crate::common::TEST_EPOCH;

repo.run_git(&[
    "config", "worktrunk.state.feature.ci-status",
    &format!(r#"{{"checked_at":{TEST_EPOCH},"head":"abc123"}}"#),
]);
```

**For production code** that needs timestamps, use `worktrunk::utils::epoch_now()` which respects `WORKTRUNK_TEST_EPOCH`. Using `SystemTime::now()` directly causes flaky tests.
