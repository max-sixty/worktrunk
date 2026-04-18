# Changelog

## 0.39.0

### Improved

- **Aliases dispatch from top-level `wt <name>`** (and graduate from experimental): Configured aliases now resolve as first-class commands — `wt deploy` works the same as `wt step deploy`, reading better as an everyday shortcut. Precedence is built-in → alias → `wt-<name>` PATH binary → unrecognized-subcommand error, matching git's model where `[alias]` entries shadow `git-foo` externals. The old "alias shadows a built-in" warning is gone; an alias named `commit` now simply runs via `wt commit` (only `wt step commit` remains shadowed). ([#2266](https://github.com/max-sixty/worktrunk/pull/2266))

- **`wt switch --base` accepts `pr:N` / `mr:N`**: `--base` now routes through the same resolver as the positional branch argument, so `wt switch -c feat-x --base pr:42` works symmetrically with `wt switch pr:42`. Same-repo PRs/MRs resolve to the source branch name; fork PRs/MRs fetch `refs/pull/N/head` (GitHub) or `refs/merge-requests/N/head` (GitLab) and use the resolved SHA, avoiding fork-branch pollution in the local namespace. ([#2263](https://github.com/max-sixty/worktrunk/pull/2263), thanks @jrdncstr for the request in [#2261](https://github.com/max-sixty/worktrunk/issues/2261))

- **`WORKTRUNK_PROJECT_CONFIG_PATH` env override**: Mirrors the existing `WORKTRUNK_CONFIG_PATH` (user) and `WORKTRUNK_SYSTEM_CONFIG_PATH` (system) overrides for the project config. Missing files at the overridden path resolve to no project config, same as a missing `.config/wt.toml`. `wt config show --format=json` now reports the overridden path in the `project.path` field. ([#2267](https://github.com/max-sixty/worktrunk/pull/2267))

### Fixed

- **`wt list` handles `[gone]` upstreams gracefully**: When a branch's configured upstream ref is gone (remote branch deleted, local tracking ref pruned), the Remote column was surfacing a raw `fatal: ambiguous argument 'origin/<branch>'` error from `git rev-parse`. Upstream resolution now reads `%(upstream:track)` and treats `[gone]` the same as no upstream, so the row renders cleanly. ([#2262](https://github.com/max-sixty/worktrunk/pull/2262))

- **Nix flake build with vendored skim-tuikit**: The `[patch.crates-io]` path dependency on `vendor/skim-tuikit` was being stubbed out by crane's `mkDummySrc` during `buildDepsOnly`, breaking downstream skim resolution. The flake now preserves real sources for vendored path deps while still benefiting from dependency caching. ([#2265](https://github.com/max-sixty/worktrunk/pull/2265), thanks @nickdichev)

### Documentation

- **Renamed "external subcommand" to "custom subcommand"**: The user-facing name for `wt-<name>` PATH-dispatched subcommands is now "custom subcommand" in docs and internal code, matching cargo's vocabulary. Avoids overloading "external," which the codebase already uses for `shell_exec` subprocesses. Internal renames: `src/commands/external.rs` → `custom.rs`, `Commands::External` → `Commands::Custom`. ([#2270](https://github.com/max-sixty/worktrunk/pull/2270))

- **Trimmed filler in prose docs**: Removed sentences that restated obvious error behavior, duplicated nearby prose, or added visual weight without information across `extending.md`, `faq.md`, and `tips-patterns.md`. ([#2271](https://github.com/max-sixty/worktrunk/pull/2271), [#2272](https://github.com/max-sixty/worktrunk/pull/2272))

### Internal

- Extracted a shared `did_you_mean` helper used by both top-level and `wt step` suggestion sites, so the 0.7 Jaro-Winkler threshold and sort order are defined in one place. ([#2268](https://github.com/max-sixty/worktrunk/pull/2268))

## 0.38.0

### Improved

- **Concurrent execution in `pre-*` pipeline hooks**: Pipeline blocks (`[[pre-start]]`, `[[pre-merge]]`, etc.) now run their concurrent commands in parallel for foreground hooks, matching the existing behavior in post-* hooks and aliases. The deprecated single-table form (`[pre-start]`) remains serial. ([#2249](https://github.com/max-sixty/worktrunk/pull/2249))

- **`cli` feature unbundles CLI-only deps for library consumers**: The `worktrunk` crate is also consumed as a library (e.g. by [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync)). Everything reachable from `src/lib.rs` previously pulled in `clap`, `clap_complete`, `skim`, `crossterm`, `termimad`, `env_logger`, and `humantime` transitively. A new `cli` feature (on by default) gates these; library consumers with `default-features = false` drop from 195 to 126 transitive crates. ([#2238](https://github.com/max-sixty/worktrunk/pull/2238))

- **Faster `wt list` and `wt switch` on warm caches**:

    - In-memory caches for remote URLs, commit details, and diff stats in `RepoCache` eliminate ~11 duplicate git subprocesses per `wt switch`. ([#2252](https://github.com/max-sixty/worktrunk/pull/2252))

    - `list_local_branches()` primes ref/SHA caches from `for-each-ref` data already collected; `Branch::upstream()` uses a single batch `for-each-ref` call instead of N per-branch `rev-parse` commands. Reduces `rev-parse` calls from 53 to 27 on a typical-8 benchmark. ([#2255](https://github.com/max-sixty/worktrunk/pull/2255))

    - Share `git status --porcelain` output between `WorkingTreeDiffTask` and `WorkingTreeConflictsTask`, halving duplicate subprocesses. ([#2259](https://github.com/max-sixty/worktrunk/pull/2259))

- **Faster `wt statusline`**: `terminal_width()` no longer walks parent processes to find a TTY — the fallback is now behind a dedicated helper used only by `wt statusline` under Claude Code. Picker, `wt list`, and help callers skip the walker entirely. ([#2260](https://github.com/max-sixty/worktrunk/pull/2260))

- **`switch.picker.timeout-ms` deprecated**: After progressive rendering landed in 0.37.1, this config field was parsed but silently ignored. It's now flagged as deprecated with migration via `wt config update`. (Breaking for library consumers: `SwitchPickerConfig::timeout_ms` field and `timeout()` accessor removed.) ([#2236](https://github.com/max-sixty/worktrunk/pull/2236))

### Fixed

- **Picker panic on terminal resize**: `Term::on_resize` panicked with `attempt to subtract with overflow` when the terminal was smaller than the picker's preferred height (reachable under `script(1)` with stdin closed, or in small tmux panes). Vendored skim-tuikit now uses `saturating_sub`. ([#2233](https://github.com/max-sixty/worktrunk/pull/2233))

- **Picker previews no longer show stale "no commits ahead" text**: BranchDiff/UpstreamDiff tabs read async fields that sometimes hadn't landed at skeleton-time precompute, caching wrong "has no commits ahead" / "has no upstream tracking branch" text. Previews now derive only from skeleton-time fields plus direct git queries. ([#2245](https://github.com/max-sixty/worktrunk/pull/2245))

- **Picker preview height with `--branches`/`--remotes`**: The Down-layout item count estimate only counted worktrees, so with `--branches` or `--remotes` the estimate underflowed and the preview claimed space the list needed. ([#2247](https://github.com/max-sixty/worktrunk/pull/2247))

- **GitLab CI status in `wt list`**: `glab ci list` now runs with the correct working directory. ([#2244](https://github.com/max-sixty/worktrunk/pull/2244))

- **Picker Summary tab empty state**: aligns with the other preview tabs (bullet + branch header) instead of a dimmed sentence. ([#2246](https://github.com/max-sixty/worktrunk/pull/2246))

### Documentation

- Use concurrent form in multi-key hook examples, now that pre-* concurrent is supported. ([#2248](https://github.com/max-sixty/worktrunk/pull/2248))

- Catalog skim 4.x upgrade impact and stability assessment. ([#2239](https://github.com/max-sixty/worktrunk/pull/2239))

- Picker and collect module docstrings gain phase timing tables and trace instrumentation at key picker phases. ([#2250](https://github.com/max-sixty/worktrunk/pull/2250))

### Internal

- Simplified `wt-perf` output; `cache-check` JSON adds wasted-time fields, sorts duplicates by wasted time, and renames `total_extra_calls` to `extra_calls`. ([#2253](https://github.com/max-sixty/worktrunk/pull/2253), [#2254](https://github.com/max-sixty/worktrunk/pull/2254))

## 0.37.1

### Improved

- **Progressive rendering in `wt switch` picker**: The picker now mirrors `wt list`'s skeleton-first model — branch and path render immediately, while status, diff stats, counts, and summaries fill in in place as they resolve. Replaces the previous ~500ms blocking freeze before first render. ([#2231](https://github.com/max-sixty/worktrunk/pull/2231))

- **Clean rows no longer flash the timeout glyph in the picker**: The LLM semaphore is now acquired only around the actual LLM call, so the no-changes and cache-hit fast paths return immediately instead of sitting behind up to 8 concurrent LLM calls. A clean `main` row in the picker now renders blank rather than the `·` "timed out" placeholder. ([#2222](https://github.com/max-sixty/worktrunk/pull/2222))

### Fixed

- **Picker preview styling bleed**: `color_print`'s `</>` emits SGR 22 to reset `<bold>`/`<dim>`, which skim 0.20's ANSI parser silently drops. Preview spans now emit an explicit full reset (`\x1b[0m`), so dim and bold no longer bleed across the rest of the preview pane. ([#2232](https://github.com/max-sixty/worktrunk/pull/2232))

- **Picker alt-screen enter/exit asymmetry**: In partial-height mode (`height=90%`), skim-tuikit skipped `smcup` on startup but still emitted `rmcup` on exit, corrupting the outer terminal's scrollback. The vendored tuikit now pairs enter/exit symmetrically. ([#2230](https://github.com/max-sixty/worktrunk/pull/2230))

- **Partial first render under tmux**: Under tmux PTY pressure, rows past the first ~1024 bytes would silently vanish because `Output::flush` used `write` instead of `write_all`. Vendored skim-tuikit fixes the short-write bug. ([#2226](https://github.com/max-sixty/worktrunk/pull/2226))

### Library

- **Expose worktree removal API from the `worktrunk` library**: `remove_worktree_with_cleanup`, `RemoveOptions`, and `BranchDeletionMode` are now public, letting external tools reuse the canonical removal flow (fsmonitor cleanup, trash-path staging) instead of reimplementing it with raw git commands. Motivated by [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync). ([#2227](https://github.com/max-sixty/worktrunk/pull/2227), thanks @pablospe for the request in [#2053](https://github.com/max-sixty/worktrunk/issues/2053))

### Documentation

- **Document `worktrunk-sync`**: Linked from the Extending page and the FAQ as a community-maintained companion tool for rebasing stacked worktree branches. ([#2225](https://github.com/max-sixty/worktrunk/pull/2225))

- **Catalog vendored skim patches**: `vendor/skim-tuikit/PATCHES.md` now records both landed and candidate patches against skim-tuikit, and a Cargo.toml comment records why skim is pinned to 0.20.x. ([#2228](https://github.com/max-sixty/worktrunk/pull/2228), [#2229](https://github.com/max-sixty/worktrunk/pull/2229))

### Internal

- **Drop unreachable `rayon::spawn` fallback** in the picker orchestrator. ([#2216](https://github.com/max-sixty/worktrunk/pull/2216))

## 0.37.0

### Improved

- **Concurrent table form across hooks and aliases**: `post-*` hooks already ran table form concurrently; aliases in table form (`[[aliases.deploy]]`) now do too, with output prefixed by a colored `{label} │ ` and line-atomic writes. `pre-*` table form (`[[pre-merge]]`) is still forced serial but will follow in a future release — it's deprecated now so the serial→concurrent switch is explicit. Run `wt config update` to migrate to pipeline form. ([#2089](https://github.com/max-sixty/worktrunk/pull/2089), [#2135](https://github.com/max-sixty/worktrunk/pull/2135), [#2145](https://github.com/max-sixty/worktrunk/pull/2145), [#2151](https://github.com/max-sixty/worktrunk/pull/2151))

- **`--KEY=VALUE` shorthand for alias and hook variables**: `wt step deploy --env=staging` and `wt hook pre-start --branch=feature/test` now work without the `--var` prefix. `--my-var=value` becomes `{{ my_var }}` in templates. Hooks also accept **custom variable names** (previously a fixed list; now matches alias behavior) and warn when a `--var` isn't referenced by any template — catching typos like `--brnach=feature`. ([#2091](https://github.com/max-sixty/worktrunk/pull/2091), [#2096](https://github.com/max-sixty/worktrunk/pull/2096), [#2117](https://github.com/max-sixty/worktrunk/pull/2117))

- **`wt step` discovers configured aliases**: Running `wt step` (or `wt step --help`) now lists user and project aliases alongside the built-in subcommands, each with a one-line template summary. Aliases that shadow a built-in are flagged `(shadowed by built-in)`. ([#2131](https://github.com/max-sixty/worktrunk/pull/2131), [#2141](https://github.com/max-sixty/worktrunk/pull/2141))

- **Shell completions for external `wt-*` subcommands**: Tab completion now discovers `wt-*` binaries on PATH and forwards completion requests to them, so `wt sync --<tab>` shows the external command's flags. Builds on the git-style external subcommand dispatch in 0.36.0. ([#2074](https://github.com/max-sixty/worktrunk/pull/2074), thanks @pablospe)

- **Persistent on-disk cache for expensive git operations**: Five SHA-keyed probes that previously ran live on every `wt list` and `wt switch` — merge-tree conflict checks, the integration/add-probe, `is-ancestor`, `has-added-changes`, and branch diff stats — are now cached to disk under `.git/wt/cache/`. Because commit SHAs are content-addressed, cached results never go stale; an LRU bound (5000 entries per kind) keeps disk usage bounded. User-visible effects:

    - **`wt list` and the `wt switch` picker open much faster on big repos**, especially those with many stale branches. A warm cache skips the expensive probes entirely; a cold cache still benefits from the faster per-worktree check below.

    - **Dirty-worktree conflict check is ~10× faster on cold cache.** Swapped `git stash create` for `git write-tree` as the ephemeral tree snapshot — same answer, far less plumbing per worktree. ([#2119](https://github.com/max-sixty/worktrunk/pull/2119))

    - **The picker now shows the same status info as `wt list`.** The old "skip stale branches" shortcut hid conflict and ahead/behind info on branches 50+ commits behind main to keep the picker responsive. The cache makes the shortcut unnecessary, so stale branches now display full status.

    - **Consistent results during in-progress rebases.** Tasks now track the branch ref rather than the worktree's transient HEAD, so rows no longer contradict themselves mid-rebase (e.g., `is_ancestor=true` alongside `1 ahead / 1 behind`).

    Cache is cleared by `wt config state clear`. ([#2085](https://github.com/max-sixty/worktrunk/pull/2085), [#2090](https://github.com/max-sixty/worktrunk/pull/2090), [#2098](https://github.com/max-sixty/worktrunk/pull/2098), [#2119](https://github.com/max-sixty/worktrunk/pull/2119))

- **Lower-priority background operations**: Extends the CPU/IO priority throttling already used by `wt step copy-ignored` to the background `rm -rf` in `wt remove` and the trash sweep, so cleanup doesn't compete with foreground work. On macOS this now uses `taskpolicy -b`, which throttles disk I/O as well as CPU; Linux uses `nice -n 19` with best-effort `ionice`. User hooks are unchanged. ([#2130](https://github.com/max-sixty/worktrunk/pull/2130), [#2133](https://github.com/max-sixty/worktrunk/pull/2133))

- **Pipeline structure in alias announcements**: Aliases now announce their pipeline structure: `Running alias deploy: install; build, lint` rather than the bare alias name. ([#2092](https://github.com/max-sixty/worktrunk/pull/2092))

- **Graceful per-layer config degradation**: A bad env var or a broken user config file no longer wipes the entire config to defaults. Each layer (system, user, env vars) degrades independently — valid layers apply, invalid layers are skipped with a warning. ([#2120](https://github.com/max-sixty/worktrunk/pull/2120))

- **Per-variable env var type resolution**: When multiple `WORKTRUNK_*` env vars target fields of different types (e.g., a numeric and a string field), each is resolved independently against the file config. Previously one incompatible var would drop every env override and the file config. ([#2111](https://github.com/max-sixty/worktrunk/pull/2111))

- **Clearer deprecation warnings**: Structural deprecation warnings follow a consistent `{label}: X is deprecated in favor of Y` pattern with a single proposed-diff preview — no more redundant current-config dump. Template variable renames and the `approved-commands` removal use the same pattern. Every command (not just `wt config show`) now emits the same per-kind warnings, with a single dedup'd hint per process pointing to `wt config show` for details and `wt config update` to apply. Deprecation warnings are suppressed in non-diagnostic contexts (tab completion, picker, `wt list statusline`) to keep prompts quiet. ([#2147](https://github.com/max-sixty/worktrunk/pull/2147), [#2148](https://github.com/max-sixty/worktrunk/pull/2148), [#2153](https://github.com/max-sixty/worktrunk/pull/2153), [#2171](https://github.com/max-sixty/worktrunk/pull/2171))

- **Structured JSON for `wt config state logs`**: `--format` is now a global flag on `state logs`, `state hints`, `state ci-status`, and `state marker` — ordering no longer matters. Logs JSON entries gain first-class `branch`, `source`, `hook_type`, `name`, `size`, `modified_at`, and absolute `path` fields alongside the existing relative `file`, so filtering works with `jq` directly. (Breaking: the `--hook` and `--branch` filters on `wt config state logs get` were removed in favor of `jq`; piping the JSON through `jq 'select(.branch == "...")'` replaces them.) ([#2156](https://github.com/max-sixty/worktrunk/pull/2156), [#2161](https://github.com/max-sixty/worktrunk/pull/2161))

- **Cleaner log filenames**: Background hook log files skip the collision-avoidance hash suffix when the input is already a safe filename. `main/project/post-merge/clippy.log` instead of `main-vfz/project/post-merge/clippy-vif.log`. Names containing invalid path characters still get the hash. ([#2157](https://github.com/max-sixty/worktrunk/pull/2157))

- **`wt list` stall visibility**: When `wt list` hangs for 5+ seconds, the progressive footer now names the blocked task and worktree (e.g. `○ Showing 13 worktrees (253/254 loaded, no recent progress; waiting on ci-status for feat)`), with a pending count when multiple tasks are outstanding. On full timeout, the warning joins the blocked-tasks list onto a single gutter-prefixed line: `▲ wt list timed out after 120s (151 results received); blocked tasks: …`. ([#2203](https://github.com/max-sixty/worktrunk/pull/2203), [#2205](https://github.com/max-sixty/worktrunk/pull/2205), [#2207](https://github.com/max-sixty/worktrunk/pull/2207))

- **`-vv` logs full subprocess output to disk; drop `-vvv`**: Captured subprocess stdout/stderr now fan out to two log targets — a bounded preview on stderr mirrored to `.git/wt/logs/trace.log` (renamed from `verbose.log`), and the uncapped body to a new `.git/wt/logs/output.log`. Large captures (e.g. `git log -p | patch-id` during `wt list`) no longer flood stderr with elision markers and force a rerun — the full body is always on disk. Any `-v` count above 2 collapses to `-vv`. ([#2201](https://github.com/max-sixty/worktrunk/pull/2201))

- **Clap-native errors for unrecognized subcommands**: `wt s` and `wt step sqush` now show clap's formatted `error: unrecognized subcommand 'X'` with typo suggestions and Usage block, rather than a custom git-style single-line message. The `#[command(external_subcommand)]` path added in 0.36.0 for `wt-<name>` dispatch is preserved. ([#2212](https://github.com/max-sixty/worktrunk/pull/2212), [#2215](https://github.com/max-sixty/worktrunk/pull/2215))

- **Quieter `wt list` loading placeholders**: The `·` loading indicator no longer appears for commands that finish within 200ms — short renders never flash the dots. The Status column's loading/timeout glyph collapses from `⋯` to a dim `·`, and the working-tree gate's loading placeholder collapses from `···` to a single `·`, matching the visual weight of neighbouring gates. ([#2177](https://github.com/max-sixty/worktrunk/pull/2177), [#2181](https://github.com/max-sixty/worktrunk/pull/2181), [#2199](https://github.com/max-sixty/worktrunk/pull/2199))

- **Fewer `wt list statusline` subprocesses**: Statusline rendering dropped four duplicate git subprocesses per render (`rev-parse --git-common-dir` ×2, `--show-toplevel` ×3, `--git-dir` ×2) by adding a process-wide `rev-parse --git-common-dir` cache and canonicalizing input paths in `Repository::worktree_at()`. ([#2209](https://github.com/max-sixty/worktrunk/pull/2209))

- **Signals named in background pipeline errors**: Killed hook children now report which signal: `pipeline step terminated by signal 15 (SIGTERM): <step>` instead of the generic `command failed with signal`. ([#2193](https://github.com/max-sixty/worktrunk/pull/2193))

- **Nested config typos surface as warnings**: Mistyped keys nested inside a known table (e.g. `[merge] squas = true`) now produce `Unknown field merge.squas` rather than going unnoticed. Built on a unified top-level + nested unknown-key analysis that also powers on-save preservation. ([#2195](https://github.com/max-sixty/worktrunk/pull/2195))

- **`sanitize_hash` minijinja filter**: New template filter that wraps `sanitize_for_filename` — produces a filesystem-safe name with a 3-char hash suffix so distinct originals never collide, while already-safe inputs pass through unchanged. Useful for matching on-disk hook log filenames from `wt config state logs --format=json`. ([#2172](https://github.com/max-sixty/worktrunk/pull/2172))

### Fixed

- **`wt --help` and `wt --version` write to stdout**: Both previously wrote to stderr, breaking `version=$(wt --version)` and pipelines like `wt --help | grep …`. If you have scripts redirecting `2>&1` as a workaround, drop the redirect. Fixes [#2072](https://github.com/max-sixty/worktrunk/issues/2072). ([#2073](https://github.com/max-sixty/worktrunk/pull/2073), thanks @koralowiec for reporting; [#2155](https://github.com/max-sixty/worktrunk/pull/2155))

- **Directive file passes through `wt step` aliases**: Inside a `wt step <alias>` body, an inner `wt switch --create` now writes its `cd` directive to the parent shell instead of dropping it. This was the last blocker for "move staged changes into a new worktree" alias recipes. ([#2077](https://github.com/max-sixty/worktrunk/pull/2077))

- **Detect `AA` and `DD` unmerged status codes**: The working-tree conflict check caught 5 of 7 unmerged states but missed `AA` (both added) and `DD` (both deleted). Worktrees with these conflict types now fall back to the commit-based check as intended. ([#2124](https://github.com/max-sixty/worktrunk/pull/2124))

- **Squash detection no longer silently misses branches when `git diff-tree` fails**: The patch-id pipeline used for squash-merge detection didn't check whether `git diff-tree` exited cleanly — a failed source command fed `git patch-id` a truncated stream, producing a bogus patch-id and reporting "not squashed" when the branch may have been. Pipeline now bails on source-exit non-zero, and streams directly between commands via an OS pipe rather than buffering in-process. ([#2136](https://github.com/max-sixty/worktrunk/pull/2136))

- **Nushell multi-line `--execute` payloads**: The nushell wrapper was executing the exec directive file line-by-line, so multi-line payloads ran as separate shell sessions — `cd` and variable assignments didn't persist across lines. Now matches bash/zsh/fish `source` semantics. ([#2134](https://github.com/max-sixty/worktrunk/pull/2134))

- **Redundant "To configure" hint for outdated shell wrappers**: When a shell's integration file exists but is stale, `wt config show` no longer prints both a specific `wt config shell install <shell>` hint and the generic "To configure" summary. The summary now appears only when a shell is genuinely not configured. ([#2152](https://github.com/max-sixty/worktrunk/pull/2152))

- **Ctrl-C aborts `wt` command loops**: Signal-derived child exits (SIGINT/SIGTERM) now abort hook pipelines, alias steps, concurrent groups, and the `wt step for-each` worktree loop. Previously, wt's signal handler forwarded SIGINT/SIGTERM to the current child but wt itself survived, and `FailureStrategy::Warn` silently swallowed each interrupt — a single Ctrl-C against `wt merge` could charge through remaining hook steps. ([#2174](https://github.com/max-sixty/worktrunk/pull/2174), [#2182](https://github.com/max-sixty/worktrunk/pull/2182))

- **Nested unknown config keys preserved on save**: Any unknown key nested inside a known table (e.g. `future-option = true` under `[merge]`) was silently deleted on any config save triggered by other mutations (first-run prompt, interactive path customization). Preservation is now computed recursively, so unknown keys survive at every nesting level. ([#2180](https://github.com/max-sixty/worktrunk/pull/2180))

- **`wt step --help` honors `-C` and `--config`**: Help previously resolved aliases before applying global flags, so `wt -C other step --help` listed aliases from the process cwd and `--config custom.toml` was ignored. Globals are now parsed in a single early pass. ([#2176](https://github.com/max-sixty/worktrunk/pull/2176))

- **`wt step --help` no longer triggers config side effects**: Rendering the alias listing in help output no longer emits deprecation warnings to stderr or writes a migration file next to the user config. ([#2179](https://github.com/max-sixty/worktrunk/pull/2179))

- **`wt step <alias> --dry-run` with lazy vars**: Dry-run previously expanded every command eagerly, so pipelines that read `{{ vars.foo }}` set by an earlier step failed with an "undefined vars" error even when the non-dry-run command would succeed. Dry-run now mirrors the hook pattern: templates that reference `vars.*` are syntax-validated (catching typos like `{{ vars..foo }}`) and shown raw, while other templates expand eagerly. ([#2170](https://github.com/max-sixty/worktrunk/pull/2170))

- **`wt config show` fish completions and false-negative gating**: A missing fish completions file used to print a confusing nested hint under "Already configured shell extension" and flip the generic "To configure" summary. It now prints a warning with specific remediation, mirroring the "Outdated shell extension" pattern. The "report a false negative" link is no longer gated on `!has_any_configured`, so a detector miss in one shell still offers the link when other shells are detected. ([#2163](https://github.com/max-sixty/worktrunk/pull/2163))

- **Nix build meets Rust 1.93 MSRV**: `flake.lock` updated to ship a newer nixpkgs rustc. ([#2185](https://github.com/max-sixty/worktrunk/pull/2185), thanks @Lysanleo)

### Documentation

- **"Extending Worktrunk" page**: Dedicated docs page collecting recipes for custom workflows via hooks and aliases, including a "move staged changes to a new worktree" recipe closing [#938](https://github.com/max-sixty/worktrunk/issues/938). ([#2079](https://github.com/max-sixty/worktrunk/pull/2079), [#2083](https://github.com/max-sixty/worktrunk/pull/2083), [#2088](https://github.com/max-sixty/worktrunk/pull/2088), [#2094](https://github.com/max-sixty/worktrunk/pull/2094))

- **OpenCode in agent handoffs**: Skill documentation now covers OpenCode alongside other agent CLIs. ([#2108](https://github.com/max-sixty/worktrunk/pull/2108), thanks @vinicius507 for the suggestion in [#2076](https://github.com/max-sixty/worktrunk/issues/2076))

- **Hook pipeline documentation**: `wt hook --help` and web docs now teach pipelines as `[[hook]]` blocks with TOML notes in `config commands`, and the aliases docs teach `[[aliases.NAME]]` pipeline blocks. ([#2144](https://github.com/max-sixty/worktrunk/pull/2144), [#2149](https://github.com/max-sixty/worktrunk/pull/2149), [#2154](https://github.com/max-sixty/worktrunk/pull/2154))

- **FAQ updates**: Qualified the "no background processes" claim; clarified coverage includes shell-integration-tests; config key location uses `git config worktrunk.*`. ([#2080](https://github.com/max-sixty/worktrunk/pull/2080), [#2086](https://github.com/max-sixty/worktrunk/pull/2086), [#2126](https://github.com/max-sixty/worktrunk/pull/2126))

- **Troubleshooting: `wt list` fsmonitor hang**: Noted the interaction with core.fsmonitor daemons. ([#2194](https://github.com/max-sixty/worktrunk/pull/2194))

- **README installation command formatting**: Fixed code-block formatting around installation commands. ([#2187](https://github.com/max-sixty/worktrunk/pull/2187), thanks @MahmoudMabrok)

### Internal

- **Shell wrapper directive file split**: The shell integration now writes `cd` paths to a separate file from `--execute` shell payloads, with the `cd` path read literally (`cd -- "$(< file)"`, no shell parsing) and the exec file scrubbed from alias and hook child environments. Hardens against shell injection from hook/alias bodies into the parent session. The legacy single-file form is honored through 0.38; nushell users need `wt config shell install` to pick up the new wrapper. ([#2118](https://github.com/max-sixty/worktrunk/pull/2118))

- **MSRV bumped to Rust 1.93**: Per the "latest stable − 1" policy. ([#2125](https://github.com/max-sixty/worktrunk/pull/2125))

- **Centralized `[wt-trace]` emitter**: Trace records are now owned by `src/trace/emit.rs` rather than ad-hoc `log::debug!` format strings, and `-vv` log verbosity is fixed. ([#2146](https://github.com/max-sixty/worktrunk/pull/2146))

- **Unified hook and alias execution paths**: Hooks and aliases share the same foreground execution, shell invocation, template expansion, and priority-spawning code. ([#2089](https://github.com/max-sixty/worktrunk/pull/2089), [#2128](https://github.com/max-sixty/worktrunk/pull/2128), [#2140](https://github.com/max-sixty/worktrunk/pull/2140), [#2095](https://github.com/max-sixty/worktrunk/pull/2095), [#2113](https://github.com/max-sixty/worktrunk/pull/2113))

- **Config migration is now in-memory; no more `.new` files**: `wt config show` renders the deprecation diff from in-memory migrated content rather than writing a `.new` file next to the user's config. `wt config update` owns the sole filesystem mutation; a new `--print` flag emits migrated TOML to stdout without writing. ([#2184](https://github.com/max-sixty/worktrunk/pull/2184))

## 0.36.0

### Improved

- **Git-style external subcommands**: `wt foo` now runs `wt-foo` from PATH when `foo` is not a built-in, mirroring `git foo` → `git-foo`. Third-party tools can be installed and invoked as `wt <name>` without touching this repo. Unrecognized commands show a git-style error with typo suggestions. [Docs](https://worktrunk.dev/tips-patterns/#external-subcommands) ([#2054](https://github.com/max-sixty/worktrunk/pull/2054), thanks @pablospe for the suggestion in [#2053](https://github.com/max-sixty/worktrunk/issues/2053))

- **`{{ owner }}` template variable**: Expands to the GitHub/GitLab repository owner, useful for constructing URLs or paths in hook templates and `worktree-path`. ([#2051](https://github.com/max-sixty/worktrunk/pull/2051), thanks @greggdonovan)

- **Typed env-var config overrides**: `WORKTRUNK__LIST__TIMEOUT_MS=30` and other typed overrides now work correctly. Previously, string-typed env values silently failed deserialization, wiping all user config and falling back to defaults. ([#2062](https://github.com/max-sixty/worktrunk/pull/2062))

- **Config error attribution**: Config load errors now identify the source — file errors show TOML line/column pointers, env-var errors list the offending `WORKTRUNK_*` variable. Previously all failures showed a generic message. ([#2068](https://github.com/max-sixty/worktrunk/pull/2068))

- **Per-symbol atomic status rendering**: The Status column in `wt list` and the `wt switch` picker now renders each symbol independently — unresolved gates show `⋯` at their position instead of fabricating defaults when the collect deadline expires. ([#2067](https://github.com/max-sixty/worktrunk/pull/2067))

- **Hook error messages**: Malformed hook command config now lists the three accepted forms (string, named table, pipeline list) with a pointer to `wt hook --help`, instead of an opaque serde error. ([#2042](https://github.com/max-sixty/worktrunk/pull/2042))

- **Stale trash cleanup**: `wt remove` now sweeps orphaned `.git/wt/trash` entries older than 24 hours after each removal, reclaiming space from interrupted background removals. ([#2039](https://github.com/max-sixty/worktrunk/pull/2039))

### Changed

- **`wt hook <type>` exits successfully when no hooks are configured**: Previously errored; now prints a warning and exits 0, so scripts and CI can invoke `wt hook` unconditionally. ([#2056](https://github.com/max-sixty/worktrunk/pull/2056))

- **Hook output log layout**: Log files moved from flat `.git/wt/logs/{name}.log` to nested `{branch}/{source}/{hook-type}/{name}.log`. Per-branch listing/clearing is now O(that branch). `logs get --format=json` paths changed to relative. Legacy flat files are swept automatically. ([#2041](https://github.com/max-sixty/worktrunk/pull/2041))

### Fixed

- **`wt config show` false "Not configured"**: When the shell init line lives in a sourced file (common with dotfile managers), `config show` no longer reports "Not configured" — it checks whether integration is actually active at runtime. Fixes [#1306](https://github.com/max-sixty/worktrunk/issues/1306). ([#2066](https://github.com/max-sixty/worktrunk/pull/2066), thanks @wouter-intveld for reporting)

- **Remove-then-switch hint**: The hint for shadowed remote branches now uses `--foreground` so the chained `wt remove && wt switch` actually works (background removal left a placeholder directory blocking the switch). ([#2040](https://github.com/max-sixty/worktrunk/pull/2040))

- **Conflict detection unified**: The `wt switch` picker and `wt list` now both run both conflict probes (commit-level and working-tree). Previously the picker skipped the cheaper probe, leaving the fallback unreachable for clean worktrees; `wt list` non-full skipped the working-tree probe, missing conflicts from interrupted rebases. ([#2064](https://github.com/max-sixty/worktrunk/pull/2064))

### Documentation

- Surfaced vars & aliases on homepage and tips-patterns, cross-linked state keys to dedicated docs, tightened hook links. ([#2035](https://github.com/max-sixty/worktrunk/pull/2035), [#2036](https://github.com/max-sixty/worktrunk/pull/2036), [#2037](https://github.com/max-sixty/worktrunk/pull/2037), [#2038](https://github.com/max-sixty/worktrunk/pull/2038))

### Internal

- Subcommand ordering aligned to documented policies (pipeline order for step, CRUD for state actions). ([#2043](https://github.com/max-sixty/worktrunk/pull/2043), [#2044](https://github.com/max-sixty/worktrunk/pull/2044))

## 0.35.3

### Improved

- **`wt step prune` streams removals inline**: Removals and "Skipped" messages now print as each integration check completes, overlapping with still-running checks — previously there was a visible gap of silence while all parallel checks finished before any output appeared. ([#2015](https://github.com/max-sixty/worktrunk/pull/2015))

- **Fewer redundant `git worktree list` calls in prune and multi-remove**: `prepare_worktree_removal()` now accepts a pre-fetched worktree list, eliminating N+1 subprocess calls when removing many worktrees. ([#2025](https://github.com/max-sixty/worktrunk/pull/2025))

### Fixed

- **Picker preview UI lag**: The picker's preview cache now stores pager-rendered output, so cache hits skip the pager subprocess entirely. Previously, scrolling past an item with a large diff froze the UI briefly on every re-render because the pager ran on every call. ([#2021](https://github.com/max-sixty/worktrunk/pull/2021))

- **Template error hint underlining**: The "Available variables" hint in template expansion errors now underlines each variable name individually instead of wrapping the entire comma-separated list in a single underline span. ([#2028](https://github.com/max-sixty/worktrunk/pull/2028))

### Documentation

- **Cross-linked vars references**: The vars feature is documented in the hook template variables table, `wt config state vars` page, and tips-patterns recipes — these now link to each other so readers can navigate between "how to set" and "how to use in templates". ([#2034](https://github.com/max-sixty/worktrunk/pull/2034))

- **Clearer project config intro**: Improved the project config introduction and template variable heading in `wt config` help text. ([#2032](https://github.com/max-sixty/worktrunk/pull/2032))

## 0.35.2

### Improved

- **Multiple NAME filters for hook subcommands**: `wt hook pre-merge --yes insta doctest doc` runs a subset of hooks in one command, instead of chaining separate invocations. ([#2013](https://github.com/max-sixty/worktrunk/pull/2013))

- **Branch context in batch removal hooks**: During prune or multi-remove, hook announcement messages now include the branch name (`Running post-remove for **branch-name**: project:cleanup`), disambiguating which worktree triggered each hook. ([#2014](https://github.com/max-sixty/worktrunk/pull/2014))

### Fixed

- **Bare repo false positive when `core.bare` is unset**: Repos cloned by Eclipse/EGit (and other tools that don't write `core.bare`) were incorrectly detected as bare. Replaced `git rev-parse --is-bare-repository` with `git config --type=bool core.bare`. Fixes [#1939](https://github.com/max-sixty/worktrunk/issues/1939). ([#1976](https://github.com/max-sixty/worktrunk/pull/1976), thanks @daniel-iwan-datacore for reporting)

## 0.35.1

### Fixed

- **PR lookup on forks respects `gh repo set-default`**: `wt switch pr:N` now checks the gh-configured default repo when origin points to a fork, instead of always querying the fork's repo (which returns 404). The error message is also context-aware based on the configured default. Fixes [#2002](https://github.com/max-sixty/worktrunk/issues/2002). ([#2004](https://github.com/max-sixty/worktrunk/pull/2004), thanks @JustinPierce for reporting)

- **JSON output stability**: `config show --format=json` log file sort is now deterministic (filename tiebreaker for identical timestamps). `step for-each --format=json` includes a consistent `error` field on all failure variants. ([#2001](https://github.com/max-sixty/worktrunk/pull/2001))

### Internal

- Continued `TestRepo` consolidation: `bare()` constructor, `at(path)` constructor, removed lifetime guard field. ([#2000](https://github.com/max-sixty/worktrunk/pull/2000), [#2005](https://github.com/max-sixty/worktrunk/pull/2005), [#2007](https://github.com/max-sixty/worktrunk/pull/2007))

## 0.35.0

### Improved

- **`--no-verify` deprecated in favor of `--no-hooks`**: All commands (`switch`, `remove`, `merge`, `step commit`, `step squash`) now use `--no-hooks`. `--no-verify` remains as a hidden alias with a deprecation warning. ([#1932](https://github.com/max-sixty/worktrunk/pull/1932))

- **JSON output**: `--format=json` on `config show`, `config state` subcommands, `switch`, `remove`, `merge`, `step prune`, and `step for-each`. ([#1969](https://github.com/max-sixty/worktrunk/pull/1969), [#1959](https://github.com/max-sixty/worktrunk/pull/1959))

- **Per-command hook log files**: Each background hook command writes to its own log file instead of sharing a pipeline log. Combined hook announcements (e.g., post-remove + post-switch) display on a single status line. ([#1934](https://github.com/max-sixty/worktrunk/pull/1934), [#1980](https://github.com/max-sixty/worktrunk/pull/1980))

- **Prune and list performance**: `step prune` streams integration checks and removes candidates in parallel (~3x faster on repos with many branches). Multiple caching layers (integration target, `git_dir`, `rev_parse_tree`, `resolve_preferring_branch`) reduce redundant `git rev-parse` calls during `wt list`. ([#1950](https://github.com/max-sixty/worktrunk/pull/1950), [#1957](https://github.com/max-sixty/worktrunk/pull/1957), [#1966](https://github.com/max-sixty/worktrunk/pull/1966), [#1948](https://github.com/max-sixty/worktrunk/pull/1948), [#1943](https://github.com/max-sixty/worktrunk/pull/1943))

- **Itemized `state clear` output**: `wt config state clear` shows per-category counts and cleans up stale trash from incomplete worktree removals. ([#1961](https://github.com/max-sixty/worktrunk/pull/1961), [#1960](https://github.com/max-sixty/worktrunk/pull/1960))

- **Hook pipeline summary**: Serial steps separated by `;` instead of `→`, repeated unnamed sources collapsed into counted form (`user ×2`), and named steps show `source:name` prefix. ([#1994](https://github.com/max-sixty/worktrunk/pull/1994))

- **Copy-pasteable help text**: `--help` output strips `$ ` prompts from code examples for direct copy-paste in the terminal. ([#1992](https://github.com/max-sixty/worktrunk/pull/1992))

- **Better PR lookup errors**: `wt switch pr:N` 404 errors now include the repository name and suggest `gh repo set-default` for fork workflows. Fixes [#1925](https://github.com/max-sixty/worktrunk/issues/1925). ([#1927](https://github.com/max-sixty/worktrunk/pull/1927), thanks @JustinPierce for reporting)

- **Claude Code worktree hooks**: WorktreeCreate and WorktreeRemove hooks for the Claude Code plugin. ([#1959](https://github.com/max-sixty/worktrunk/pull/1959))

### Fixed

- **File permissions lost on copy-ignored**: `wt step copy-ignored` now preserves execute bits when copying files via reflink. Fixes [#1936](https://github.com/max-sixty/worktrunk/issues/1936). ([#1937](https://github.com/max-sixty/worktrunk/pull/1937), thanks @RileyMathews for reporting)

- **Git alias breaks `wt`**: Relative `GIT_DIR`/`GIT_WORK_TREE` paths inherited from git aliases now normalized to absolute paths at startup. Fixes [#1914](https://github.com/max-sixty/worktrunk/issues/1914). ([#1915](https://github.com/max-sixty/worktrunk/pull/1915), thanks @yasuhiroki for reporting)

- **Diagnostic files in state logs**: `verbose.log` and `diagnostic.md` now properly categorized in `wt config state logs` output. ([#1981](https://github.com/max-sixty/worktrunk/pull/1981))

- **Integration target in removal display**: Background removal now shows `origin/main` (effective target) instead of `main` when the remote is ahead. ([#1993](https://github.com/max-sixty/worktrunk/pull/1993))

- **Worktree-path hint suppression**: The "customize worktree locations" hint no longer appears when project-specific `worktree-path` is configured. ([#1941](https://github.com/max-sixty/worktrunk/pull/1941))

- **State logs formatting**: Missing newline between log sections in `wt config state logs` output. ([#1968](https://github.com/max-sixty/worktrunk/pull/1968))

- **Claude Code WorktreeCreate hook**: Fixed jq filter using wrong input field. ([#1964](https://github.com/max-sixty/worktrunk/pull/1964))

- **OpenCode unicode escaping**: Fixed broken emoji markers depending on Bun version. ([#1935](https://github.com/max-sixty/worktrunk/pull/1935), thanks @noirbizarre)

### Documentation

- Clarified plugin install command. ([#1906](https://github.com/max-sixty/worktrunk/pull/1906), thanks @suyua9)
- Fixed inaccurate logs documentation. ([#1986](https://github.com/max-sixty/worktrunk/pull/1986))

### Internal

- Consolidated `TestRepo` into single `src/testing` module, shared across lib and bin unit tests. ([#1944](https://github.com/max-sixty/worktrunk/pull/1944), [#1963](https://github.com/max-sixty/worktrunk/pull/1963), [#1971](https://github.com/max-sixty/worktrunk/pull/1971), [#1991](https://github.com/max-sixty/worktrunk/pull/1991))
- Simplified dispatch, timeout, and copy pool internals. ([#1949](https://github.com/max-sixty/worktrunk/pull/1949), [#1930](https://github.com/max-sixty/worktrunk/pull/1930), [#1931](https://github.com/max-sixty/worktrunk/pull/1931))

## 0.34.2

### Improved

- **OpenCode integration**: Activity tracking plugin shows agent status (`🤖` working, `💬` waiting) in `wt list`, with `wt config plugins opencode install/uninstall` for management. Also adds OpenCode as an LLM commit generation backend. ([#1807](https://github.com/max-sixty/worktrunk/pull/1807), thanks @noirbizarre)

- **Lower priority for copy-ignored**: `wt step copy-ignored` now runs at the lowest OS scheduling priority (`renice -n 19`), yielding CPU to interactive foreground tasks on large trees. ([#1916](https://github.com/max-sixty/worktrunk/pull/1916))

- **Diff stats performance**: Switched from `--numstat` (one line per file) to `--shortstat` (single summary line), reducing diff output from O(files) to O(1) per worktree. ([#1917](https://github.com/max-sixty/worktrunk/pull/1917))

### Fixed

- **Remote detection with `includeIf` config**: `primary_remote()` failed when non-remote git config keys (like `includeIf.hasconfig:remote.*.url`) matched the remote regex. ([#1908](https://github.com/max-sixty/worktrunk/pull/1908), thanks @nirvdrum)

- **Background hook execution**: Fixed three issues — list-form configs lost serial/concurrent semantics in post-merge/post-remove hooks, pipeline `hook_name` context leaked across steps, and lazy template expansion was broken for name-filtered hooks (e.g., `wt hook post-start db`). ([#1910](https://github.com/max-sixty/worktrunk/pull/1910))

- **Copy-ignored parallelism**: The outer loop in `wt step copy-ignored` ran on the global rayon pool instead of the dedicated copy pool, effectively serializing top-level entries. Now runs entirely on the 4-thread copy pool. ([#1913](https://github.com/max-sixty/worktrunk/pull/1913))

- **Windows stack overflow in copy-ignored**: Copy pool worker threads used platform default stack size (~2 MiB on Windows), causing overflow with 200+ directories. Now uses explicit 8 MiB stack size across all platforms. ([#1911](https://github.com/max-sixty/worktrunk/pull/1911))

- **Nix flake build**: Fixed `flake.nix` filtering out the `dev/` directory, which broke builds after OpenCode integration added `include_str!("../../../dev/opencode-plugin.ts")`. ([#1924](https://github.com/max-sixty/worktrunk/pull/1924), thanks @mariuskimmina)

### Internal

- Unified background hook execution into a single pipeline-based path, removing ~260 lines of dual-path branching. ([#1912](https://github.com/max-sixty/worktrunk/pull/1912))
- Replaced deprecated `codecov/test-results-action` with `codecov/codecov-action`. ([#1918](https://github.com/max-sixty/worktrunk/pull/1918))
- Bumped AUR deploy action to v4.1.2 (fixes argument order with Arch Linux's updated `runuser`). ([#1909](https://github.com/max-sixty/worktrunk/pull/1909))

## 0.34.1

### Improved

- **`step prune` performance**: Integration checks now run in parallel, dramatically reducing prune time for repos with many branches (3+ minutes → seconds with 100+ branches). Fixes [#1888](https://github.com/max-sixty/worktrunk/issues/1888). ([#1890](https://github.com/max-sixty/worktrunk/pull/1890), thanks @ortonomy for reporting)

### Fixed

- **CPU saturation during copy operations**: Restored a dedicated 4-thread copy pool that was accidentally removed in v0.34.0, preventing ~1000% CPU usage on copy-heavy operations like `step copy-ignored`. ([#1905](https://github.com/max-sixty/worktrunk/pull/1905))

- **Background pipeline template variables**: When `wt switch --create` fires both post-switch and post-start hooks, pipeline steps were incorrectly accumulated into a single background process, causing `{{ hook_type }}` to expand to the wrong value. Each hook type now spawns its own pipeline. ([#1904](https://github.com/max-sixty/worktrunk/pull/1904))

### Internal

- Extracted shared `classify_unknown_key` to deduplicate config warning logic. ([#1902](https://github.com/max-sixty/worktrunk/pull/1902))

## 0.34.0

### Improved

- **Per-branch custom variables**: New `wt config state vars set/get/list/clear` commands store custom key-value pairs per branch, accessible as `{{ vars.key }}` in hook templates and `wt step eval`. Variables persist in git config and appear in `wt list --format=json`. ([#1006](https://github.com/max-sixty/worktrunk/pull/1006))

- **Lazy template expansion in pipelines**: Pipeline steps now expand `{{ vars.* }}` at execution time rather than at pipeline construction, so variables set by step N are available in step N+1. ([#1840](https://github.com/max-sixty/worktrunk/pull/1840))

- **`wt config plugins claude` commands**: New `install`, `uninstall`, and `install-statusline` subcommands manage Claude Code integration. `install` registers the worktrunk plugin via the Claude marketplace, `install-statusline` configures the Claude Code status line, and `wt config show` suggests these commands instead of raw CLI instructions. ([#1830](https://github.com/max-sixty/worktrunk/pull/1830), [#1834](https://github.com/max-sixty/worktrunk/pull/1834))

- **`[forge]` config section**: New explicit `[forge]` section with `platform` and `hostname` fields for SSH host aliases and non-standard remotes. `ci.platform` is deprecated with automatic migration. ([#1826](https://github.com/max-sixty/worktrunk/pull/1826))

- **Forge detection with `url.insteadOf`**: Forge platform detection now falls back to the effective URL (after git `url.insteadOf` rewrites), fixing CI status, PR/MR detection, and push-remote features for users with SSH aliases or corporate mirrors. ([#1771](https://github.com/max-sixty/worktrunk/pull/1771), thanks @amodelaweb; thanks @roytouw for reporting [#1790](https://github.com/max-sixty/worktrunk/issues/1790))

- **`--branch` flag for `wt step commit`**: Commit to a specific branch without switching to it — useful in automation and scripts. ([#1750](https://github.com/max-sixty/worktrunk/pull/1750))

- **Last fetch time in branch-not-found hint**: When `wt switch` can't find a branch, the hint now shows when the remote was last fetched (e.g., "last fetched 3h ago") to help identify stale local refs. ([#1877](https://github.com/max-sixty/worktrunk/pull/1877))

- **Config field renames**: `merge.no-ff` → `merge.ff` and `switch.no-cd` → `switch.cd`, using positive-sense naming. Old names continue to work with deprecation warnings and automatic migration via `wt config update`. ([#1856](https://github.com/max-sixty/worktrunk/pull/1856), [#1860](https://github.com/max-sixty/worktrunk/pull/1860))

- **Syntax highlighting for template blocks**: Documentation site now renders `{{ }}` template expressions with syntax highlighting. ([#1792](https://github.com/max-sixty/worktrunk/pull/1792))

- **Hide Claude Code section when CLI unavailable**: `wt config show` no longer displays the Claude Code integration section if the `claude` CLI is not found. ([#1827](https://github.com/max-sixty/worktrunk/pull/1827))

### Fixed

- **Copy-ignored too many open files**: `wt step copy-ignored` could exhaust file descriptors on large trees. Now reuses a single thread pool across all copy operations. Fixes [#1865](https://github.com/max-sixty/worktrunk/issues/1865). ([#1864](https://github.com/max-sixty/worktrunk/pull/1864), thanks @fspeirs)

- **Squash-merged branch detection with merge-tree conflicts**: `wt step prune` and `wt list` failed to detect squash-merged branches when the default branch modified the same files. Now uses patch-id matching as fallback. Fixes [#1818](https://github.com/max-sixty/worktrunk/issues/1818). ([#1820](https://github.com/max-sixty/worktrunk/pull/1820), thanks @tthyer for reporting)

- **Background removal blocked for 1 second**: `wt remove` blocked unnecessarily due to incorrect shell operator precedence in the background process spawn. ([#1858](https://github.com/max-sixty/worktrunk/pull/1858))

- **Fish shell getcwd error in Zellij**: Removing a worktree while using fish in Zellij produced "error retrieving current directory" messages. ([#1787](https://github.com/max-sixty/worktrunk/pull/1787))

- **Alias detection false positive on path substrings**: `wt config show` incorrectly flagged unrelated aliases when the alias target path contained "wt" as a substring. Fixes [#1772](https://github.com/max-sixty/worktrunk/issues/1772). ([#1773](https://github.com/max-sixty/worktrunk/pull/1773), thanks @nicolasff for reporting)

- **Branch names with dots in vars**: Vars parsing incorrectly split branch names containing dots (e.g., `release.1.0`) as nested config keys. ([#1837](https://github.com/max-sixty/worktrunk/pull/1837))

- **Lazy pipeline vars expansion in background hooks**: Background hook execution failed with lazy vars expansion due to raw string quoting and overly strict template validation. ([#1855](https://github.com/max-sixty/worktrunk/pull/1855))

- **GitLab MR remote tracking**: `wt switch mr:N` could reuse branches tracking the correct merge-request ref but on the wrong remote. ([#1817](https://github.com/max-sixty/worktrunk/pull/1817))

- **Fork CI and integration target detection**: Fixed CI check-runs querying the wrong repo for forks, branch tracking checking only merge config, and diverged local branches missing remote merges. ([#1812](https://github.com/max-sixty/worktrunk/pull/1812))

- **Placeholder directory on non-current worktree removal**: `wt remove` created unnecessary empty placeholder directories and slept for 1 second when removing worktrees other than the current one. ([#1868](https://github.com/max-sixty/worktrunk/pull/1868), [#1874](https://github.com/max-sixty/worktrunk/pull/1874))

- **Merge-tree errors silently swallowed**: `git merge-tree` failures (invalid refs, corrupt repos) were treated as conflicts instead of propagating, triggering expensive patch-id fallback unnecessarily. ([#1896](https://github.com/max-sixty/worktrunk/pull/1896))

- **Deprecated key in wrong config file**: A deprecated section key (e.g., `[commit-generation]`) in the wrong config file (e.g., project config) was silently filtered. Now warns "Key X belongs in Y config as Z". ([#1899](https://github.com/max-sixty/worktrunk/pull/1899))

- **Config migration mutex panic**: Replaced unsafe `unwrap()` with error propagation in config deprecation migration. ([#1887](https://github.com/max-sixty/worktrunk/pull/1887))

- **Hook show outside git repo**: `wt hook show` now provides a clear error message when run outside a git repository. ([#1809](https://github.com/max-sixty/worktrunk/pull/1809), thanks @noirbizarre)

### Documentation

- Help text rewritten for `switch`, `merge`, `hook`, and `remove` commands. ([#1782](https://github.com/max-sixty/worktrunk/pull/1782), [#1783](https://github.com/max-sixty/worktrunk/pull/1783), [#1785](https://github.com/max-sixty/worktrunk/pull/1785), [#1765](https://github.com/max-sixty/worktrunk/pull/1765), [#1764](https://github.com/max-sixty/worktrunk/pull/1764))

- Hook documentation restructured: types reordered by paired events, pipeline ordering rewritten with progressive examples, approval prompt shown in color. ([#1763](https://github.com/max-sixty/worktrunk/pull/1763), [#1756](https://github.com/max-sixty/worktrunk/pull/1756), [#1766](https://github.com/max-sixty/worktrunk/pull/1766))

- Hooks documented in user config reference. ([#1845](https://github.com/max-sixty/worktrunk/pull/1845))

- Deprecated `post-create` removed from documentation. ([#1776](https://github.com/max-sixty/worktrunk/pull/1776))

- Arch Linux official package added to installation instructions. ([#1872](https://github.com/max-sixty/worktrunk/pull/1872), thanks @ctrl-q)

- README template syntax fixed. Fixes [#1851](https://github.com/max-sixty/worktrunk/issues/1851). ([#1852](https://github.com/max-sixty/worktrunk/pull/1852), thanks @IlyaSemenov for reporting)

### Internal

- Config deprecation consolidated from two layers to one pre-deserialization TOML migration. ([#1879](https://github.com/max-sixty/worktrunk/pull/1879), [#1880](https://github.com/max-sixty/worktrunk/pull/1880), [#1876](https://github.com/max-sixty/worktrunk/pull/1876))

- Benchmark infrastructure extracted into `wt-perf` crate. ([#1878](https://github.com/max-sixty/worktrunk/pull/1878))

- `wt remove` approval path reuses already-loaded repo/config (~50ms savings). ([#1875](https://github.com/max-sixty/worktrunk/pull/1875))

### Deprecated

| Old | New | Action |
|-----|-----|--------|
| `[ci]` section | `[forge]` section | `wt config update` migrates; `wt config show` warns |
| `no-ff` in `[merge]` | `ff` (reversed) | `wt config update` migrates; `wt config show` warns |
| `no-cd` in `[switch]` | `cd` (reversed) | `wt config update` migrates; `wt config show` warns |

All deprecated fields continue to work. Run `wt config update` to migrate, or `wt config show` for details.

## 0.33.0

### Improved

- **Hook execution pipelines**: Post-* hooks support TOML array syntax for serial dependencies — steps execute in order, with maps within steps running concurrently. `post-start = [{ install = "npm install" }, { build = "npm run build", lint = "npm run lint" }]` runs install first, then build and lint in parallel. [Docs](https://worktrunk.dev/hook/) ([#1713](https://github.com/max-sixty/worktrunk/pull/1713))

- **Copy-ignored exclude patterns**: `wt step copy-ignored` now skips built-in VCS metadata and tool-state directories (`.bzr/`, `.conductor/`, `.entire/`, `.hg/`, `.jj/`, `.pi/`, `.pijul/`, `.sl/`, `.svn/`, `.worktrees/`) by default. Additional excludes are configurable via `[step.copy-ignored] exclude = [...]` in user or project config. ([#1667](https://github.com/max-sixty/worktrunk/pull/1667), thanks @shunkakinoki for [#1653](https://github.com/max-sixty/worktrunk/issues/1653))

- **Copy-ignored parallelized**: `wt step copy-ignored` directory walks run in parallel with a dedicated 4-thread pool, improving performance on multi-core systems. ([#1721](https://github.com/max-sixty/worktrunk/pull/1721))

- **Alias append semantics**: Aliases now use append semantics across all config layers, matching hook merge behavior. Within user config, per-project aliases append to global aliases on collision (global first). Across configs, project-config aliases also run alongside user aliases (user first, then project with approval) — previously the user version silently suppressed the project version. ([#1724](https://github.com/max-sixty/worktrunk/pull/1724), [#1727](https://github.com/max-sixty/worktrunk/pull/1727))

- **Agent skill discovery**: The website now serves `.well-known/agent-skills/` for web-based skill discovery by AI agents. ([#1751](https://github.com/max-sixty/worktrunk/pull/1751))

### Fixed

- **Picker alt-r skipped remove hooks**: Removing a worktree via `alt-r` in the picker bypassed pre-remove and post-remove hooks. Pre-remove hooks now run synchronously (non-zero exit aborts removal), and post-remove hooks spawn in the background. ([#1710](https://github.com/max-sixty/worktrunk/pull/1710))

- **False positive shell integration warning**: `wt config show` reported "Found wt in ... but not detected as integration" for Nushell and Fish wrapper files that ARE the integration. Fixes [#1735](https://github.com/max-sixty/worktrunk/issues/1735). ([#1736](https://github.com/max-sixty/worktrunk/pull/1736), thanks @saschabratton)

- **Bare repo config path ignored**: `wt hook approvals add` and other config commands failed to find `.config/wt.toml` in bare repositories because they looked relative to the current worktree instead of the primary worktree. Fixes [#1744](https://github.com/max-sixty/worktrunk/issues/1744). ([#1745](https://github.com/max-sixty/worktrunk/pull/1745), thanks @jrdncstr)

### Documentation

- Help text for `wt step` subcommands cleaned up — redundant openers removed. ([#1737](https://github.com/max-sixty/worktrunk/pull/1737))

- Experimental badge placement fixed in generated documentation. ([#1742](https://github.com/max-sixty/worktrunk/pull/1742), [#1729](https://github.com/max-sixty/worktrunk/pull/1729), [#1734](https://github.com/max-sixty/worktrunk/pull/1734), [#1746](https://github.com/max-sixty/worktrunk/pull/1746))

### Internal

- Copy-ignored built-in exclude constants consolidated. ([#1738](https://github.com/max-sixty/worktrunk/pull/1738))

- `Cmd::env()` accepts `AsRef<OsStr>` for direct path compatibility. ([#1723](https://github.com/max-sixty/worktrunk/pull/1723))

- Picker width survey snapshots for layout testing at various terminal sizes. ([#1613](https://github.com/max-sixty/worktrunk/pull/1613))

## 0.32.0

### Improved

- **Hooks rationalized**: Every lifecycle event now has a symmetric `pre-` (blocking) / `post-` (background) pair. This required one rename: `post-create` → `pre-start`, reflecting that it runs *before* `post-start` as a blocking dependency step. A new `post-commit` hook fires in the background after commits (including squash commits during merge). `post-merge` is now background instead of blocking, consistent with all other `post-*` hooks. Configs using `post-create` get a deprecation warning on any `wt` command; run `wt config update` to rename automatically. The old name continues to work during the deprecation period. [Docs](https://worktrunk.dev/hook/) ([#1679](https://github.com/max-sixty/worktrunk/pull/1679), closes [#1670](https://github.com/max-sixty/worktrunk/issues/1670), thanks @ortonomy for reporting [#1571](https://github.com/max-sixty/worktrunk/issues/1571))

- **Detached worktree support**: Detached HEAD worktrees can now be removed via `wt remove /path/to/worktree` and switched to via `wt switch /path/to/worktree`. The interactive picker also handles detached worktrees for both operations. ([#1665](https://github.com/max-sixty/worktrunk/pull/1665), [#1680](https://github.com/max-sixty/worktrunk/pull/1680), thanks @mjakl for reporting [#1661](https://github.com/max-sixty/worktrunk/issues/1661))

- **In-place worktree removal in picker**: Press `alt-r` in the `wt switch` picker to remove the selected worktree without leaving the picker. Currently hidden from picker legend and help text pending a cursor-reset issue ([#1695](https://github.com/max-sixty/worktrunk/issues/1695)). ([#1677](https://github.com/max-sixty/worktrunk/pull/1677), [#1696](https://github.com/max-sixty/worktrunk/pull/1696))

- **Smarter column dropping in `wt list`**: Low-priority columns (Message, Time, Commit) are now dropped when Summary needs more space, using graduated thresholds based on priority distance. Extends the no-data column dropping from v0.31.0. ([#1678](https://github.com/max-sixty/worktrunk/pull/1678))

### Fixed

- **Bare repo project config ignored**: `.config/wt.toml` placed in the primary worktree of a bare repository was not found when running commands from the bare repo root directory. Config is now loaded from the primary worktree as fallback, and accidental config in the bare repo root itself is skipped. Fixes [#1691](https://github.com/max-sixty/worktrunk/issues/1691). ([#1692](https://github.com/max-sixty/worktrunk/pull/1692), [#1697](https://github.com/max-sixty/worktrunk/pull/1697), thanks @seakayone)

- **`pre-start` hook failure was non-blocking**: `pre-start` was the only `pre-*` hook that warned on failure instead of aborting. All `pre-*` hooks now consistently use FailFast. (Breaking: `pre-start` hook failures that previously only warned now abort the operation.) ([#1708](https://github.com/max-sixty/worktrunk/pull/1708))

- **Spurious mismatch warning for detached worktree switches**: Switching to a detached worktree by path produced a "Branch-worktree mismatch" warning because the directory name was treated as a branch name. ([#1686](https://github.com/max-sixty/worktrunk/pull/1686))

- **Detached worktree switch output showed redundant path**: Output now shows "detached worktree" instead of repeating the directory name (which duplicated the path after `@`). ([#1685](https://github.com/max-sixty/worktrunk/pull/1685))

- **Picker alt-r removal fixes**: Picker removals now validate the worktree synchronously before removing it from the list, perform the actual git removal on a background thread to prevent UI freezing, and correctly handle detached worktrees. ([#1699](https://github.com/max-sixty/worktrunk/pull/1699), [#1702](https://github.com/max-sixty/worktrunk/pull/1702), [#1717](https://github.com/max-sixty/worktrunk/pull/1717))

### Documentation

- Changelog and migration guide for hook rationalization. ([#1693](https://github.com/max-sixty/worktrunk/pull/1693))

### Internal

- All test git commands now go through `Cmd` for consistent debug logging and timing traces. ([#1714](https://github.com/max-sixty/worktrunk/pull/1714), [#1716](https://github.com/max-sixty/worktrunk/pull/1716), [#1718](https://github.com/max-sixty/worktrunk/pull/1718))

- Worktree removal logic extracted into shared helpers. ([#1683](https://github.com/max-sixty/worktrunk/pull/1683), [#1700](https://github.com/max-sixty/worktrunk/pull/1700), [#1701](https://github.com/max-sixty/worktrunk/pull/1701))

## 0.31.0

### Improved

- **Hook template variables consolidated**: Bare variables (`branch`, `worktree_path`, `commit`) now consistently point to the Active worktree — the destination for switch/create, the source for merge/remove. New directional variables (`{{ base }}`, `{{ base_worktree_path }}`, `{{ target_worktree_path }}`, `{{ cwd }}`) give hooks explicit access to both sides of two-worktree operations. (Breaking: `{{ worktree_path }}` changed in pre-switch for existing worktrees and in post-merge — use `{{ cwd }}` or `{{ base_worktree_path }}` for the previous behavior.) [Docs](https://worktrunk.dev/hook/) ([#1655](https://github.com/max-sixty/worktrunk/pull/1655), [#1660](https://github.com/max-sixty/worktrunk/pull/1660), [#1663](https://github.com/max-sixty/worktrunk/pull/1663), thanks @sysradium for reporting [#1543](https://github.com/max-sixty/worktrunk/issues/1543))

- **Bare repo worktree-path prompt**: When a bare repo lives at a hidden path like `.git` or `.bare`, `wt switch` now detects that worktrees would get awkward names (e.g., `project/.git.feature`) and offers to configure a `worktree-path` override. Non-interactive environments show the config to add manually. ([#1656](https://github.com/max-sixty/worktrunk/pull/1656), thanks @seakayone for reporting [#1279](https://github.com/max-sixty/worktrunk/issues/1279))

- **Shell completion for step aliases**: Tab-completing `wt step <TAB>` now shows configured aliases alongside built-in step commands, with `--dry-run`, `--yes`, and `--var` flags. ([#1641](https://github.com/max-sixty/worktrunk/pull/1641))

- **`wt list` reclaims space from redundant columns**: When the Path column carries no useful information (all worktree paths are predictable from branch names), its space is reclaimed for Summary and Message. ([#1634](https://github.com/max-sixty/worktrunk/pull/1634))

- **Syntax highlighting for alias dry-run**: `wt step <alias> --dry-run` now uses bash syntax highlighting, matching hook dry-run output. ([#1635](https://github.com/max-sixty/worktrunk/pull/1635))

### Fixed

- **`wt list` hang from fsmonitor daemon**: On macOS with builtin fsmonitor, `wt list` could hang at the "(loading...)" stage because `git fsmonitor--daemon start` inherited pipe file descriptors and held them open indefinitely. ([#1648](https://github.com/max-sixty/worktrunk/pull/1648))

- **Post-remove hooks ran at wrong directory**: Post-remove hooks executed at the user's cwd (which could be the worktree being removed) instead of the primary worktree. ([#1645](https://github.com/max-sixty/worktrunk/pull/1645))

- **Picker showed loading indicator for unavailable data**: The interactive picker used `⋯` (loading) for fields that would never arrive; now uses `·` (unavailable). ([#1651](https://github.com/max-sixty/worktrunk/pull/1651))

- **`wt hook --dry-run` missing directional variables**: Hook dry-run and `--show --expanded` output was missing `base`, `target`, and `target_worktree_path` variables for switch, create, and remove hooks. ([#1669](https://github.com/max-sixty/worktrunk/pull/1669))

### Documentation

- Bare repository layout guide and `worktree-path` example in config docs. ([#1664](https://github.com/max-sixty/worktrunk/pull/1664))

- Migration guide for template variable changes in hook docs. ([#1666](https://github.com/max-sixty/worktrunk/pull/1666))

### Internal

- Renamed internal `select` module to `picker`. ([#1650](https://github.com/max-sixty/worktrunk/pull/1650))

- Consolidated merge/remove removal validation. ([#1625](https://github.com/max-sixty/worktrunk/pull/1625))

## 0.30.1

### Fixed

- **Narrow terminal layout**: `wt switch` picker now uses vertical (Down) layout on terminals narrower than 80 columns, and the Branch column shrinks instead of being dropped — branch names are always visible. ([#1564](https://github.com/max-sixty/worktrunk/pull/1564), [#1626](https://github.com/max-sixty/worktrunk/pull/1626), thanks @armstrjare for reporting [#1563](https://github.com/max-sixty/worktrunk/issues/1563))

- **Bash tab completion showed all branches**: `wt switch feat<TAB>` displayed every branch instead of filtering by prefix, prompting "Display all N possibilities?" for users with many branches. Fish and zsh still use their native substring/fuzzy matching. ([#1622](https://github.com/max-sixty/worktrunk/pull/1622), thanks @altruic for reporting [#1621](https://github.com/max-sixty/worktrunk/issues/1621))

- **Hook command completion pre-filtered in all shells**: `HookCommandCompleter` filtered by prefix before returning candidates, preventing fish/zsh substring matching on hook command names. ([#1627](https://github.com/max-sixty/worktrunk/pull/1627))

- **`wt merge` failed with `submodule.recurse=true`**: Users with `submodule.recurse=true` in their git config saw push errors during merge. Local push now passes `--recurse-submodules=no`. ([#1619](https://github.com/max-sixty/worktrunk/pull/1619), thanks @viicslen for reporting [#1604](https://github.com/max-sixty/worktrunk/issues/1604))

- **Worktree sync uses safe `read-tree`**: Target worktree sync after `--no-ff` push uses `read-tree -m -u` instead of `reset --hard`, consistent with the project's norms. ([#1623](https://github.com/max-sixty/worktrunk/pull/1623))

### Internal

- Inlined `complete_branches` and `complete_hook_commands` into their respective completers. ([#1628](https://github.com/max-sixty/worktrunk/pull/1628), [#1627](https://github.com/max-sixty/worktrunk/pull/1627))

## 0.30.0

### Improved

- **`wt merge --no-ff`**: Create a merge commit instead of fast-forwarding, for semi-linear history (rebased commits plus a merge commit). Also available as `merge.no-ff = true` in user config. [Docs](https://worktrunk.dev/merge/) ([#1438](https://github.com/max-sixty/worktrunk/pull/1438), thanks @siriobalmelli)

- **`wt step eval`** [experimental]: Evaluate template expressions from the command line. All hook variables (`branch`, `repo`, `worktree_path`) and filters (`hash_port`, `sanitize`, `sanitize_db`) are available. Designed for scripting: `curl http://localhost:$(wt step eval '{{ branch | hash_port }}')/health`. [Docs](https://worktrunk.dev/step/) ([#1004](https://github.com/max-sixty/worktrunk/pull/1004), thanks @EduardoSimon for the feature request in [#947](https://github.com/max-sixty/worktrunk/issues/947))

- **`wt step push --no-ff`**: Mirrors `wt merge --no-ff` for manual step-by-step workflows: `wt step commit && wt step rebase && wt step push --no-ff`. ([#1587](https://github.com/max-sixty/worktrunk/pull/1587))

- **Worktree removal now hidden**: Removed worktrees are staged in `.git/wt/trash/` instead of a visible `.wt-removing-*` sibling directory. All worktrunk state consolidated under `.git/wt/`. ([#1583](https://github.com/max-sixty/worktrunk/pull/1583), thanks @ortonomy for reporting [#1572](https://github.com/max-sixty/worktrunk/issues/1572))

### Fixed

- **`wt merge` could remove the default branch worktree in bare repos**: In bare repository layouts, merging from the default branch worktree could remove it instead of preserving it. ([#1620](https://github.com/max-sixty/worktrunk/pull/1620), thanks @viicslen for reporting [#1618](https://github.com/max-sixty/worktrunk/issues/1618))

- **`wt switch` panicked on empty picker selection**: Entering a non-existent branch name in the interactive picker caused a panic. Now returns an error message gracefully. ([#1566](https://github.com/max-sixty/worktrunk/pull/1566), thanks @dlnilsson for reporting [#1565](https://github.com/max-sixty/worktrunk/issues/1565))

- **`copy-ignored` lost directory permissions**: Source directory permissions (e.g., mode 0700 for Postgres data directories) were replaced with default 0755. ([#1590](https://github.com/max-sixty/worktrunk/pull/1590), thanks @RileyMathews for reporting [#1589](https://github.com/max-sixty/worktrunk/issues/1589))

- **`copy-ignored` failed on broken symlinks at destination**: If a gitignored file's destination was already an invalid symlink, the copy failed with "No such file or directory". ([#1549](https://github.com/max-sixty/worktrunk/pull/1549), thanks @armstrjare for reporting [#1547](https://github.com/max-sixty/worktrunk/issues/1547))

- **Nushell `$env.PWD` errors after `wt remove`**: Removing a worktree from inside it produced repeated `$env.PWD points to a non-existent directory` errors in Nushell. ([#1508](https://github.com/max-sixty/worktrunk/pull/1508), thanks @mystilleef for reporting [#1507](https://github.com/max-sixty/worktrunk/issues/1507))

- **Remote URL used `insteadOf` rewrites**: `wt list` and PR detection used the rewritten remote URL instead of the raw config value, causing mismatches with CI and forge detection. ([#1546](https://github.com/max-sixty/worktrunk/pull/1546), thanks @volkanbicer)

- **SIGPIPE from pager quit treated as error**: Quitting a pager (e.g., `q` in `less`) during `wt step diff` showed "terminated by signal 13" instead of exiting cleanly. ([#1559](https://github.com/max-sixty/worktrunk/pull/1559))

- **Missing vs corrupt git config errors conflated**: Missing config keys and corrupt config files both returned the same error, making corrupt configurations hard to diagnose. ([#1610](https://github.com/max-sixty/worktrunk/pull/1610))

- **Shell operator precedence in remove command**: The `|| true` for fsmonitor stop had incorrect precedence, potentially swallowing failures from the entire removal chain. ([#1584](https://github.com/max-sixty/worktrunk/pull/1584))

- **Missing shell escaping in error hints**: Branch names and paths in suggested `cd ... && git switch ...` commands were not shell-escaped. ([#1584](https://github.com/max-sixty/worktrunk/pull/1584))

### Documentation

- **pnpm post-create example**: Added a recipe for running `pnpm install` after worktree creation via `copy-ignored`. ([#1581](https://github.com/max-sixty/worktrunk/pull/1581))

- **Hook execution order**: Clarified that post-create hooks run before post-start hooks. ([#1573](https://github.com/max-sixty/worktrunk/pull/1573))

## 0.29.4

### Improved

- **Destination branch in pre-switch hooks**: `{{ branch }}` in pre-switch hooks now expands to the **destination** branch (as typed by the user) instead of the source worktree's branch. Previously, pre-switch hooks could only see where you were, not where you were going. [Docs](https://worktrunk.dev/hook/) ([#1497](https://github.com/max-sixty/worktrunk/pull/1497), thanks @mayureshwaykole for the discussion in [#1494](https://github.com/max-sixty/worktrunk/issues/1494))

- **LLM tool commands in example config**: `wt config create` now includes double-commented entries for Claude, Codex, opencode, llm, and aichat commands, making them discoverable without reading the docs. [Docs](https://worktrunk.dev/llm-commits/) ([#1531](https://github.com/max-sixty/worktrunk/pull/1531), [#1533](https://github.com/max-sixty/worktrunk/pull/1533))

### Fixed

- **Extra blank line in `config create` output**: The success path printed a blank line between the success message and hint lines, inconsistent with the "already exists" path. ([#1525](https://github.com/max-sixty/worktrunk/pull/1525))

### Documentation

- **Switch docs**: Trimmed upstream tracking paragraph, added missing `pre-switch`/`post-switch` hooks to creation lifecycle, combined GitHub/GitLab sections. ([#1521](https://github.com/max-sixty/worktrunk/pull/1521))

- **List docs**: Restored `--full` prerequisite note for LLM summaries. ([#1517](https://github.com/max-sixty/worktrunk/pull/1517))

- **Experimental badges in headings**: Moved experimental badges from description paragraphs to headings in web docs for cleaner TOC entries. ([#1523](https://github.com/max-sixty/worktrunk/pull/1523))

### Internal

- **CI improvements**: Prevented duplicate inline review comments across cycles, banned blocking `gh pr checks --watch`, fixed verify step for concurrency-cancelled runs, stopped hourly audit from flagging CI polling, added rolling file survey to nightly cleaner. ([#1514](https://github.com/max-sixty/worktrunk/pull/1514), [#1498](https://github.com/max-sixty/worktrunk/pull/1498), [#1519](https://github.com/max-sixty/worktrunk/pull/1519), [#1520](https://github.com/max-sixty/worktrunk/pull/1520), [#1522](https://github.com/max-sixty/worktrunk/pull/1522))

- **Simplified review-pr skill**: Cut metacognitive coaching and collapsed confidence tiers; 504 → 369 lines (−27%). ([#1530](https://github.com/max-sixty/worktrunk/pull/1530))

## 0.29.3

### Improved

- **Unified timeout model for list and picker**: Consolidated the picker's per-command timeout and list's experimental `timeout-ms` into a shared config with `[list] task-timeout-ms` (per-task, shared by both) and per-context wall-clock budgets (`[list] timeout-ms`, `[switch.picker] timeout-ms`). Picker default budget raised from 200ms per-command to 500ms wall-clock. ([#1515](https://github.com/max-sixty/worktrunk/pull/1515), [#1487](https://github.com/max-sixty/worktrunk/pull/1487))

- **Pre-flight template validation for `wt switch`**: Switch templates (`--execute` and hook commands) are now validated before worktree creation, preventing orphan worktrees from syntax errors like `{{ unclosed`. ([#1500](https://github.com/max-sixty/worktrunk/pull/1500))

### Fixed

- **`wt remove` allowed removing default branch worktree**: The default branch worktree (e.g., main) could be removed because it was trivially "integrated" into itself. Now blocked unless `-D` is used. ([#1460](https://github.com/max-sixty/worktrunk/pull/1460), thanks @cperalt for reporting [#1448](https://github.com/max-sixty/worktrunk/issues/1448))

- **Symlinks copied as regular files in `copy-ignored`**: Top-level gitignored symlinks were copied as regular files instead of preserved as symlinks, breaking setups like Yarn monorepos. ([#1489](https://github.com/max-sixty/worktrunk/pull/1489), thanks @karmeleon for reporting [#1488](https://github.com/max-sixty/worktrunk/issues/1488))

- **Missing placeholders in WorkingDiff and Upstream columns**: These columns showed blank instead of `⋯`/`·` placeholders when data wasn't loaded, breaking the visual loading signal. ([#1503](https://github.com/max-sixty/worktrunk/pull/1503))

### Documentation

- **Step command docs**: Added promote subdoc, improved swap description, linked Operations index to subcommand sections, moved aliases section after subcommands, fixed cross-filesystem fallback description. ([#1505](https://github.com/max-sixty/worktrunk/pull/1505), [#1495](https://github.com/max-sixty/worktrunk/pull/1495), [#1502](https://github.com/max-sixty/worktrunk/pull/1502), [#1513](https://github.com/max-sixty/worktrunk/pull/1513))

- **List docs**: Documented placeholder symbols (`⋯`, `·`) in help text, rewrote LLM summaries section. ([#1496](https://github.com/max-sixty/worktrunk/pull/1496), [#1506](https://github.com/max-sixty/worktrunk/pull/1506))

- **Homepage**: Added headline features (CI status, PR checkout, hash_port) and tips link. ([#1501](https://github.com/max-sixty/worktrunk/pull/1501))

- **Experimental badge pills**: Styled `[experimental]` markers as pill badges in web docs. ([#1499](https://github.com/max-sixty/worktrunk/pull/1499))

### Internal

- **Deduplicated hook config resolution**: Extracted shared hook-type list and made `lookup_hook_configs` pub(crate). ([#1512](https://github.com/max-sixty/worktrunk/pull/1512))

- **Agent Skills metadata**: Added `metadata.internal: true` to repo-scoped skills so `npx skills add` only offers user-facing ones. ([#1491](https://github.com/max-sixty/worktrunk/pull/1491))

## 0.29.2

### Improved

- **`[switch] no-cd` config option**: Disable directory change by default with `no-cd = true` in the `[switch]` section. Use `--cd` flag to override when needed. Useful for tmux workflows where sessions handle navigation. [Docs](https://worktrunk.dev/config/) ([#1401](https://github.com/max-sixty/worktrunk/pull/1401), thanks @jradtilbrook)

### Fixed

- **GPG signature output breaks `wt list`**: When `log.showSignature` is set in git config, GPG verification lines contaminated stdout in `git log` calls, causing parse failures. All git log invocations now pass `--no-show-signature`. ([#1465](https://github.com/max-sixty/worktrunk/pull/1465), thanks @apre)

- **Tab completions ignore shell substring matching**: The binary was prefix-filtering branch candidates before returning them to the shell, preventing fish substring matching (`auth<TAB>` → `feature/user-auth`) and zsh fuzzy matching. Completions now return all candidates and let the shell apply its own matching. ([#1471](https://github.com/max-sixty/worktrunk/pull/1471), thanks @benjaminbauer for reporting [#1468](https://github.com/max-sixty/worktrunk/issues/1468))

- **Tab completions unusable in large repos**: Repos with many remote branches triggered the "do you wish to see all N possibilities?" prompt. Remote-only branches are now excluded when the total exceeds 100. ([#1442](https://github.com/max-sixty/worktrunk/pull/1442), thanks @cperalt for reporting [#1415](https://github.com/max-sixty/worktrunk/issues/1415))

- **Nushell shell integration broken in Home Manager module**: The Nix Home Manager module used `use` instead of `source` for the nushell init script, and template definitions were not exported, preventing the `wt` wrapper function from loading. ([#1476](https://github.com/max-sixty/worktrunk/pull/1476), thanks @mystilleef for reporting [#1475](https://github.com/max-sixty/worktrunk/issues/1475))

### Documentation

- **Manual commit message recipes**: Added recipes to Tips & Patterns for using `commit.generation.command` config to write commit messages by hand with `$EDITOR` instead of an LLM. ([#1469](https://github.com/max-sixty/worktrunk/pull/1469), thanks @viicslen for the feature request in [#1467](https://github.com/max-sixty/worktrunk/issues/1467))

### Internal

- **Skill/CI guidance**: Improved Claude bot skills for triage, code review, and CI monitoring. ([#1485](https://github.com/max-sixty/worktrunk/pull/1485), [#1477](https://github.com/max-sixty/worktrunk/pull/1477), [#1474](https://github.com/max-sixty/worktrunk/pull/1474), [#1472](https://github.com/max-sixty/worktrunk/pull/1472), [#1470](https://github.com/max-sixty/worktrunk/pull/1470), [#1458](https://github.com/max-sixty/worktrunk/pull/1458), [#1447](https://github.com/max-sixty/worktrunk/pull/1447))

## 0.29.1

### Improved

- **GitHub Enterprise support for `wt switch`**: `wt switch pr:<number>` now works with GitHub Enterprise instances by extracting the hostname from the remote URL and passing `--hostname` to `gh`. ([#1408](https://github.com/max-sixty/worktrunk/pull/1408), thanks @TomRomeo)

- **`wt switch --no-cd` print mode**: When `wt switch --no-cd` opens the interactive picker (no branch argument), selecting a branch prints its name to stdout and exits — useful for scripting. ([#1445](https://github.com/max-sixty/worktrunk/pull/1445), thanks @ruudk for the feature request in [#1404](https://github.com/max-sixty/worktrunk/pull/1404))

- **Shadow warning for step aliases**: `wt step` now warns when a user-defined alias has the same name as a built-in step command (e.g., `commit`, `rebase`), since clap intercepts the built-in before the alias runs. ([#1389](https://github.com/max-sixty/worktrunk/pull/1389))

### Fixed

- **Post-switch hooks on `wt remove`**: When removing the current worktree, post-switch hooks now fire correctly as the user is implicitly switched to the primary worktree. Previously, project hooks were silently skipped because config lookup failed from the removed CWD. ([#1452](https://github.com/max-sixty/worktrunk/pull/1452), thanks @mjakl for reporting [#1450](https://github.com/max-sixty/worktrunk/issues/1450))

- **LLM commit session isolation**: The recommended Claude command for commit generation now includes `--no-session-persistence`, preventing commit conversations from polluting `claude --continue`. ([#1454](https://github.com/max-sixty/worktrunk/pull/1454))

- **Color formatting in error messages**: `DetachedHead` and `NotInWorktree` error messages now support color-print styling, matching other error variants. ([#1387](https://github.com/max-sixty/worktrunk/pull/1387))

- **Windows error handling**: Replaced `std::process::exit()` with proper error returns in Windows-specific code paths, so destructors and cleanup run correctly. ([#1456](https://github.com/max-sixty/worktrunk/pull/1456))

### Documentation

- **Hook JSON context section**: Fixed documentation that incorrectly described `hook_type` and `hook_name` as extras; added the TOML hook definition showing how JSON stdin is wired. ([#1360](https://github.com/max-sixty/worktrunk/pull/1360))

- **`wt remove` help text**: Updated example heading to clarify that `wt remove` works on both worktrees and branches. ([#1449](https://github.com/max-sixty/worktrunk/pull/1449))

- **Xcode DerivedData cleanup recipe**: Added recipe for cleaning Xcode build artifacts across worktrees. ([#1423](https://github.com/max-sixty/worktrunk/pull/1423), thanks @RickeyBoy)

### Internal

- **Refactoring**: Extracted handler functions from `main()` dispatch, replaced negated boolean variables with positive-polarity names (`no_verify` → `verify`, `no_delete_branch` → `keep_branch`). ([#1394](https://github.com/max-sixty/worktrunk/pull/1394), [#1388](https://github.com/max-sixty/worktrunk/pull/1388), [#1393](https://github.com/max-sixty/worktrunk/pull/1393))

- **Test reliability**: Resolved flaky PTY/timing issues in integration tests, consolidated trivial tests into inline snapshots. ([#1459](https://github.com/max-sixty/worktrunk/pull/1459), [#1392](https://github.com/max-sixty/worktrunk/pull/1392), [#1382](https://github.com/max-sixty/worktrunk/pull/1382), [#1390](https://github.com/max-sixty/worktrunk/pull/1390))

- **CI**: Added Zola docs validation to PR checks, catching broken internal anchor links before merge. ([#1396](https://github.com/max-sixty/worktrunk/pull/1396))

## 0.29.0

### Improved

- **`wt step <alias>` command**: User-defined command templates with template variables (`{{ branch }}`, `{{ worktree }}`, custom `--var KEY=VALUE`). Project-config aliases require approval; user-config aliases are trusted. [Docs](https://worktrunk.dev/step/) ([#1348](https://github.com/max-sixty/worktrunk/pull/1348), thanks @cavanaug for the feature request in [#1214](https://github.com/max-sixty/worktrunk/issues/1214))

- **Remove worktrees from switch picker**: `alt-r` in `wt switch` interactive picker removes the highlighted worktree directly (no force flags — matches safety defaults). ([#1253](https://github.com/max-sixty/worktrunk/pull/1253), thanks @alfredomtx for the feature request in [#1251](https://github.com/max-sixty/worktrunk/issues/1251))

- **`wt hook <type> --dry-run`**: Preview hook expansion with template variables resolved, without executing. ([#1361](https://github.com/max-sixty/worktrunk/pull/1361))

- **Hook template variables**: `{{ hook_type }}` and `{{ hook_name }}` are now available in hook command templates. ([#1364](https://github.com/max-sixty/worktrunk/pull/1364))

- **Typo suggestions for step commands**: Unknown step commands and aliases now suggest the closest match. ([#1363](https://github.com/max-sixty/worktrunk/pull/1363))

- **Syntax-highlighted `--help`**: Code blocks in `--help` output now render with language-aware syntax highlighting (TOML, bash) instead of plain dimmed text. Help options are grouped under navigational headings (Picker Options, Automation). ([#1365](https://github.com/max-sixty/worktrunk/pull/1365), [#1355](https://github.com/max-sixty/worktrunk/pull/1355), [#1359](https://github.com/max-sixty/worktrunk/pull/1359))

- **Nix Home Manager module**: Install worktrunk via Nix Home Manager. ([#1287](https://github.com/max-sixty/worktrunk/pull/1287), thanks @DuskyElf; thanks @meicale for reporting [#1257](https://github.com/max-sixty/worktrunk/issues/1257))

- **Output styling**: Bold names replace quoted names in error messages, underlined references replace bright-black in hints, `@ path` convention unified in section headings, and branch-worktree mismatch warnings now show both actual and expected paths. ([#1375](https://github.com/max-sixty/worktrunk/pull/1375), [#1380](https://github.com/max-sixty/worktrunk/pull/1380), [#1285](https://github.com/max-sixty/worktrunk/pull/1285), [#1376](https://github.com/max-sixty/worktrunk/pull/1376), [#1377](https://github.com/max-sixty/worktrunk/pull/1377), thanks @jhigh2000 for reporting [#1184](https://github.com/max-sixty/worktrunk/issues/1184))

### Fixed

- **`--no-cd` with interactive picker**: The `--no-cd` flag is now passed through when using `wt switch` with the interactive picker. ([#1331](https://github.com/max-sixty/worktrunk/pull/1331), thanks @cperalt for reporting [#1330](https://github.com/max-sixty/worktrunk/issues/1330))

- **Remote branches with `/` in picker**: `wt switch --remotes` now correctly handles remote branches with `/` in the name (e.g., `origin/user/feature`). ([#1266](https://github.com/max-sixty/worktrunk/pull/1266), thanks @curtbushko for reporting [#1260](https://github.com/max-sixty/worktrunk/issues/1260))

- **Nushell config path on Windows**: `wt config shell install` now uses the platform-appropriate config directory for nushell on Windows. ([#1294](https://github.com/max-sixty/worktrunk/pull/1294), thanks @deltoss for reporting [#1293](https://github.com/max-sixty/worktrunk/issues/1293))

- **Git for Windows per-user install**: Detect per-user Git for Windows installations and show a clean error message instead of panicking when Git Bash is not found. ([#1261](https://github.com/max-sixty/worktrunk/pull/1261), [#1262](https://github.com/max-sixty/worktrunk/pull/1262), thanks @JefMasereel for reporting [#1259](https://github.com/max-sixty/worktrunk/issues/1259))

- **JSON output `summary` field**: `wt list --format=json` now includes the `summary` field. ([#1339](https://github.com/max-sixty/worktrunk/pull/1339))

- **Squash merge message**: Uses source branch name instead of target branch in the merge commit message. ([#1319](https://github.com/max-sixty/worktrunk/pull/1319), thanks @ricafeal)

- **Alias approval errors**: Propagate the real error (e.g., "no remote URL found") instead of a vague "Cannot determine project identifier". ([#1374](https://github.com/max-sixty/worktrunk/pull/1374))

- **`wt step prune` output**: Summary uses cleaner paired format ("Pruned 1 worktree & branch") and fixes post-remove hook display path for non-current worktrees. ([#1344](https://github.com/max-sixty/worktrunk/pull/1344))

- **VCS metadata in `copy-ignored`**: Exclude `.git`, `.hg`, `.svn`, `_darcs` directories from `wt step copy-ignored`. ([#1250](https://github.com/max-sixty/worktrunk/pull/1250))

- **Nix evaluation warning**: Use `stdenv.hostPlatform.system` instead of deprecated `system`. ([#1336](https://github.com/max-sixty/worktrunk/pull/1336), thanks @onelocked)

### Documentation

- **Home page SEO**: Canonical URL deduplication and consistent tagline. ([#1357](https://github.com/max-sixty/worktrunk/pull/1357))

- **LLM commit tools**: Add opencode and consolidate other LLM commit tool references. ([#1295](https://github.com/max-sixty/worktrunk/pull/1295))

### Internal

- **Git plumbing**: Replace porcelain commands with plumbing alternatives (`rev-parse --symbolic-full-name`, `log --no-walk`, `for-each-ref`) for more robust output parsing. Cache deprecated-variable regexes and fix silent wrong results in `same_commit()`/`trees_match()`. ([#1345](https://github.com/max-sixty/worktrunk/pull/1345), [#1338](https://github.com/max-sixty/worktrunk/pull/1338), [#1358](https://github.com/max-sixty/worktrunk/pull/1358))

- **Error propagation**: `repo_path()` and `ShellConfig::get()` now return `Result` instead of silently falling back. ([#1280](https://github.com/max-sixty/worktrunk/pull/1280), [#1262](https://github.com/max-sixty/worktrunk/pull/1262))

- **CI improvements**: Consolidated setup into composite action, replaced `gh run watch` with poll loops, added conflict resolution for bot PRs in nightly cleaner. ([#1273](https://github.com/max-sixty/worktrunk/pull/1273), [#1329](https://github.com/max-sixty/worktrunk/pull/1329), [#1307](https://github.com/max-sixty/worktrunk/pull/1307))

## 0.28.2

### Improved

- **`wt step prune` output**: Dirty or locked worktrees are silently skipped instead of printing warnings, and "No worktree found for branch" info messages are suppressed — prune output now shows only what was actually removed. ([#1236](https://github.com/max-sixty/worktrunk/pull/1236))

### Fixed

- **CWD removal hint**: After a worktree is removed while a shell is in that directory, the hint now checks whether `wt switch ^` would actually work before suggesting it — falls back to suggesting `wt list` when the default branch worktree doesn't exist (e.g., bare repos). ([#1238](https://github.com/max-sixty/worktrunk/pull/1238), thanks @davidbeesley for reporting [#1168](https://github.com/max-sixty/worktrunk/issues/1168))

- **Submodule detection in worktree removal**: Submodule detection now uses `git submodule status` output instead of parsing error messages, avoiding locale-dependent and version-dependent string matching. ([#1247](https://github.com/max-sixty/worktrunk/pull/1247))

### Internal

- **Hook dispatch**: Introduced `HookCommandSpec` struct and extracted helper functions to deduplicate hook dispatch code (~50 lines net reduction). ([#1248](https://github.com/max-sixty/worktrunk/pull/1248))

- **CI skills**: Fixed jq escaping in ad-hoc CI polling queries and improved Step 5 dismissal ordering in pr-review skill. ([#1241](https://github.com/max-sixty/worktrunk/pull/1241), [#1246](https://github.com/max-sixty/worktrunk/pull/1246))

## 0.28.1

### Improved

- **Nushell tab completions**: `wt switch <TAB>` and subcommand completions now work in nushell. ([#1220](https://github.com/max-sixty/worktrunk/pull/1220), thanks @omerxx for reporting [#1215](https://github.com/max-sixty/worktrunk/issues/1215))

- **`wt step prune` reliability**: Candidates are now removed inline as they're discovered instead of scan-then-remove, with per-candidate error handling (dirty worktrees are warned and skipped). Dry-run and execution summaries now distinguish worktrees, branches, and detached worktrees. Command marked `[experimental]`. ([#1234](https://github.com/max-sixty/worktrunk/pull/1234), [#1232](https://github.com/max-sixty/worktrunk/pull/1232), [#1223](https://github.com/max-sixty/worktrunk/pull/1223))

- **`wt step diff` performance**: Copies the real git index instead of creating an empty one, preserving git's stat cache so unchanged tracked files are skipped. ([#1230](https://github.com/max-sixty/worktrunk/pull/1230))

### Fixed

- **Branch delete race on fast-path remove**: `wt remove` now deletes merged branches synchronously on the fast path instead of deferring to the background process, fixing a race where `wt switch --create <branch>` fails with "branch already exists". ([#1216](https://github.com/max-sixty/worktrunk/pull/1216))

- **Panic in `is_bare()` on unusual repositories**: `is_bare()` now propagates errors instead of panicking. ([#1221](https://github.com/max-sixty/worktrunk/pull/1221), @bendrucker)

- **Help text table coloring**: Status symbols and backtick-enclosed text in `--help` tables now render with proper ANSI colors. ([#1231](https://github.com/max-sixty/worktrunk/pull/1231))

### Internal

- **CI workflow**: Added concurrency group to claude-mention workflow, fixed external contributor PR review permissions. ([#1233](https://github.com/max-sixty/worktrunk/pull/1233), [#1226](https://github.com/max-sixty/worktrunk/pull/1226))

## 0.28.0

### Improved

- **`wt step prune` command**: Remove worktrees whose branches are already merged into the default branch. Skips unmerged and recently created worktrees, with `--min-age` to control the staleness threshold. [Docs](https://worktrunk.dev/step/) ([#1191](https://github.com/max-sixty/worktrunk/pull/1191))

- **Color palette in `wt config shell show-theme`**: Shows each color and style rendered in itself — base colors, modifiers, bold+color and dim+color variants — for diagnosing legibility issues on different terminal themes. ([#1185](https://github.com/max-sixty/worktrunk/pull/1185), thanks @jhigh2000 for reporting [#1184](https://github.com/max-sixty/worktrunk/issues/1184))

- **Smarter column layout in `wt list`**: The Message column is hidden when the terminal is too narrow for Summary to reach 40 characters, preventing both columns from being truncated to unreadable widths. ([#1166](https://github.com/max-sixty/worktrunk/pull/1166))

### Fixed

- **Submodules in worktree removal**: `wt remove` now handles worktrees containing initialized git submodules, which previously failed with "working trees containing submodules cannot be moved or removed". ([#1196](https://github.com/max-sixty/worktrunk/pull/1196), thanks @dlecan for reporting [#1194](https://github.com/max-sixty/worktrunk/issues/1194))

- **CWD recovery validation**: Recovery from a deleted worktree directory now validates that candidate repositories actually contain the deleted path as a worktree, preventing false matches when multiple repos share a parent directory. ([#1193](https://github.com/max-sixty/worktrunk/pull/1193))

- **Shell-escape paths in `-C` flag hints**: Paths containing spaces or special characters in `-C` hints are now properly shell-escaped. ([#1173](https://github.com/max-sixty/worktrunk/pull/1173))

- **ANSI handling in CWD recovery**: Recovery messages now use `anstream` for proper ANSI handling on terminals that don't support color. ([#1183](https://github.com/max-sixty/worktrunk/pull/1183))

- **Worktree path in detached HEAD removal messages**: Removal output for detached HEAD worktrees now includes the worktree path for clarity. ([#1210](https://github.com/max-sixty/worktrunk/pull/1210))

- **Pruned worktree output**: Worktree and branch deletion for pruned worktrees are combined into a single output line instead of two separate messages. ([#1211](https://github.com/max-sixty/worktrunk/pull/1211))

### Documentation

- **Page metadata and SEO**: All doc pages now have `<meta name="description">`, canonical URLs, and structured data (JSON-LD) for better search engine visibility. ([#1167](https://github.com/max-sixty/worktrunk/pull/1167))

### Internal

- **CI bot improvements**: Inline suggestions, confidence-based review scrutiny, consolidated review+CI analysis, self-poll prevention, verified-facts guideline for triage, and explicit issue-closing in nightly cleaner. ([#1172](https://github.com/max-sixty/worktrunk/pull/1172), [#1181](https://github.com/max-sixty/worktrunk/pull/1181), [#1199](https://github.com/max-sixty/worktrunk/pull/1199), [#1204](https://github.com/max-sixty/worktrunk/pull/1204), [#1212](https://github.com/max-sixty/worktrunk/pull/1212), [#1198](https://github.com/max-sixty/worktrunk/pull/1198), [#1209](https://github.com/max-sixty/worktrunk/pull/1209))

## 0.27.0

### Improved

- **`wt step promote` command (experimental)**: Exchange branches between the main worktree and any linked worktree, including swapping gitignored files (build artifacts, `.env`, `node_modules/`). Shows mismatch state in `wt list` with ⚑ indicator; restore with no arguments from main worktree. [Docs](https://worktrunk.dev/step/) ([#789](https://github.com/max-sixty/worktrunk/pull/789), thanks @zpeleg for the feature request in [#738](https://github.com/max-sixty/worktrunk/issues/738))

- **Instant worktree removal**: `wt remove` now renames the worktree to a staging path before spawning the background cleanup, making the path unavailable immediately instead of after a 1-second sleep. Falls back to legacy removal if rename fails (cross-filesystem, permissions). ([#773](https://github.com/max-sixty/worktrunk/pull/773))

- **Graceful recovery from deleted worktree directory**: When a worktree is removed while a shell is still in that directory, `wt switch` and `wt list` now recover automatically — find the parent repository from `$PWD` and proceed without pre-switch hooks. ([#1146](https://github.com/max-sixty/worktrunk/pull/1146), thanks @davidbeesley for reporting [#1109](https://github.com/max-sixty/worktrunk/issues/1109))

- **PR/MR support promoted out of experimental**: GitHub PR (`pr:<number>`) and GitLab MR (`mr:<number>`) targets in `wt switch` are now considered stable — 11 minor releases with no interface changes since v0.15.0. ([#1114](https://github.com/max-sixty/worktrunk/pull/1114))

### Fixed

- **SSH URLs with ports**: Remote matching now handles `ssh://git@host:2222/owner/repo.git` — ports are stripped during URL parsing instead of rejecting the URL. ([#1151](https://github.com/max-sixty/worktrunk/pull/1151))

- **Config path resolution**: `wt config create` now resolves the same path as config loading, fixing a mismatch when using XDG directories. ([#1135](https://github.com/max-sixty/worktrunk/pull/1135), thanks @christopher-buss for reporting [#1134](https://github.com/max-sixty/worktrunk/issues/1134))

- **PTY prompt echo interleaving**: Approval prompts no longer intermix with echoed input on slower systems. Uses quiescence detection instead of a fixed sleep. ([#1133](https://github.com/max-sixty/worktrunk/pull/1133))

- **Better diagnostics when foreground removal fails**: When `wt remove --foreground` fails with "Directory not empty", now shows the remaining top-level entries (capped at 10) and suggests trying background removal. ([#1150](https://github.com/max-sixty/worktrunk/pull/1150))

- **Output formatting consistency**: Hints use canonical "To X, run Y" phrasing, config update hints render in gutter blocks with correct `-C` flag for linked worktrees, and ANSI color nesting fixed in hint messages. ([#1138](https://github.com/max-sixty/worktrunk/pull/1138), [#1137](https://github.com/max-sixty/worktrunk/pull/1137))

- **Panic-safe error propagation**: Replaced `.unwrap()` and `.expect()` calls in functions returning `Result` with proper `?` and `bail!` error propagation. ([#1127](https://github.com/max-sixty/worktrunk/pull/1127))

### Documentation

- **Bot trigger renamed**: CI bot responds to `@worktrunk-bot` instead of `@claude`, matching the actual GitHub username. ([#1149](https://github.com/max-sixty/worktrunk/pull/1149))

- **`wt step promote` documented in worktree model**: The branch-exchange operation is noted as the sole exception to the "never retarget a worktree" rule. ([#1154](https://github.com/max-sixty/worktrunk/pull/1154))

### Internal

- **CI security model**: Rulesets, token consolidation, and environment protection hardened for GitHub Actions workflows. ([#1118](https://github.com/max-sixty/worktrunk/pull/1118))

- **Nightly CI workflows**: Automated review of Claude CI session logs and 24-hour code quality sweep for bugs, missing tests, and stale docs. ([#1111](https://github.com/max-sixty/worktrunk/pull/1111))

- **CI reviewer and bot improvements**: Better failure tracing, Dependabot PR reviews, thread resolution ordering, LGTM dedup, actionable feedback, automatic response to bot PR comments, and graceful handling of mentions on merged/closed PRs. ([#1117](https://github.com/max-sixty/worktrunk/pull/1117), [#1128](https://github.com/max-sixty/worktrunk/pull/1128), [#1129](https://github.com/max-sixty/worktrunk/pull/1129), [#1131](https://github.com/max-sixty/worktrunk/pull/1131), [#1141](https://github.com/max-sixty/worktrunk/pull/1141), [#1142](https://github.com/max-sixty/worktrunk/pull/1142), [#1145](https://github.com/max-sixty/worktrunk/pull/1145), [#1147](https://github.com/max-sixty/worktrunk/pull/1147), [#1153](https://github.com/max-sixty/worktrunk/pull/1153), [#1158](https://github.com/max-sixty/worktrunk/pull/1158), [#1164](https://github.com/max-sixty/worktrunk/pull/1164))

## 0.26.1

### Fixed

- **Statusline panic without LLM config**: `wt list statusline` panicked when no LLM command was configured. Now skips summary generation gracefully. ([#1107](https://github.com/max-sixty/worktrunk/pull/1107))

### Internal

- Demo GIFs now show the Summary column in `wt list --full` output. ([#1104](https://github.com/max-sixty/worktrunk/pull/1104), [#1106](https://github.com/max-sixty/worktrunk/pull/1106))
- CI session log uploads fixed to use correct path. ([#1103](https://github.com/max-sixty/worktrunk/pull/1103))

## 0.26.0

### Improved

- **Summary column in `wt list --full`**: LLM-generated one-line branch descriptions. Opt-in via `[list] summary = true` in config (experimental). Requires `[commit.generation]` config. ([#1100](https://github.com/max-sixty/worktrunk/pull/1100))

- **`wt step diff` command**: Show all uncommitted and untracked changes that `wt merge` would include as a unified diff against the merge base. Pass `-- --stat` for a summary. [Docs](https://worktrunk.dev/step/) Closes [#1043](https://github.com/max-sixty/worktrunk/issues/1043). ([#1074](https://github.com/max-sixty/worktrunk/pull/1074), thanks @davidbeesley for the feature discussion)

- **`pre-switch` hook**: New hook that runs before `wt switch` validation. Use it to fetch-if-stale or run pre-flight checks before switching. Respects `--no-verify`. [Docs](https://worktrunk.dev/hook/) ([#1094](https://github.com/max-sixty/worktrunk/pull/1094), thanks @jdb8 for the use case in [#1085](https://github.com/max-sixty/worktrunk/issues/1085))

- **`wt config update` command**: Automatically apply config migrations — detects deprecated patterns (template variables, `[commit-generation]`, `approved-commands`), shows a diff preview, and applies with confirmation. Use `--yes` to skip the prompt. ([#1083](https://github.com/max-sixty/worktrunk/pull/1083))

- **Configurable picker timeout**: New `[switch.picker] timeout-ms` setting (default: 200ms, `0` to disable). The `[select]` config section is deprecated in favor of `[switch.picker]` — run `wt config update` to migrate. ([#1087](https://github.com/max-sixty/worktrunk/pull/1087))

- **Command audit log**: All hook executions and LLM commands are logged to `.git/wt-logs/commands.jsonl` with timestamps, exit codes, and duration. Auto-rotates at 1MB. View with `wt config state logs get` or query with `jq`. ([#1088](https://github.com/max-sixty/worktrunk/pull/1088))

### Fixed

- **Hook CWD wrong from subdirectories**: Hooks invoked from a subdirectory within a worktree ran with incorrect CWD and `{{ worktree_path }}`/`{{ worktree_name }}` template variables resolved incorrectly. ([#1097](https://github.com/max-sixty/worktrunk/pull/1097))

- **`copy-ignored` verbose output and error handling**: `-v` flag was silently ignored, error messages lacked file paths, and broken symlinks from interrupted copies caused failures. Also skips non-regular files (sockets, FIFOs) instead of failing. Fixes [#1084](https://github.com/max-sixty/worktrunk/issues/1084). ([#1090](https://github.com/max-sixty/worktrunk/pull/1090), thanks @jdb8 for reporting)

- **Nushell `wt list` piping**: `wt list --format json | from json` failed in nushell because the wrapper's stdout capture prevented piping. Fixes [#1062](https://github.com/max-sixty/worktrunk/issues/1062). ([#1081](https://github.com/max-sixty/worktrunk/pull/1081), thanks @omerxx for reporting)

- **Approved-commands lost during config migration**: Running the config migration could silently discard existing approval data. Now copies `approved-commands` entries to `approvals.toml` before migration. ([#1079](https://github.com/max-sixty/worktrunk/pull/1079))

- **Deprecation messages reference `wt config update`**: Deprecation warnings now point to the new `wt config update` command for one-step migration instead of manual `mv` instructions. ([#1089](https://github.com/max-sixty/worktrunk/pull/1089))

### Documentation

- **`wt switch` help text**: Updated description to "Switch to a worktree; create if needed" to surface auto-create behavior. ([#1082](https://github.com/max-sixty/worktrunk/pull/1082))

- **Docs syntax highlighting**: Migrated to giallo engine with a warm theme. ([#1080](https://github.com/max-sixty/worktrunk/pull/1080))

### Internal

- **CI reviewer improvements**: File-based GraphQL queries, centralized shell quoting guidance, artifact upload path fixes. ([#1091](https://github.com/max-sixty/worktrunk/pull/1091), [#1098](https://github.com/max-sixty/worktrunk/pull/1098), [#1099](https://github.com/max-sixty/worktrunk/pull/1099))

- **Issue triage for external contributors**: CI triage workflow now runs for all external contributor issues. ([#1086](https://github.com/max-sixty/worktrunk/pull/1086))

## 0.25.0

### Improved

- **System-wide config file**: Load organization-wide defaults from a system config file (`/etc/xdg/worktrunk/config.toml` on Linux, `/Library/Application Support/worktrunk/config.toml` on macOS) before user config. Override the path with `$WORKTRUNK_SYSTEM_CONFIG_PATH`. Visible in `wt config show`. ([#963](https://github.com/max-sixty/worktrunk/pull/963), thanks @goodtune)

- **AI summary preview in `wt switch`**: New 5th tab shows AI-generated branch summaries using your configured `[commit.generation]` LLM command. Opt-in via `[list] summary = true` in config. Summaries are cached in `.git/wt-cache/summaries/` with hash-based invalidation. [Docs](https://worktrunk.dev/llm-commits/) ([#1049](https://github.com/max-sixty/worktrunk/pull/1049))

- **Approvals stored in dedicated file**: Approved commands moved from `config.toml` to `approvals.toml`, enabling dotfile management of config without exposing machine-local trust state. Existing approvals in `config.toml` are read automatically with a deprecation warning and migration instructions in `wt config show`. ([#1042](https://github.com/max-sixty/worktrunk/pull/1042))

- **Error hints include `--execute` context**: When `wt switch --execute=<cmd>` fails, suggested commands now include the full `--execute` and trailing args so you can copy-paste the fix directly. ([#1064](https://github.com/max-sixty/worktrunk/pull/1064))

- **`wt list` startup performance**: Config resolution moved into the parallel phase, running concurrently with other git commands instead of sequentially on the critical path. ([#1054](https://github.com/max-sixty/worktrunk/pull/1054))

### Fixed

- **Submodule worktree path resolution**: `wt switch` resolved to `.git/modules/` instead of the working directory inside git submodules. Fixes [#1069](https://github.com/max-sixty/worktrunk/issues/1069). ([#1070](https://github.com/max-sixty/worktrunk/pull/1070), thanks @SokiKawashima for reporting)

- **Per-project `[list] timeout` ignored**: The timeout setting from per-project config (`[projects."name".list]`) was not applied — only the global config value was used. ([#1063](https://github.com/max-sixty/worktrunk/pull/1063))

- **Empty repos crash `wt list`**: Repositories with no commits (unborn HEAD) caused errors. Now renders empty cells for commit-dependent fields. ([#1058](https://github.com/max-sixty/worktrunk/pull/1058))

- **Stray blank lines before hints in error output**: Error messages with hints (↳) had an extra blank line separating the hint from its subject. ([#1072](https://github.com/max-sixty/worktrunk/pull/1072))

### Internal

- **Shell escaping consolidation**: Dropped `shlex` crate, consolidated on `shell_escape` across the codebase. ([#1065](https://github.com/max-sixty/worktrunk/pull/1065))

- **CI reviewer improvements**: Resolve review threads, skip trivial re-approvals, default to suggestions. ([#1068](https://github.com/max-sixty/worktrunk/pull/1068))

## 0.24.1

### Improved

- **Template error messages**: Template expansion errors now show what failed, the failing template line, and available variables for undefined variable errors. ([#1041](https://github.com/max-sixty/worktrunk/pull/1041))

- **Interactive picker preview speed**: Preview pre-computation is parallelized via rayon, reducing the chance of a blocking cache miss when switching preview tabs. ([#1048](https://github.com/max-sixty/worktrunk/pull/1048))

- **`wt switch` performance**: Switching to existing worktrees defers path computation, reducing startup latency. ([#1029](https://github.com/max-sixty/worktrunk/pull/1029), [#1030](https://github.com/max-sixty/worktrunk/pull/1030), [#1031](https://github.com/max-sixty/worktrunk/pull/1031))

### Fixed

- **PowerShell wrapper swallows `-D` flag**: The wrapper's `[Parameter(ValueFromRemainingArguments)]` promoted it to an "advanced function", causing PowerShell to consume `-D` as `-Debug` instead of passing it to `wt.exe`. Fixes [#885](https://github.com/max-sixty/worktrunk/issues/885). ([#1057](https://github.com/max-sixty/worktrunk/pull/1057), thanks @DiTo97 for reporting)

- **Nushell shell integration**: Multiple fixes for nushell — auto-detect for install even without vendor/autoload directory ([#1032](https://github.com/max-sixty/worktrunk/pull/1032)), detection checks multiple config paths ([#1038](https://github.com/max-sixty/worktrunk/pull/1038)), uninstall cleans all candidate locations ([#1050](https://github.com/max-sixty/worktrunk/pull/1050)), wrapper hardening and improved diagnostics ([#1059](https://github.com/max-sixty/worktrunk/pull/1059)). (thanks @arnaudlimbourg for [#1032](https://github.com/max-sixty/worktrunk/pull/1032), [#1038](https://github.com/max-sixty/worktrunk/pull/1038), and @omerxx for reporting in [#964](https://github.com/max-sixty/worktrunk/pull/964))

- **Interactive picker leaves screen artifacts**: The picker left visual artifacts after exiting. Fixes [#1027](https://github.com/max-sixty/worktrunk/issues/1027). ([#1028](https://github.com/max-sixty/worktrunk/pull/1028), [#1044](https://github.com/max-sixty/worktrunk/pull/1044), thanks @davidbeesley)

- **Statusline counts files outside sparse checkout cone**: Branch diff statistics in the statusline included files outside the sparse checkout cone, inflating counts. ([#1024](https://github.com/max-sixty/worktrunk/pull/1024), thanks @bendrucker)

- **Template placeholders leak into displayed commands**: `{{ }}` delimiters in hook commands were incorrectly syntax-highlighted, showing ANSI artifacts instead of the template text. ([#1022](https://github.com/max-sixty/worktrunk/pull/1022))

- **Hook announcement trailing colon**: Hook announcements like "Running post-merge project:sync:" had a trailing colon that created visual noise. ([#1025](https://github.com/max-sixty/worktrunk/pull/1025))

- **Blank line after approval prompts**: Approval prompts showed an extra blank line after the user pressed Enter. ([#1040](https://github.com/max-sixty/worktrunk/pull/1040))

### Internal

- **Automated Claude PR review**: Added workflow for automated code review on PRs. ([#1037](https://github.com/max-sixty/worktrunk/pull/1037))

- **Time-to-first-output benchmarks**: Added benchmarks for `remove`, `switch`, and `list` startup latency. ([#1023](https://github.com/max-sixty/worktrunk/pull/1023))

## 0.24.0

### Improved

- **Nushell support (experimental)**: Initial nushell shell integration — shell wrapper, completions, and `wt config shell install` support. This is a proof-of-concept and will need iteration before it's usable; if you're a nushell user feel free to try it and report issues. ([#964](https://github.com/max-sixty/worktrunk/pull/964), thanks @arnaudlimbourg)

- **Version check in `wt config show --full`**: The diagnostics section now queries GitHub for the latest release and shows "Up to date", "Update available", or "Version check unavailable". Gated behind `--full` so normal commands are unaffected. Closes [#1003](https://github.com/max-sixty/worktrunk/issues/1003). ([#1011](https://github.com/max-sixty/worktrunk/pull/1011), thanks @risperdal for requesting)

- **Fish outdated wrapper detection**: `wt config show` now detects when the installed fish shell wrapper has outdated code (e.g., from a previous version) and shows "Outdated shell extension" with a reinstall hint, instead of incorrectly reporting "Not configured". ([#1009](https://github.com/max-sixty/worktrunk/pull/1009))

### Fixed

- **LLM subprocess blocked in Claude Code sessions**: Claude Code sets `CLAUDECODE=1` which blocks nested invocations, breaking `wt step commit` and `wt merge` commit generation. Now strips the env var before spawning the LLM command. ([#1021](https://github.com/max-sixty/worktrunk/pull/1021))

- **Blank line between hint and subject in config show**: The "To configure, run wt config shell install" hint was visually detached from the shell entries it referred to. ([#1007](https://github.com/max-sixty/worktrunk/pull/1007))

### Documentation

- **Status symbol descriptions**: Corrected quick start documentation — `↕` means diverged from default branch (not unpushed commits), `+` means staged changes (not uncommitted changes). ([#1017](https://github.com/max-sixty/worktrunk/pull/1017))

- **Claude Code commit command**: Added `CLAUDECODE` env var unsetting to the Claude Code documentation for commit message generation. ([#1020](https://github.com/max-sixty/worktrunk/pull/1020))

### Internal

- **Environment variable prefix standardization**: Renamed remaining `WT_TEST_*` env vars to `WORKTRUNK_TEST_*`, completing the prefix migration. ([#1016](https://github.com/max-sixty/worktrunk/pull/1016))

- **Plugin metadata**: Aligned plugin description with Cargo.toml tagline ([#1019](https://github.com/max-sixty/worktrunk/pull/1019)), fixed duplicate skills declaration ([#1014](https://github.com/max-sixty/worktrunk/pull/1014), thanks @jacksonblankenship for reporting [#1013](https://github.com/max-sixty/worktrunk/issues/1013)), corrected marketplace source path ([#1012](https://github.com/max-sixty/worktrunk/pull/1012)).

## 0.23.3

### Improved

- **Error display for failed commands**: Failed git commands are now shown in a separate bash-highlighted gutter block instead of inline parenthesized text, making long commands much more readable. ([#1001](https://github.com/max-sixty/worktrunk/pull/1001))

- **PowerShell detection and diagnostics**: Detect PowerShell via `PSModulePath` environment variable so Windows users get "shell requires restart" instead of "not installed". `wt config show` now displays the detected shell and verification hints. Fixes [#885](https://github.com/max-sixty/worktrunk/issues/885). ([#987](https://github.com/max-sixty/worktrunk/pull/987), thanks @DiTo97 for reporting)

### Fixed

- **Fish shell wrapper incompatible with fish < 3.1**: The shell wrapper used `VAR=value command` syntax which requires fish 3.1+. Now uses `env VAR=value ...` for compatibility with all fish versions. Fixes [#999](https://github.com/max-sixty/worktrunk/issues/999). ([#1000](https://github.com/max-sixty/worktrunk/pull/1000), thanks @chrisrickard for reporting)

- **Symlink paths resolved in display messages**: Status messages like "Created worktree @ path" showed canonical paths instead of the user's symlink path. Now consistent with cd directives. Fixes [#968](https://github.com/max-sixty/worktrunk/issues/968). ([#985](https://github.com/max-sixty/worktrunk/pull/985), thanks @brooke-hamilton for reporting)

### Documentation

- **Deduplicated manual shell setup**: Removed duplicated per-shell eval snippets from `wt config --help`, referencing `wt config shell init --help` instead. ([#986](https://github.com/max-sixty/worktrunk/pull/986))

- **PowerShell diagnostic guidance**: Added PowerShell-specific debugging steps to shell integration and troubleshooting references. ([#993](https://github.com/max-sixty/worktrunk/pull/993))

## 0.23.2

### Improved

- **`--force` flag for `wt step copy-ignored`**: Overwrite existing destination files when copying gitignored files to new worktrees. Closes [#971](https://github.com/max-sixty/worktrunk/issues/971). ([#974](https://github.com/max-sixty/worktrunk/pull/974), thanks @williamgoulois for requesting)

### Fixed

- **`wt switch pr:NNNN` / `mr:NNNN` fails in repos without fetch refspecs**: Same-repo PRs and MRs failed with "No branch named X" in single-branch clones or bare repos because fetch didn't create remote tracking branches, and worktree creation relied on DWIM. Now uses explicit refspecs and `-b` fallback. ([#965](https://github.com/max-sixty/worktrunk/pull/965), thanks @andoniaf)

- **Progressive table garbled when output exceeds terminal height**: `wt list` output was corrupted when more lines than the terminal height, because cursor-up commands tried to reach scrolled-off lines. Now detects overflow and falls back to a clean full-table print. ([#981](https://github.com/max-sixty/worktrunk/pull/981))

- **Symlink paths resolved to canonical in cd directives**: When navigating via symlinks, cd directives wrote canonical paths, silently moving users out of their symlink tree. Now preserves the user's logical path. Fixes [#968](https://github.com/max-sixty/worktrunk/issues/968). ([#976](https://github.com/max-sixty/worktrunk/pull/976), thanks @brooke-hamilton for reporting)

- **Terminal artifacts when cancelling interactive picker**: Pressing Esc to cancel the picker left terminal artifacts and a misplaced cursor. Now skim handles cleanup symmetrically for both cancel and accept. ([#984](https://github.com/max-sixty/worktrunk/pull/984))

### Documentation

- **Hook examples: safer port cleanup**: Added `-sTCP:LISTEN` to `lsof` in hook examples to prevent accidentally killing unrelated processes with connections to the port. ([#952](https://github.com/max-sixty/worktrunk/pull/952), thanks @andoniaf)

## 0.23.1

### Improved

- **Interactive picker runs hooks**: `wt switch` without arguments (the interactive picker) now runs post-switch, post-start, and post-create hooks, matching the non-interactive path. ([#942](https://github.com/max-sixty/worktrunk/pull/942))

- **Combined hook output during removal**: Post-remove and post-switch hooks during worktree removal are now shown on a single output line instead of two separate lines. ([#943](https://github.com/max-sixty/worktrunk/pull/943))

### Fixed

- **Shell escape corruption with template filters**: Shell escaping was applied before template rendering, so filters like `sanitize` operated on already-escaped strings, corrupting values with special characters (e.g., apostrophes in branch names). ([#944](https://github.com/max-sixty/worktrunk/pull/944))

- **`wt switch -` history corruption**: `wt switch foo` while already in `foo` would incorrectly record `foo` as the previous branch, breaking `wt switch -` ping-pong. ([#944](https://github.com/max-sixty/worktrunk/pull/944))

- **`--base` without `--create` showed wrong error**: Using `--base` without `--create` could produce misleading errors (e.g., "No previous branch") instead of the expected warning that `--base` requires `--create`. ([#944](https://github.com/max-sixty/worktrunk/pull/944))

## 0.23.0

### Improved

- **Preserve subdirectory position when switching**: `wt switch` now lands in the same subdirectory of the target worktree if it exists, falling back to the root if it doesn't. [Docs](https://worktrunk.dev/switch/) ([#939](https://github.com/max-sixty/worktrunk/pull/939), thanks @frederik-suerig for requesting)

- **`wt switch --no-cd`**: Skip the directory change after switching, useful for scripting or running commands in another worktree without leaving your current shell position. [Docs](https://worktrunk.dev/switch/) ([#932](https://github.com/max-sixty/worktrunk/pull/932), thanks @ArnaudRinquin for requesting)

- **`Alt-c` to create worktree from picker**: In the interactive picker, press `Alt-c` to create a new worktree using the current query as the branch name. ([#933](https://github.com/max-sixty/worktrunk/pull/933))

- **Faster preview tab switching**: Preview tabs (HEAD±, log, main…±, remote⇅) are now pre-computed in a background thread, making tab switching near-instant. ([#935](https://github.com/max-sixty/worktrunk/pull/935))

### Fixed

- **Pager width detection**: Makes preview pane width available to pagers via `$COLUMNS`, so tools like delta can use it for correct side-by-side rendering (e.g., `pager = "delta --width=$COLUMNS"`). Fixes [#924](https://github.com/max-sixty/worktrunk/issues/924). (thanks @tnlanh for reporting) ([#930](https://github.com/max-sixty/worktrunk/pull/930))

- **ANSI style bleeding in preview tabs**: Fixed styling artifacts where dividers appeared emphasized and diffstat lines appeared dim. ([#931](https://github.com/max-sixty/worktrunk/pull/931))

- **URL template expansion with `--skip`**: Skip URL template expansion when `--skip url-status` is used, avoiding unnecessary work. ([#923](https://github.com/max-sixty/worktrunk/pull/923))

- **Hook error consistency**: `wt hook <type>` now errors consistently for all hook types when no hooks are configured, instead of silently succeeding for some types. ([#916](https://github.com/max-sixty/worktrunk/pull/916))

### Documentation

- Improved install instructions in release notes. ([#918](https://github.com/max-sixty/worktrunk/pull/918))

### Internal

- CI: check for existing fix PRs before creating duplicates. ([#922](https://github.com/max-sixty/worktrunk/pull/922))

## 0.22.0

### Improved

- **`wt switch` integrates interactive picker**: `wt switch` without arguments now opens the interactive picker (previously `wt select`). The separate `wt select` command is deprecated with a warning directing users to use `wt switch` instead. Closes [#890](https://github.com/max-sixty/worktrunk/issues/890). (thanks @strangemonad for the suggestion) ([#894](https://github.com/max-sixty/worktrunk/pull/894))

- **TOML syntax highlighting**: Config output from `wt config show` and `wt config shell show-theme` now renders TOML with syntax highlighting (table headers cyan, string values green, comments dimmed). ([#905](https://github.com/max-sixty/worktrunk/pull/905))

- **Bash syntax highlighting improvements**: Multi-line bash commands in hook previews now preserve syntax highlighting across newlines. Wrapped continuation lines are indented with 3 extra spaces to distinguish terminal-forced wraps from actual newlines. ([#906](https://github.com/max-sixty/worktrunk/pull/906))

- **Unified background hook output**: Contiguous post-switch and post-start hooks are now combined into a single output line instead of two separate lines. ([#908](https://github.com/max-sixty/worktrunk/pull/908))

### Documentation

- Removed redundant horizontal rules before H1 headers in documentation pages. ([#909](https://github.com/max-sixty/worktrunk/pull/909))

### Internal

- Updated GitHub Actions and Rust nightly versions. ([#910](https://github.com/max-sixty/worktrunk/pull/910))
- Bumped tree-sitter ecosystem to 0.26 for unified multi-line highlighting. ([#906](https://github.com/max-sixty/worktrunk/pull/906))
- Dependency updates: minijinja 2.15.1, clap, indexmap, ignore, thiserror, time, and others. ([#912](https://github.com/max-sixty/worktrunk/pull/912), [#913](https://github.com/max-sixty/worktrunk/pull/913))

## 0.21.0

### Improved

- **Absolute paths in `worktree-path` templates**: New `{{ repo_path }}` variable enables absolute path configurations like `{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}`. Tilde expansion is also supported (`~/worktrees/{{ repo }}/{{ branch }}`). Fixes [#902](https://github.com/max-sixty/worktrunk/issues/902). (thanks @bingryan for reporting) ([#904](https://github.com/max-sixty/worktrunk/pull/904))

### Documentation

- Documented prefix stripping in `worktree-path` templates using minijinja's built-in `replace` filter and slicing syntax. Closes [#900](https://github.com/max-sixty/worktrunk/issues/900). (thanks @laurentkempe for requesting) ([#903](https://github.com/max-sixty/worktrunk/pull/903))

## 0.20.3

### Fixed

- **PowerShell auto-configuration on Windows**: When running `wt config shell install` from cmd.exe or PowerShell, both PowerShell profile files are now created automatically (Documents/PowerShell and Documents/WindowsPowerShell). Fixes [#885](https://github.com/max-sixty/worktrunk/issues/885). (thanks @DiTo97 for reporting) ([#898](https://github.com/max-sixty/worktrunk/pull/898))

- **`-C` flag respected in hook context**: The `-C` flag now correctly sets the worktree path for hooks, fixing `wt -C /path hook ...` commands that were using the wrong context. ([#899](https://github.com/max-sixty/worktrunk/pull/899))

- **`--config` path validation**: Now warns when `--config` points to a non-existent file instead of silently using defaults. ([#895](https://github.com/max-sixty/worktrunk/pull/895))

### Documentation

- Fix shell quoting in hook examples — template variables are auto-escaped, so manual quoting caused issues with special characters. ([#895](https://github.com/max-sixty/worktrunk/pull/895))

- Updated documentation to use tool-agnostic terminology for LLM commit messages. ([#891](https://github.com/max-sixty/worktrunk/pull/891))

### Internal

- Consolidated PR/MR resolution into unified `remote_ref` module. ([#893](https://github.com/max-sixty/worktrunk/pull/893))

- Simplified command structure and removed dead code. ([#892](https://github.com/max-sixty/worktrunk/pull/892))

- Eliminated Settings types, added accessor methods to Config types. ([#896](https://github.com/max-sixty/worktrunk/pull/896))

## 0.20.2

### Fixed

- **PowerShell shell integration**: Fixed shell integration not working on Windows PowerShell. The init script now includes `| Out-String` to convert array output to a string. Existing configs without this fix are detected as "not installed" so `wt config shell install` will update them automatically. Fixes [#885](https://github.com/max-sixty/worktrunk/issues/885). (thanks @DiTo97 for reporting) ([#888](https://github.com/max-sixty/worktrunk/pull/888))

- **Branch removal message**: "No worktree found for branch X" now shows as info (○) instead of warning (▲) when removing a branch-only, since this is expected behavior. ([#887](https://github.com/max-sixty/worktrunk/pull/887))

### Documentation

- Documented main worktree behavior in `wt step relocate --help`. ([#889](https://github.com/max-sixty/worktrunk/pull/889))

## 0.20.1

### Improved

- **`wt statusline --format=json`**: Output current worktree as JSON (same structure as `wt list --format=json`). Also adds `--format=claude-code` as canonical syntax (the old `--claude-code` flag remains supported). Fixes nested worktree detection that incorrectly identified parent worktrees. ([#875](https://github.com/max-sixty/worktrunk/pull/875))

- **`wt config show` shell status**: Each shell integration line now starts with the shell name (e.g., "bash: Already configured...") for easier scanning. ([#881](https://github.com/max-sixty/worktrunk/pull/881))

- **`wt config show` performance**: 8x faster (~1.2s → ~150ms) by using PATH lookup instead of running `claude --version`. ([#883](https://github.com/max-sixty/worktrunk/pull/883))

### Fixed

- **Config TOML formatting**: Fixed spurious empty `[commit]` header appearing when only `[commit.generation]` is configured. ([#879](https://github.com/max-sixty/worktrunk/pull/879))

- **Documentation URLs**: Fixed broken worktrunk.dev URLs in fish wrapper and config templates. ([#882](https://github.com/max-sixty/worktrunk/pull/882))

### Documentation

- Fixed `worktree-path` example on tips page. ([#876](https://github.com/max-sixty/worktrunk/pull/876), thanks @uriahcarpenter)

- Fixed OSC 8 hyperlink sequences leaking through to web docs as garbage text. ([#870](https://github.com/max-sixty/worktrunk/pull/870))

### Internal

- Demo snapshot mode for regression testing of command output. ([#871](https://github.com/max-sixty/worktrunk/pull/871))

- CI improvements: nextest binary compatibility fix, pinned runner versions, weekly renovation workflow. ([#878](https://github.com/max-sixty/worktrunk/pull/878), [#884](https://github.com/max-sixty/worktrunk/pull/884))

## 0.20.0

### Improved

- **`wt step relocate` command**: Move worktrees to their expected paths based on the `worktree-path` template. Supports `--dry-run` preview, filtering by branch name, and `--commit` to auto-commit dirty worktrees before moving. Handles complex scenarios including worktree swaps (A→B, B→A), chains, and the `--clobber` flag to back up blocking non-worktree paths. [Docs](https://worktrunk.dev/step/) ([#790](https://github.com/max-sixty/worktrunk/pull/790))

- **LLM setup prompt**: First-time interactive prompt when users attempt `wt merge`, `wt step commit`, or `wt step squash` without LLM configuration. Detects available tools (claude, codex) and offers auto-configuration with `?` to preview the generated config. Add `skip-commit-generation-prompt` to user config to suppress. ([#867](https://github.com/max-sixty/worktrunk/pull/867))

- **Consistent prompt styling**: Interactive prompts now use consistent cyan styling via `prompt_message()` formatting. ([#858](https://github.com/max-sixty/worktrunk/pull/858))

### Fixed

- **Path display in error messages**: User-facing paths now consistently use `format_path_for_display()`, fixing cases where raw `.display()` output could show inconsistent path formats. ([#856](https://github.com/max-sixty/worktrunk/pull/856))

### Documentation

- Added Quick Start section to front page showing the switch → list → merge workflow. ([#864](https://github.com/max-sixty/worktrunk/pull/864))
- Updated template documentation: removed deprecated `template-file` options, added `{{ git_diff_stat }}` variable, clarified squash-only variables. ([#854](https://github.com/max-sixty/worktrunk/pull/854))
- Fixed stale documentation for `[commit.generation]` config format, statusline context gauge, and CI status for remote-only branches. ([#853](https://github.com/max-sixty/worktrunk/pull/853))

### Internal

- Bumped nix crate from 0.30.1 to 0.31.1. ([#860](https://github.com/max-sixty/worktrunk/pull/860))
- Refactored deprecation detection for better modularity. ([#852](https://github.com/max-sixty/worktrunk/pull/852))

## 0.19.0

### Improved

- **LLM commit configuration redesign**: The `[commit-generation]` section is now `[commit.generation]`, and `command` + `args` are unified into a single shell-executed `command` string. Existing configs continue to work — a deprecation warning shows the new format and creates a `.new` config file you can apply with `mv`. Claude Code (`claude -p`) and Codex (`codex exec`) are documented as first-class options alongside `llm`. See the [LLM commits guide](https://worktrunk.dev/llm-commits/). ([#809](https://github.com/max-sixty/worktrunk/pull/809), [#837](https://github.com/max-sixty/worktrunk/pull/837))

- **Per-project hooks**: User config can define hooks per-project that append to global hooks. Execution order: global → per-project → project config. Configure under `[projects."owner/repo".hooks]`. ([#842](https://github.com/max-sixty/worktrunk/pull/842))

- **Context window gauge for Claude Code**: Statusline mode shows a moon phase gauge (🌕🌔🌓🌒🌑) for context window usage. ([#840](https://github.com/max-sixty/worktrunk/pull/840))

- **CI status for remote-only branches**: `wt list --remotes` shows CI status for branches that only exist on the remote. ([#817](https://github.com/max-sixty/worktrunk/pull/817))

- **Hook log file lookup**: `wt config state logs get --hook=<spec>` returns the path to a specific hook's log file. ([#816](https://github.com/max-sixty/worktrunk/pull/816), thanks @EduardoSimon for requesting)

- **Branch/fork info in PR/MR display**: `wt switch pr:N` shows the source branch (e.g., `feature-auth`) or fork reference (e.g., `contributor:feature`) alongside PR details. ([#808](https://github.com/max-sixty/worktrunk/pull/808))

- **Claude Code section in `wt config show`**: Shows Claude CLI installation status, plugin status, and statusline configuration. ([#833](https://github.com/max-sixty/worktrunk/pull/833))

- **Deprecation details moved to `wt config show`**: Other commands show a brief pointer instead of full deprecation details. ([#828](https://github.com/max-sixty/worktrunk/pull/828))

- **Config validation suggests correct file**: When a config key belongs in user config but appears in project config (or vice versa), the warning suggests the correct location. ([#804](https://github.com/max-sixty/worktrunk/pull/804))

- **Tilde paths in hints**: Shell command hints use `~` instead of full home directory paths when safe. ([#710](https://github.com/max-sixty/worktrunk/pull/710))

- **Improved `--create` conflict error**: `wt switch --create pr:101` shows the existing branch name in the error. ([#807](https://github.com/max-sixty/worktrunk/pull/807))

- **CI status prioritized in statusline**: CI status is retained longer when the statusline truncates. ([#845](https://github.com/max-sixty/worktrunk/pull/845))

### Fixed

- **Template expansion bugs**: Fixed `worktree_path_of_branch` not respecting shell_escape flag, Windows CI cache rename failures, and `WORKTRUNK_MAX_CONCURRENT_COMMANDS=0` meaning "no limit". ([#847](https://github.com/max-sixty/worktrunk/pull/847), [#849](https://github.com/max-sixty/worktrunk/pull/849))

- **Hook and CI status panics**: Fixed panic when serializing mixed named/unnamed hook configs, banned colons in hook names to prevent parsing ambiguity, and fixed GitLab MR detection when multiple MRs exist without project ID. ([#846](https://github.com/max-sixty/worktrunk/pull/846), [#848](https://github.com/max-sixty/worktrunk/pull/848))

- **Pre-commit hooks for clean worktree squash**: Pre-commit hooks are collected for approval when squashing on a clean worktree. Previously only collected when dirty. ([#695](https://github.com/max-sixty/worktrunk/pull/695))

- **Hint message formatting**: Fixed ANSI escape code interference in dim hint messages. ([#836](https://github.com/max-sixty/worktrunk/pull/836))

- **Spurious [commit] header**: Fixed config migration showing `[commit]` section header when only `commit-generation` fields needed migration. ([#834](https://github.com/max-sixty/worktrunk/pull/834))

### Documentation

- Added at-a-glance examples to config documentation. ([#826](https://github.com/max-sixty/worktrunk/pull/826))
- Clarified user project-specific settings section. ([#835](https://github.com/max-sixty/worktrunk/pull/835))
- Consistent worktree terminology throughout docs. ([#813](https://github.com/max-sixty/worktrunk/pull/813))
- Added tip for monitoring hook logs. ([#838](https://github.com/max-sixty/worktrunk/pull/838))

### Internal

- Replaced manual quote escaping with `shell_escape` crate. ([#810](https://github.com/max-sixty/worktrunk/pull/810))
- Used `sanitize-filename` crate for filename sanitization. ([#832](https://github.com/max-sixty/worktrunk/pull/832))
- Cached CI tool availability checks. ([#831](https://github.com/max-sixty/worktrunk/pull/831))
- Moved inline imports to module top level. ([#818](https://github.com/max-sixty/worktrunk/pull/818), [#819](https://github.com/max-sixty/worktrunk/pull/819), [#820](https://github.com/max-sixty/worktrunk/pull/820), [#822](https://github.com/max-sixty/worktrunk/pull/822))

## 0.18.2

### Improved

- **PR/MR context display**: `wt switch pr:N` and `mr:N` now show PR/MR details (title, author, state, URL) after fetching. ([#782](https://github.com/max-sixty/worktrunk/pull/782))

- **Fork PR branch conflicts**: When a fork PR's branch name conflicts with an existing local branch (e.g., contributor opens PR from their `main`), worktrunk now creates a prefixed branch like `contributor/main` instead of failing. Closes [#714](https://github.com/max-sixty/worktrunk/issues/714). (thanks @vimtor for reporting)

### Fixed

- **Help output formatting**: Fixed double blank lines appearing after demo comments in help output. ([#795](https://github.com/max-sixty/worktrunk/pull/795))

- **Error handling reliability**: Replaced fragile string-based error parsing with structured approaches for git stash, GitHub CLI, and GitLab CLI operations. ([#787](https://github.com/max-sixty/worktrunk/pull/787))

### Documentation

- **ci-status help text**: Improved clarity of the ci-status configuration documentation. ([#794](https://github.com/max-sixty/worktrunk/pull/794))

- **wt remove help text**: Simplified short description and added documentation for `pre-remove` and `post-remove` hooks. ([#792](https://github.com/max-sixty/worktrunk/pull/792))

- **Subcommand documentation**: Fixed generated website docs for subcommands (like `wt step copy-ignored`, `wt config state`) to include their short descriptions. ([#793](https://github.com/max-sixty/worktrunk/pull/793))

## 0.18.1

### Fixed

- **Submodule worktree paths**: Worktrees are now created in the correct location when running inside a git submodule. Previously, worktrees were created relative to the parent repo's `.git/modules/` directory instead of the submodule's working directory. ([#762](https://github.com/max-sixty/worktrunk/pull/762), thanks @lajarre; [#777](https://github.com/max-sixty/worktrunk/issues/777), thanks @mhonsel for reporting)
- **Shell integration warnings**: Warnings about shell integration now check if the *current* shell has integration configured, not whether *any* shell does. This fixes misleading "shell requires restart" messages when e.g. bash had integration but the user was running fish. ([#772](https://github.com/max-sixty/worktrunk/pull/772))
- **"Not found" error messages**: Improved error message phrasing — "No branch named X" instead of "Branch X not found", "Branch X has no worktree" instead of "No worktree found for branch X". Context-appropriate hints now appear (e.g., `wt remove` no longer suggests `--create`). ([#774](https://github.com/max-sixty/worktrunk/pull/774))

### Internal

- Unified PR/MR reference resolution, reducing code duplication. ([#778](https://github.com/max-sixty/worktrunk/pull/778))

## 0.18.0

### Improved

- **Post-remove hook**: New hook type runs after worktree removal. Template variables (`{{ branch }}`, `{{ worktree_path }}`, `{{ commit }}`) reference the removed worktree, enabling cleanup scripts for containers, servers, or other resources. ([#757](https://github.com/max-sixty/worktrunk/pull/757))
- **Graceful handling of missing worktree directories**: `wt remove` now prunes stale git metadata when the worktree directory was deleted externally (e.g., `rm -rf`), making the command more idempotent. Fixes [#724](https://github.com/max-sixty/worktrunk/issues/724). (thanks @strangemonad for reporting)
- **Config validation warnings at load time**: Unknown fields in config files (typos like `[commit-gen]` instead of `[commit-generation]`) now show warnings immediately instead of only in `wt config show`. ([#758](https://github.com/max-sixty/worktrunk/pull/758))

### Fixed

- **Age column shows "future" on NixOS/direnv**: `wt list` no longer uses `SOURCE_DATE_EPOCH` for time calculations, which NixOS and direnv commonly set to past timestamps for reproducible builds. Fixes [#763](https://github.com/max-sixty/worktrunk/issues/763). (thanks @ngotchac for reporting)
- **CI status with URL-based pushremote**: CI detection now works when `branch.<name>.pushremote` is set to a URL directly (as `gh pr checkout` does) instead of a remote name. ([#769](https://github.com/max-sixty/worktrunk/pull/769))
- **GitLab nested groups in URL parsing**: URLs like `gitlab.com/group/subgroup/repo` now correctly identify `repo` as the repository name instead of `subgroup`. This was a security fix — previously, approval bypass was possible across sibling repos in the same parent group. ([#768](https://github.com/max-sixty/worktrunk/pull/768))
- **GitLab CI status detection**: Fixed multiple issues with `glab` CLI compatibility — MR lookup now uses two-step resolution, "manual" pipelines show as running instead of failed, and rate limit errors are handled properly. Fixes [#764](https://github.com/max-sixty/worktrunk/issues/764). (thanks @ngotchac for reporting)

### Internal

- Refactored accessor functions to use bare nouns per Rust convention. ([#765](https://github.com/max-sixty/worktrunk/pull/765))
- Clarified target/integration naming across codebase. ([#755](https://github.com/max-sixty/worktrunk/pull/755))

## 0.17.0

### Improved

- **Per-project config overrides** (Experimental): Override settings per-project in user config. Supports `worktree-path`, `commit-generation`, `list`, `commit`, and `merge` sections. Config precedence: CLI arg > project config > global config > default. Closes [#596](https://github.com/max-sixty/worktrunk/issues/596). ([#749](https://github.com/max-sixty/worktrunk/pull/749))
- **Search all remotes for branch existence**: Branch existence checks and completions now search all remotes instead of just the primary remote, matching git's behavior. When a branch exists on multiple remotes, completions show all of them (e.g., `feature ⇣ 2d origin, upstream`). ([#744](https://github.com/max-sixty/worktrunk/pull/744))
- **CI detection for fork workflows**: CI status detection now searches all remotes and uses `gh config get git_protocol` / `glab config get git_protocol` for fork URL protocol preference instead of inferring from existing remotes. ([#753](https://github.com/max-sixty/worktrunk/pull/753))

### Fixed

- **Same-repo PR switching with stale refs**: `wt switch pr:N` for same-repo PRs now fetches the branch before validation, fixing "Branch not found" errors when local refs were stale. ([#742](https://github.com/max-sixty/worktrunk/pull/742))
- **Project identifier collision for repos without remotes**: Repos without remotes now use their full canonical path as the project identifier instead of just the directory name, preventing approval collisions between unrelated repos (e.g., `~/work/myproject` vs `~/personal/myproject`). Users with remoteless repos will need to re-approve commands. ([#747](https://github.com/max-sixty/worktrunk/pull/747))

### Internal

- Cross-platform path handling improvements using `path-slash` crate and `Path::file_name()`. ([#750](https://github.com/max-sixty/worktrunk/pull/750))
- Renamed `WorktrunkConfig` to `UserConfig` internally. ([#746](https://github.com/max-sixty/worktrunk/pull/746))

## 0.16.0

### Improved

- **Background hook verbosity**: Background hooks (post-start, post-switch) now show a single-line summary by default instead of per-hook output. Use `-v` to see detailed output with expanded commands. We're open to feedback on this change — let us know in [#690](https://github.com/max-sixty/worktrunk/issues/690). (thanks @clutchski for reporting)

### Internal

- Fixed dead Apple documentation link in copy-ignored rationale. ([#743](https://github.com/max-sixty/worktrunk/pull/743))

## 0.15.5

### Fixed

- **Hook execution order**: Hooks now run in the order defined in the config file. Previously, HashMap iteration randomized the order. Fixes [#737](https://github.com/max-sixty/worktrunk/issues/737). (thanks @ngotchac for reporting)

## 0.15.4

### Improved

- **Git progress for slow worktree creation**: When `git worktree add` takes more than 400ms (common on large repos), worktrunk now shows a progress message and streams git's output instead of going silent. ([#725](https://github.com/max-sixty/worktrunk/pull/725))
- **Verbose template expansion output**: `-v` now shows template expansion details: the template, expanded command, and any undefined variables with SemiStrict fallback behavior. ([#712](https://github.com/max-sixty/worktrunk/pull/712))
- **Shell integration hint for explicit path invocation**: When running wt via explicit path (e.g., `./target/debug/wt`) with shell integration configured, the warning now suggests running `wt switch <branch>` to use the shell-wrapped command. ([#721](https://github.com/max-sixty/worktrunk/pull/721))

### Fixed

- **Unsafe upstream when creating branch from remote base**: `wt switch --create feature --base=origin/main` no longer sets up tracking to origin/main, preventing accidental pushes to the base branch. Fixes [#713](https://github.com/max-sixty/worktrunk/issues/713). (thanks @kfirba)
- **Credential redaction in debug logs**: URLs with embedded credentials (e.g., `https://token@github.com/...`) are now redacted in `-vv` debug output. ([#718](https://github.com/max-sixty/worktrunk/pull/718))
- **Hook preview shows template on expansion failure**: `wt hook show --expanded` now displays both the error message and original template when expansion fails, instead of hiding the template. ([#722](https://github.com/max-sixty/worktrunk/pull/722))

### Documentation

- **Homebrew install uses core tap**: Install command updated from `max-sixty/worktrunk/wt` to `worktrunk`. ([#716](https://github.com/max-sixty/worktrunk/pull/716), thanks @chenrui333)
- **Hook docs reordered**: post-start (background) is now the recommended default, with post-create for blocking dependencies. ([#733](https://github.com/max-sixty/worktrunk/pull/733))

### Internal

- Simplified GitHub/GitLab CI status detection. ([#730](https://github.com/max-sixty/worktrunk/pull/730))
- Previous worktree gutter changed from `-` to `+` for visual consistency. ([#699](https://github.com/max-sixty/worktrunk/pull/699))

## 0.15.3

### Fixed

- **`--execute` command display**: Shows the expanded command in a gutter with path context instead of showing the raw template before expansion. ([#708](https://github.com/max-sixty/worktrunk/pull/708))
- **CRLF line endings in error display**: Multiline errors with Windows (`\r\n`) or old Mac (`\r`) line endings now display correctly instead of falling through to single-line handling. ([#707](https://github.com/max-sixty/worktrunk/pull/707))

### Documentation

- **Arch Linux install via AUR**: Added installation instructions and shell integration command. ([#709](https://github.com/max-sixty/worktrunk/pull/709), [#561](https://github.com/max-sixty/worktrunk/pull/561), thanks @razor-x)

## 0.15.2

### Improved

- **`wt config shell completions <shell>`**: Generate static shell completion scripts for package managers and custom installation. ([#701](https://github.com/max-sixty/worktrunk/pull/701), thanks @chenrui333)
- **Debug logging threshold**: Now requires `-vv` instead of `-v` for debug logging and diagnostic file generation, freeing `-v` for future use. ([#702](https://github.com/max-sixty/worktrunk/pull/702))

### Fixed

- **Fork PR fetching**: `wt switch pr:N` now works when `origin` points to a fork by fetching PR refs from the upstream remote. Shows actionable error with `git remote add` command if upstream remote is missing. ([#704](https://github.com/max-sixty/worktrunk/pull/704))
- **Fork PR branch naming**: Fork PR branches now use the original branch name (e.g., `feature-fix`) instead of `owner/feature-fix`, so `git push` works correctly. ([#706](https://github.com/max-sixty/worktrunk/pull/706))
- **Config race conditions**: File locking prevents corruption when multiple `wt` processes modify config simultaneously. ([#693](https://github.com/max-sixty/worktrunk/pull/693))
- **Nested worktree detection**: Current worktree indicator (`@`) now shows on the correct worktree when worktrees are nested (e.g., `.worktrees/` layout inside repo). ([#697](https://github.com/max-sixty/worktrunk/pull/697))
- **Symlink path resolution**: Worktree commands work correctly on systems with symlinks (e.g., macOS `/var` → `/private/var`). ([#696](https://github.com/max-sixty/worktrunk/pull/696))
- **Pre-remove hook failures**: Shell no longer cd's to main worktree when pre-remove hooks fail, leaving user in their current location. ([#692](https://github.com/max-sixty/worktrunk/pull/692))
- **PowerShell completion robustness**: Completion registration errors no longer break the shell wrapper function. ([#674](https://github.com/max-sixty/worktrunk/pull/674))

### Documentation

- Added missing `orphan` (`∅`) symbol and `no_worktree` state to JSON output documentation. ([#687](https://github.com/max-sixty/worktrunk/pull/687))
- Clarified Unicode handling in shell detection. ([#700](https://github.com/max-sixty/worktrunk/pull/700))

### Internal

- Refactored large files into focused modules. ([#688](https://github.com/max-sixty/worktrunk/pull/688))
- Consolidated integration reason computation into Repository method. ([#689](https://github.com/max-sixty/worktrunk/pull/689))
- Added verbose level tracking infrastructure for future `-v` output. ([#703](https://github.com/max-sixty/worktrunk/pull/703))
- PowerShell template uses `WORKTRUNK_BIN` for test isolation. ([#674](https://github.com/max-sixty/worktrunk/pull/674))

## 0.15.1

### Improved

- **`wt config show` diagnostics**: When shell integration is not active, now shows how the command was invoked, the binary path (if different), and `$SHELL` environment variable. Helps diagnose setup issues. ([#683](https://github.com/max-sixty/worktrunk/pull/683))
- **Help pager follows git convention**: `-h` never opens a pager, `--help` uses pager when available. Closes [#583](https://github.com/max-sixty/worktrunk/issues/583). ([#651](https://github.com/max-sixty/worktrunk/pull/651), thanks @razor-x)
- **Verbose mode logging**: `-v` now logs command stdout/stderr and all spawned processes including background hooks, `wt for-each` commands, and shell probes. ([#680](https://github.com/max-sixty/worktrunk/pull/680))

### Documentation

- **FAQ reordered**: Questions now ordered by frequency and importance.

### Internal

- **AUR package**: Worktrunk now published to Arch Linux AUR on each release. ([#585](https://github.com/max-sixty/worktrunk/pull/585), thanks @razor-x)
- **Codecov Test Analytics**: Integration tests now report to Codecov Test Analytics. ([#682](https://github.com/max-sixty/worktrunk/pull/682))

## 0.15.0

### Improved

- **`wt switch pr:<number>` syntax** (experimental): Switch directly to a GitHub PR by number. Same-repo PRs delegate to normal switch flow; fork PRs fetch from refs/pull/N/head and configure pushRemote. ([#673](https://github.com/max-sixty/worktrunk/pull/673), closes [#657](https://github.com/max-sixty/worktrunk/issues/657), thanks @wladpaiva for requesting)
- **`--force` hint for dirty worktrees**: When `wt remove` fails due to uncommitted changes, the hint now shows the full command: `wt remove <branch> --force`. ([#671](https://github.com/max-sixty/worktrunk/pull/671))

### Documentation

- **Windows install guidance**: Winget as recommended install (ships `git-wt` by default), plus the App Execution Aliases workaround to use `wt` directly. Closes [#133](https://github.com/max-sixty/worktrunk/issues/133). (thanks @ctolkien for reporting, @shanselman for the aliases tip, @Farley-Chen for [#648](https://github.com/max-sixty/worktrunk/pull/648))
- **Caddy subdomain routing pattern**: Clean URLs like `feature-auth.myproject.lvh.me` via Caddy reverse proxy with dynamic route registration.
- **tmux session per worktree pattern**: Dedicated tmux session with multi-pane layout per worktree.

## 0.14.2

### Fixed

- **`wt remove --force` works with dirty worktrees**: The `--force` flag was documented to allow removal with uncommitted changes, but worktrunk's own cleanliness check blocked it before git could apply the flag. Fixes [#658](https://github.com/max-sixty/worktrunk/issues/658). (thanks @pedro93)
- **Correct output when switching to existing local branch**: When switching to a local branch that tracks a remote, worktrunk incorrectly reported "Created branch X" instead of "Created worktree for X". Now only reports branch creation when git's DWIM actually creates a new tracking branch from a remote. Fixes [#656](https://github.com/max-sixty/worktrunk/issues/656). (thanks @guidupuy-ws)
- **PowerShell handles multiple `wt.exe` binaries**: On Windows, when both Windows Terminal's `wt.exe` and worktrunk's `wt.exe` exist in PATH, shell integration errored with "Cannot convert 'System.Object[]' to the type 'System.String'". Now correctly uses the first match. Relates to [#648](https://github.com/max-sixty/worktrunk/issues/648). (thanks @Farley-Chen)

## 0.14.1

### Improved

- **`--base` accepts commit-ish refs**: `wt switch --create --base` now accepts HEAD, tags, commit SHAs, and relative refs (e.g., `HEAD~2`), not just branch names. Fixes [#630](https://github.com/max-sixty/worktrunk/issues/630). (thanks @myhau)
- **Upfront validation for target refs**: `wt merge` and `wt step` commands now validate target refs before approval prompts, giving clearer "Branch X not found" errors immediately.
- **Visual hierarchy in help**: Section dividers, improved heading structure, and sentence case in `--help` output.

### Fixed

- **macOS shell freeze during `copy-ignored`**: Atomic `clonefile()` on directories saturated disk I/O, blocking shell startup. Now uses per-file reflink which is slower but keeps the system responsive.
- **`copy-ignored` no longer copies nested worktrees**: When `worktree-path` places worktrees inside the main worktree, `copy-ignored` now skips them. Also now copies symlinks (fixes `node_modules/.bin/` etc.). Fixes [#641](https://github.com/max-sixty/worktrunk/issues/641). (thanks @razor-x)
- **Context-aware hints for `wt config create`**: Hints now suggest relevant next steps based on which configs exist.

## 0.14.0

### Improved

- **`worktree_path_of_branch(branch)` template function**: Look up the filesystem path of any branch's worktree in hooks. Enables copying files between worktrees: `setup = "cp {{ worktree_path_of_branch('main') }}/config.local {{ worktree_path }}"`. Returns empty string if no worktree exists for the branch.
- **Per-task timeout for `wt list`**: Configure timeout for git operations via `[list] timeout-ms` in user config. Shows timeout count in footer. Use `--full` to disable timeout for complete data collection.
- **Atomic COW directory cloning on macOS**: `wt step copy-ignored` uses `clonefile()` syscall on APFS for O(1) directory cloning instead of file-by-file copying. ~12-15x faster for large directories like `target/`.
- **Template variable renamed**: `main_worktree_path` → `primary_worktree_path` for clarity. Old name still works with deprecation warning.

### Fixed

- **`wt step copy-ignored` in bare repositories**: Fixed "this operation must be run in a work tree" error when using bare repo setups. Closes [#598](https://github.com/max-sixty/worktrunk/issues/598). (thanks @sbennett33 for reporting)

### Internal

- **Help system extraction**: Moved help and invocation utilities from main.rs to dedicated modules.
- **`wt list` model refactor**: Split monolithic model.rs into modular directory structure.

## 0.13.4

### Fixed

- **LESS flag concatenation with long options**: Fixed "invalid option" error when users have long options in LESS (e.g., `LESS=--mouse`). The pager auto-quit feature from v0.13.1 now correctly separates flags. Fixes [#594](https://github.com/max-sixty/worktrunk/issues/594). (thanks @tnlanh for reporting)

### Internal

- **Homebrew formula generation**: Release workflow now uses cargo-dist for Homebrew formula generation, simplifying the release process.

## 0.13.2

### Improved

- **Validate before approval prompts**: `wt switch` and `wt remove` now validate operations before prompting for hook approval, so users don't approve hooks for operations that will fail.

### Fixed

- **Homebrew formula SHA256 hashes**: Fixed release workflow that was setting incorrect checksums for Intel and Linux binaries, causing `brew install` to fail. Fixes [#589](https://github.com/max-sixty/worktrunk/issues/589). (thanks @kobrigo for reporting)

## 0.13.1

### Fixed

- **Pager auto-quit**: Help text now auto-quits when it fits on screen, even when `LESS` is set without the `F` flag (common with oh-my-zsh's `LESS=-R` default). Fixes [#583](https://github.com/max-sixty/worktrunk/issues/583). (thanks @razor-x for reporting)
- **`--create` hint for remote branch shadowing**: Improved recovery hint when `--create` shadows a remote branch — now shows the full recovery command.

## 0.13.0

### Improved

- **`wt list` parallelization improvements**: Better parallelization of worktree operations reduce latency in some conditions. Respects `RAYON_NUM_THREADS` environment variable for controlling parallelism.
- **Template variables in `--execute`**: Hook template variables (`{{ branch }}`, `{{ worktree_path }}`, etc.) are now expanded in `--execute` commands and trailing args. With `--create`, `{{ base }}` and `{{ base_worktree_path }}` are also available.
- **Fish shell Homebrew compatibility**: Fish shell integration now installs to `~/.config/fish/functions/wt.fish` instead of `conf.d/`, ensuring PATH is fully configured before the wt function loads. `wt config show` detects legacy installations and `wt config shell install` handles migration automatically. ([#586](https://github.com/max-sixty/worktrunk/issues/586) — thanks @ekans & @itzlambda)
- **Chrome Trace Format export**: Performance traces can be exported for analysis with Chrome's trace viewer or Perfetto.
- **`--dry-run` flag for shell commands**: `wt config shell install` and `wt config shell uninstall` now support `--dry-run` to preview changes without prompting.
- **Nested subcommand suggestions**: When typing `wt squash` instead of `wt step squash`, the error now suggests the correct command path.
- **Orphan branch indicator**: `wt list` shows `∅` (empty set) for orphan branches with no common ancestor to the default branch.
- **Improved `-vv` diagnostic workflow**: Bug reporting hint now uses a gist workflow to avoid URL length limits.

### Fixed

- **`wt switch --create --base` error message**: Now correctly identifies the invalid base branch instead of the target branch. Fixes [#562](https://github.com/max-sixty/worktrunk/issues/562). (thanks @fablefactor)
- **AheadBehind column loading indicator**: Shows `⋯` when not yet loaded instead of appearing empty, distinguishing loading state from "in sync".
- **Post-merge hook failure output**: Simplified error messages and removed confusing `--no-verify` hint.
- **`wt select` log preview**: Graph structure is now preserved when displaying commit history, and columns dynamically align.

### Documentation

- **FAQ entry for shell setup issues**: Added troubleshooting guidance for common shell integration problems.
- **Template variables reference**: Consolidated template variables documentation into hook.md.
- **Clarified `--force` vs `-D` flags**: Updated `wt remove` documentation. (thanks @hlee-cb)
- **Performance benchmarks**: Added documentation for `copy-ignored` performance.

## 0.12.0

### Improved

- **`wt select --branches` and `--remotes` flags**: Control which items appear in the selection UI. Shares the `[list]` config section with `wt list` for consistent defaults.
- **Graceful degradation when default branch unavailable**: When the default branch cannot be determined (e.g., misconfigured), `wt list` shows warnings and empty cells rather than failing. `wt switch --create` without `--base` gives a clear error message.
- **Remove `--refresh` flag from state commands**: `wt config state default-branch get` and `wt config state ci-status get` now purely read cached state. To force re-detection, use the explicit workflow: `clear` then `get`. (Breaking: `--refresh` flag removed)
- **Windows: Require Git for Windows**: Removed PowerShell fallback. Worktrunk now requires Git for Windows (Git Bash) and shows a clear error message pointing to the download page if not found. (Breaking: PowerShell no longer supported)

### Fixed

- **Flag styling in messages**: Flags like `--clobber` and `--no-verify` in parentheses now inherit message color instead of using bright-black styling.
- **Nix flake**: Remove apple_sdk framework dependency. ([#525](https://github.com/max-sixty/worktrunk/pull/525), thanks @MattiasMTS)
- **`gh issue create` hint**: Now includes `--web` flag to open the issue form in browser.

### Internal

- **Binary size reduced ~1MB**: Trimmed unused config/minijinja features (13MB → 12MB).
- **Repository module split**: Split 2200-line module into 8 focused submodules for maintainability.

## 0.11.0

### Improved

- **Nix flake for packaging**: New `flake.nix` for Nix users with crane for efficient Rust builds. ([#502](https://github.com/max-sixty/worktrunk/pull/502), thanks @marktoda; thanks @Kabilan108 for requesting)
- **`sanitize_db` template filter**: New filter that transforms strings into database-safe identifiers with a 3-character hash suffix for collision/keyword safety. ([#498](https://github.com/max-sixty/worktrunk/pull/498), thanks @hugobarauna for requesting)
- **`wt select` performance**: 500ms timeout for git commands improves TUI responsiveness on large repos with many branches. (thanks @KidkArolis for reporting [#461](https://github.com/max-sixty/worktrunk/issues/461))
- **`wt select` stale branch handling**: Branches 50+ commits behind the default branch now skip expensive operations, showing `...` in the diff column. Improves performance on repos with many stale branches.
- **Global merge-base cache**: Cached merge-base results improve `wt list` performance by avoiding redundant git calls.
- **`wt config show` git version**: Now displays the git version alongside the worktrunk version.
- **`wt step copy-ignored` default**: Now copies all gitignored files by default. Use `.worktreeinclude` to limit what gets copied (previously required `.worktreeinclude` to specify what to copy).
- **Trace log analysis**: New `analyze-trace` binary for analyzing `[wt-trace]` performance logs.

### Fixed

- **Statusline truncation**: No longer truncates when terminal width is unknown, fixing Claude Code statusline display.
- **Shell completions**: Deprecated args like `--no-background` no longer appear in tab completions.
- **`wt remove` progress ordering**: Progress message now appears after pre-remove hooks, not before.
- **`wt list` index lock**: Uses `--no-optional-locks` for git status to avoid lock contention with parallel tasks.

## 0.10.0

### Improved

- **`wt step copy-ignored`**: Copy gitignored files listed in `.worktreeinclude` between worktrees. Useful for syncing `.env` files, IDE settings, and build caches to new worktrees via post-create hooks. Uses COW (reflink) copying for efficient handling of large directories. Matches Claude Code Desktop's worktree file syncing behavior.
- **`--foreground` flag**: Debug background hooks by running them in the foreground. Available on `wt hook post-start`, `wt hook post-switch`, and `wt remove`. Replaces the deprecated `--no-background` flag.
- **`--var` flag for hooks**: Override template variables when running hooks manually, e.g., `wt hook post-create --var target=main`.
- **`ci.platform` config**: Explicitly set CI platform (`github` or `gitlab`) for GitHub Enterprise or self-hosted GitLab where URL-based detection fails.
- **Upstream diff in `wt select`**: Tab 4 shows ahead/behind diff vs upstream tracking branch (remote⇅), matching the column in `wt list`.
- **`{{ base }}` and `{{ base_worktree_path }}` variables**: New template variables for creation hooks (post-create, post-start, post-switch) to access the base branch name and worktree path.
- **`-vv` diagnostic reports**: Double-verbose flag writes a diagnostic report to `.git/wt-logs/diagnostic.md` with environment info, configs, and logs for easy bug reporting.

### Fixed

- **Warning ordering**: Warnings about state discovered during evaluation now appear before the action message, making them feel like considered observations rather than afterthoughts.
- **Config validation in `wt config show`**: Now validates TOML syntax and schema, displaying parse errors with details.

### Documentation

- **Undocumented features**: Added documentation for `--show-prompt` and `--stage` flags on `wt step commit/squash`, `skip-shell-integration-prompt` config, and `[select] pager` config.

## 0.9.5

### Improved

- **Pager config for `wt select`**: New `[select] pager` config option to customize the diff pager in `wt select` previews. Auto-detects delta/bat when not configured.
- **Infinity symbol for extreme diffs**: `wt list` shows `∞` instead of `9K` for diffs >= 10,000 commits, avoiding misleading values.

### Fixed

- **Windows shell integration message**: Warning now shows just the command name instead of the full absolute path, and gives targeted advice when only the `.exe` suffix differs.
- **URL column width**: Column width in `wt list` now accounts for hyperlink display showing just `:PORT` instead of full URLs.

### Internal

- **Deprecated `template-file` and `squash-template-file`**: Legacy LLM template config options now show deprecation warnings.
- **Path handling improvements**: Replaced string manipulation with proper Path/PathBuf stdlib methods throughout the codebase.

## 0.9.4

### Improved

- **Diagnostic report generation**: `wt list --verbose` generates diagnostic reports (`.git/wt-logs/diagnostic.md`) when warnings or errors occur, with a `gh issue create` command hint when GitHub CLI is available.
- **Alias bypass detection**: `wt config show` detects shell aliases that point to binary paths (e.g., `alias gwt="/usr/bin/wt"`) and warns that they bypass shell integration with suggested fixes.
- **Switch message clarity**: Messages now explicitly state what was created — "Created branch X and worktree" vs "Created worktree for X" vs "Switched to worktree for X".
- **Worktree-path hint**: One-time hint after first `wt switch --create` suggesting `wt config create` to customize worktree locations.
- **Path mismatch warnings**: `wt remove` and `wt merge` show warnings when worktree paths don't match the config template.
- **CLI command ordering**: Commands reordered by usage frequency in `--help` (switch, list, remove, merge...).

### Fixed

- **Progress counter overflow**: Fixed `wt list` progressive rendering when URL sends caused completed count to exceed expected count.
- **Windows shell integration**: Shell function now correctly strips `.exe` suffix, relying on MSYS2/Git Bash automatic resolution (fixes [#348](https://github.com/max-sixty/worktrunk/issues/348)).
- **Prunable worktrees**: Gracefully handle worktrees where the directory was deleted but git still tracks metadata.
- **Help text tables**: Disabled clap text wrapping to preserve markdown tables in `--help` output.

### Documentation

- **FAQ entries**: Added entries for "What files does Worktrunk create?" and "What can Worktrunk delete?".

### Internal

- **Hint state management**: New `wt config state hints` subcommand for viewing and clearing shown hints.
- **Deprecated config deduplication**: Migration files (`.new`) only written once per repo, tracked via git config hints.

## 0.9.3

### Improved

- **Terminal hyperlinks for URLs**: The URL column in `wt list` now shows clickable links (OSC 8) in supported terminals, displaying a compact `:port` that links to the full URL.
- **Statusline truncation**: Statusline output now intelligently truncates by dropping low-priority segments (URL, CI) before high-priority ones (branch, model) when exceeding terminal width.
- **Statusline URL**: When a project has a `[list] url` template configured, the URL now appears in statusline output for shell prompts.
- **Bare repo default branch detection**: Uses `symbolic-ref HEAD` as a heuristic for detecting the default branch in bare repos and empty repos before the first commit.
- **Terminology**: Renamed "path mismatch" to "branch-worktree mismatch" for clarity. In JSON output (`wt list --format=json`), the field `path_mismatch` is now `branch_worktree_mismatch`.

### Fixed

- **Empty bare repo bootstrap**: `wt switch --create main` now works in empty bare repos by handling unborn branches correctly.

### Documentation

- **CLI help text**: Improved descriptions across multiple commands including `wt`, `wt list`, `wt select`, `wt step`, `wt merge`, `wt remove`, and `wt hook`.
- **Web docs copy button**: Fixed copy button position so it stays at top-right when scrolling horizontally through code blocks.

### Internal

- **Claude Code plugin detection**: `wt config show` now displays whether the worktrunk Claude Code plugin is installed, with install hints if needed.
- **Hyperlink diagnostics**: `wt config show` shows hyperlink support status (active/inactive).

## 0.9.2

### Fixed

- **Locked worktree detection**: `wt remove` now detects locked worktrees upfront and shows a clear error with unlock instructions, instead of reporting success but silently failing. ([#408](https://github.com/max-sixty/worktrunk/pull/408), [#412](https://github.com/max-sixty/worktrunk/pull/412))
- **Windows Git Bash shell integration**: Shell detection now handles Windows-style paths in `$SHELL` (e.g., `C:\Program Files\Git\usr\bin\bash.exe`). Fixes [#348](https://github.com/max-sixty/worktrunk/issues/348). ([#398](https://github.com/max-sixty/worktrunk/pull/398))

### Documentation

- **CLI help text clarity**: Improved descriptions for `wt`, `wt list`, `wt step push`, `wt step squash`, `wt remove`, and `wt config state`. ([#410](https://github.com/max-sixty/worktrunk/pull/410))
- **Installation commands**: Removed `$` prefixes from install commands for easier copy-paste. ([#405](https://github.com/max-sixty/worktrunk/pull/405), thanks @muzzlol)

### Internal

- **Home worktree lookup**: Centralized with `find_home()` and `home_path()` methods for more consistent behavior with bare repos.
- **Windows CI**: Added cross-platform mock infrastructure for testing Windows-specific behavior.

## 0.9.1

### Improved

- **Shell integration debug info**: `wt config show` now displays invocation details (path, git subcommand mode, explicit path usage) to help diagnose shell integration issues. "Shell integration not active" is now a warning instead of a hint.

## 0.9.0

### Improved

- **Shell integration prompt**: When shell integration isn't active after `wt switch`, an interactive prompt offers to install it. The prompt remembers your choice and falls back to a hint for non-TTY environments.
- **Template variable names**: Renamed for clarity: `repo_root` → `repo_path`, `worktree` → `worktree_path`, `main_worktree` → `repo`. Added `main_worktree_path` for accessing the main worktree's absolute path. Deprecated names work with migration warnings and auto-generated `.new` config files.
- **Shell integration warnings**: Specific diagnostic messages when shell cd won't work: "shell integration not installed", "shell requires restart", "ran ./wt; shell integration wraps wt", or "ran git wt; running through git prevents cd".
- **RUNTIME section in `wt config show`**: Displays binary name, version, and shell integration status to help debug invocation issues.
- **Clickable CI indicator**: The CI status indicator (●) in `wt list` output is now a clickable link to the PR in terminals that support OSC 8 hyperlinks.
- **`wt switch` help text**: Clarifies the difference from `git switch` and documents common failure conditions.

### Fixed

- **Hook path display**: Hook announcements show the execution path when shell integration isn't active.
- **Approval matching with deprecated vars**: Approvals now match regardless of whether they were saved with deprecated or current variable names.
- **Documentation filter syntax**: Fixed incorrect Jinja filter examples that showed `~` concatenation with `|` filter without parentheses. ([#373](https://github.com/max-sixty/worktrunk/pull/373), thanks @coriocactus)

### Documentation

- **Pre-remove hook example**: Added pattern for cleaning up background processes (e.g., killing dev servers) when worktrees are removed.

## 0.8.5

### Improved

- **Windows `git-wt` command**: Winget now ships with `git-wt` as a workaround to the Windows Terminal `wt` naming conflict. We're still considering better options — see [#133](https://github.com/max-sixty/worktrunk/issues/133).

## 0.8.4

### Improved

- **Shell integration detection**: More robust detection of `git wt` (space) vs `git-wt` patterns. `wt config show` now displays line numbers for detected shell integration.
- **Windows `wt select` error**: Shows a helpful error message with alternatives instead of "unrecognized subcommand".

### Fixed

- **Markdown table rendering**: Escaped pipe characters (`\|`) in help output now render correctly.
- **Dim styling on wrapped lines**: Dim text attribute now preserved on continuation lines when text wraps.
- **Path occupied hint**: Fixed tilde expansion issue where `~/...` paths didn't work in shell commands.

### Documentation

- **Hook design guide**: Added comprehensive guide for designing hooks.
- **Command docs**: Added `wt config show` to command documentation.
- **Windows paths**: Documented MSYS2 auto path conversion for Windows shell integration.

### Internal

- **Output system**: Consolidated output functions, removed redundant aliases.
- **Zsh compinit**: Improved handling of "insecure directories" warning in tests.

## 0.8.3

### Improved

- **Hook execution path**: Shows the execution path when post-merge hooks run in a different directory than where the user invoked the command (e.g., with `--no-remove`).
- **TTY check for `wt select`**: Now fails gracefully when run in a non-interactive terminal instead of hanging.
- **Background hooks**: `post-start` and `post-switch` hooks spawn in background via stdin piping, matching their normal behavior during `wt switch`.
- **Occupied path error message**: When a worktree path is occupied by a different branch, the error now explains the situation clearly and suggests `git switch`.
- **Shell integration hint**: Shows a hint to restart the shell when shell integration is configured but not active.
- **Message style**: Removed 2nd person pronouns ("you/your") from user-facing messages following CLI guidelines.

### Fixed

- **`wt hook post-start` blocking**: Fixed bug where `wt hook post-start` ran in foreground blocking the command, instead of spawning in background like during normal `wt switch --create`.
- **Approval bypass with `project:` prefix**: Fixed security issue where using `project:` filter prefix (e.g., `wt hook pre-merge project:`) bypassed the approval check, allowing unapproved project commands to run.

### Documentation

- **License file**: Added combined MIT and Apache-2.0 license file.
- **Demo GIFs**: Added demo GIFs to command pages on the documentation site.
- **Install instructions**: Simplified to single-line commands.

### Internal

- **Pre-commit hooks**: Updated to immutable tags.
- **Lychee exclusions**: Cleaned up link checker configuration.

## 0.8.2

### Improved

- **Concurrent hook execution**: `wt hook post-start` and `wt hook post-switch` now run all commands concurrently (matching their normal background behavior) instead of sequentially with fail-fast. Multiple failures are collected and reported together.

### Documentation

- **Nested bare repo layout**: Added worktree-path template example for nested bare repo layout (`project/.git` pattern). Uses relative paths like `../{{ branch | sanitize }}` to create worktrees as siblings to the .git directory.

## 0.8.1

### Improved

- **Shell and PowerShell installers**: Added one-line install commands for Linux/macOS and Windows.
- **Consistent terminology**: CLI now uses "branch name" consistently instead of mixing "worktree" and "branch". The `wt remove` argument is renamed from `worktrees` to `branches` to reflect that worktrees are addressed by branch name.

### Fixed

- **Switch hints**: Removed incorrect `wt switch @` hint and improved error output spacing.

### Documentation

- **Dev server and database patterns**: Added practical examples for running per-worktree dev servers with subdomain routing and databases with unique ports.

## 0.8.0

### Improved

- **Separate `--yes` and `--force` flags**: `--force/-f` renamed to `--yes/-y` for skipping prompts (all commands). New `--force/-f` on `wt remove` forces removal of worktrees with untracked files (build artifacts, node_modules, etc.). (Breaking: `--force` no longer skips prompts; use `--yes`)
- **Clearer branch deletion output**: `wt remove` output now shows "worktree & branch" when the branch is deleted, or plain "worktree" with a hint when kept. Makes scanning output for branch fate easier.
- **`post-switch` hook on remove**: When `wt remove` switches to the main worktree, post-switch hooks now run in the destination.
- **Allow merge commits by default**: `wt step push` no longer rejects history with merge commits. Removed `--allow-merge-commits` flag. (Breaking: flag removed)

### Fixed

- **Orphan branches in `wt list`**: Branches with no common ancestor with the default branch no longer cause errors.
- **Remote branch filtering**: `wt list --remotes` now filters out branches that are tracked as upstreams, not just branches with worktrees.
- **Error message spacing**: Reduced double-newline spacing in error messages.

## 0.7.0

### Improved

- **Working tree conflict detection**: `wt list --full` now detects conflicts using uncommitted working tree changes, not just committed content. This catches conflicts earlier—before committing changes that would conflict with the target branch.
- **Dev server URL column**: New optional URL column in `wt list` configured via `[list] url` template in project config (`.config/wt.toml`). URLs show with health-check styling: normal if the port is listening, dimmed otherwise.
- **Shell integration simplification**: The shell wrapper is now self-contained with all directive handling inlined. Removes the separate helper function that could become unavailable if shell initialization order changed.
- **Performance**: Repository caching reduces git subprocess spawns; parallelized pre-skeleton operations for faster initial display.
- **Improved error hints**: When a worktree path already exists during creation, the error hint now correctly suggests `--create --clobber`.

### Fixed

- **Docs syntax highlighting**: Fixed syntax highlighting colors being stripped by 1Password browser extension on the documentation site.

## 0.6.1

### Improved

- **`post-switch` hook**: New hook that runs in the background after every `wt switch` operation. Unlike `post-start` (which only runs on creation), `post-switch` runs on all switch results. Use cases include renaming terminal tabs, updating tmux window names, and IDE notifications.
- **Signal forwarding for hooks**: Hooks now receive SIGINT/SIGTERM when the parent process is interrupted, allowing proper cleanup. Previously, non-interactive shells continued executing after signals.
- **Faster `wt list` skeleton**: Time-to-skeleton reduced by caching default branch lookup, batching timestamp fetching, and deferring non-essential git operations. Skeleton shows `·` placeholder for gutter symbols until data loads.
- **Clearer `--clobber` hint**: Error message now says "to overwrite (with backup)" instead of "to retry with backup".

### Documentation

- **State side-effects**: Added section explaining how Worktrunk state operations may trigger git commands.
- **`wt merge` location**: Clarified that `wt merge` runs from the feature worktree.

## 0.6.0

### Improved

- **Single-width Unicode symbols**: Replaced emojis (🔄, ✅, ❌) with single-width Unicode symbols (◎, ✓, ✗, ▲, ↳, ○, ❯) for better terminal compatibility and consistent alignment.
- **Output system overhaul**: Clean separation of output channels (data→stdout, status→stderr, directives→file) means piping works with shell integration active. `wt list --format=json | jq` and `wt switch feature | tee log.txt` both work correctly. Background processes use `process_group(0)` instead of `nohup` for more reliable detachment.
- **Trailing arguments for `--execute`**: `wt switch --execute` now accepts arguments after `--`, enabling shell aliases like `alias wsc='wt switch --create -x claude'` then `wsc feature -- 'implement login'`.
- **`hash_port` template filter**: `{{ branch | hash_port }}` hashes the branch name to a deterministic port number (10000-19999), useful for running dev servers without port conflicts.
- **`sanitize` template filter**: `{{ branch | sanitize }}` explicitly replaces `/` and `\` with `-` for filesystem-safe paths. (Breaking: `{{ branch }}` now provides raw branch names. Update templates that use `{{ branch }}` in filesystem paths to use `{{ branch | sanitize }}` instead)
- **Log directory in state output**: `wt config state logs` and `wt config state get` now show the log directory path under a LOG FILES heading.
- **Actionable error hints**: Error messages now include hints about what command to run next.
- **Unified directory change output**: `wt remove` now shows "Switched to worktree for {branch} @ {path}" matching `wt switch` format.
- **Consistent "already up to date" formatting**: Standardized message wording and styling across commands.

### Fixed

- **`wt step rebase` with merge commits**: Fixed incorrect "Already up-to-date" when a branch has merge commits from merging target into itself.

### Documentation

- **Local CI workflow**: Added "Local CI" section to `wt merge --help` explaining how pre-merge hooks enable faster iteration.
- **Colored command reference**: Web docs now preserve ANSI colors in command reference output.
- **Clarified terminology**: Help text uses "default branch" instead of hardcoded "main".

## 0.5.2

### Improved

- **`--clobber` flag for `wt switch`**: When encountering a stale directory or file at the target worktree path, `--clobber` moves it to a timestamped `.bak` file instead of failing.
- **Relative paths in `wt list`**: Paths are now shown relative to the main worktree (`.`, `./subdir`, `../repo.feature`) instead of a computed common prefix that could degenerate to `/`.
- **Multiline error formatting**: Errors with context now show a header describing what worktrunk was trying to do, with the full error chain in a gutter block.
- **Semantic switch messaging**: Switching to an existing worktree now shows ⚪ (info) instead of ✅ (success), reflecting that nothing was created.

### Fixed

- **Symbol styling in removal messages**: Integration symbols (`_`, `⊂`) now render in their canonical dim appearance instead of inheriting the message's cyan color.
- **ConflictingChanges error formatting**: Fixed double newlines in the error message output.

## 0.5.1

### Improved

- **Integration status in removal messages**: Shows integration symbols (`_` for same commit, `⊂` for integrated) when removing worktrees, matching `wt list` display.
- **Concurrent command limiting**: Limits concurrent git processes to 32 (configurable via `WORKTRUNK_MAX_CONCURRENT_COMMANDS`), preventing resource exhaustion on repos with many branches.
- **Better error display for `wt list`**: Task errors are now collected and displayed as warnings after the table renders, instead of being silently swallowed.
- **Remove continues on partial failures**: `wt remove` continues removing other worktrees when some fail, reporting all errors at the end.
- **Bash syntax highlighting**: Shell commands in error gutters now have syntax highlighting.
- **Shell integration is command-aware**: Detection and removal works correctly when installed as `git-wt` or other names.
- **CI fetch error documentation**: Yellow warning symbol (⚠) in CI column is now documented in help text.

### Fixed

- **CI status with multiple workflows**: Fixed incorrect status when multiple workflows exist (e.g., `ci` and `publish-docs`). Now uses GitHub's check-runs API to aggregate all workflow statuses.
- **State storage unification**: Unified branch-keyed state under `worktrunk.state.<branch>.*`. Numeric branch names now work. (Existing CI cache and markers regenerate on first access)

### Internal

- **Environment variable prefix**: Standardized to `WORKTRUNK_` prefix (e.g., `WORKTRUNK_MAX_CONCURRENT_COMMANDS`).
- Automatic winget package publishing on releases.

## 0.5.0

### Improved

- **Path column hidden when redundant**: Path column is deprioritized when all paths match the naming template, showing only at wider terminal widths (~125+ columns).
- **Better error formatting**: Errors with context now show a header with the root cause in a gutter block, improving readability for git errors.
- **Clearer integration target**: Separated `default_branch` (for stats like ahead/behind) from `target` (for integration checks), catching branches merged remotely before pulling.

### Fixed

- **Untracked files block integration**: Untracked files now prevent a worktree from being flagged as integrated, avoiding accidental data loss on removal.
- **Dirty worktree count includes untracked**: Summary now correctly counts worktrees with untracked files as dirty.
- **Branch name disambiguation**: Fixed `refname:short` issues when a branch and remote have the same name.
- **JSON output uses kebab-case**: Enum values changed from snake_case to kebab-case (e.g., `same_commit` → `same-commit`). (Breaking: scripts parsing JSON output may need updates)
- **Legacy marker format removed**: Plain-text markers no longer parsed. (Breaking: re-set markers with `wt config state marker set`)

### Internal

- **Unified command execution**: All external commands now go through `shell_exec::run()` for consistent logging and tracing.

## 0.4.0

### Added

- **`--no-rebase` flag for `wt merge`**: Fails early with a clear error if the branch is not already rebased onto target, rather than auto-rebasing. Useful for workflows that handle rebasing separately. ([#194](https://github.com/max-sixty/worktrunk/pull/194))

### Changed

- **Branch-first argument resolution**: `wt switch` and `wt remove` now check if the branch has a worktree anywhere before checking the expected path. If you type `wt switch foo`, you get branch foo's worktree, not whatever happens to be at the expected path. ([#197](https://github.com/max-sixty/worktrunk/pull/197))

### Fixed

- **`--no-commit` incorrectly skipped rebasing**: `wt merge --no-commit` now correctly rebases before stopping (if needed), rather than skipping the rebase entirely. ([#194](https://github.com/max-sixty/worktrunk/pull/194))
- **Pager for `wt config show --full`**: The pager now works correctly with the `--full` flag, showing diagnostics properly. ([#198](https://github.com/max-sixty/worktrunk/pull/198))
- **Statusline stdin handling**: Fixed flaky behavior on Windows CI by using standard is_terminal() check instead of timeout-based approach. ([#210](https://github.com/max-sixty/worktrunk/pull/210))

### Improved

- **Path-occupied error messages**: When `wt switch` can't create a worktree because the path exists, error messages now show which branch occupies the path and provide actionable commands to fix the situation. ([#195](https://github.com/max-sixty/worktrunk/pull/195), [#206](https://github.com/max-sixty/worktrunk/pull/206), [#207](https://github.com/max-sixty/worktrunk/pull/207))
- **Switch mismatch detection**: Better error messages when path/branch mismatches occur, with hints showing the expected path. ([#195](https://github.com/max-sixty/worktrunk/pull/195))

## 0.3.1

### Fixed

- **Branch names with slashes**: Branch names like `fix/feature-name` no longer break git config markers. Slashes are now escaped for git config compatibility. ([#189](https://github.com/max-sixty/worktrunk/pull/189), thanks @kyleacmooney)
- **stdin inheritance for `--execute`**: Interactive programs (vim, python -i, claude) now work correctly with `--execute` on non-Unix platforms. ([#191](https://github.com/max-sixty/worktrunk/pull/191))
- **Filenames with spaces/newlines**: Git status parsing now handles filenames containing spaces and newlines correctly using NUL-separated output.
- **Concurrent approval race condition**: Multiple concurrent approval/revocation operations no longer overwrite each other. Approvals now reload from disk before saving.
- **Dirty worktrees incorrectly marked integrated**: Priority 5 integration check now requires clean working tree state, preventing worktrees with uncommitted changes from being flagged as safe to remove.
- **Type changes not detected as staged**: Index status check now recognizes file type changes (`T` status) as staged changes.
- **User hook failure strategy**: Hook failure strategy now correctly applies to user hooks instead of always using fail-fast.
- **Branch variable in detached HEAD**: `{{ branch }}` now correctly expands to "HEAD" in detached HEAD worktrees instead of "(detached)".

### Improved

- **Self-hosted GitLab support**: CI auth checks now detect the GitLab host from the remote URL, supporting self-hosted GitLab instances instead of always checking gitlab.com.
- **Platform-specific CI status**: `wt list --full` and `wt config show` now show only the relevant CI tool (GitHub Actions or GitLab CI) based on the repository's remote URL.
- **LLM error reproduction**: When LLM commands fail, error messages now show the full reproduction command (e.g., `wt step commit --show-prompt | llm`) for easier debugging.
- **Location format**: Messages now use `@` instead of `at` for location phrases (e.g., "Switched to feature @ /path").
- **Switch help text**: Clarified that `wt switch` creates worktrees automatically for existing branches, not just for new branches with `--create`.

## 0.3.0

### Added

- **`--show-prompt` flag for LLM commands**: `wt step commit --show-prompt` and `wt step squash --show-prompt` output the rendered LLM prompt without executing the command. Useful for debugging templates or manually piping to LLM tools. ([#187](https://github.com/max-sixty/worktrunk/pull/187))
- **Diff size limits and diffstat for LLM prompts**: Large diffs (>400K chars) are progressively filtered—first removing lock files, then truncating to 50 lines/file, max 50 files. New `git_diff_stat` template variable shows line change statistics. ([#186](https://github.com/max-sixty/worktrunk/pull/186))
- **`MainState::Empty` status**: New `_` symbol for clean same-commit branches (safe to delete), distinguished from `–` (en-dash) for same-commit branches with uncommitted changes. Previously, both showed `_`. Only Empty branches are dimmed and considered "potentially removable". ([#185](https://github.com/max-sixty/worktrunk/pull/185))

### Changed

- **State subcommands default to `get`**: Running `wt config state default-branch` now defaults to `get`, making the command shorter. Use explicit `get` subcommand to access options like `--refresh` or `--branch`. ([#184](https://github.com/max-sixty/worktrunk/pull/184))
- **Clearer integration reason messages**: Updated descriptions to be more precise—"same commit as" instead of "already in" for SameCommit, "ancestor of" for Ancestor, "no added changes" for NoAddedChanges, "tree matches" for TreesMatch.

## 0.2.1

### Changed

- **Unified state management**: `wt config var` and `wt config cache` replaced by `wt config state` with consistent get/set/clear semantics for all runtime state. New subcommands: `default-branch`, `ci-status`, `marker`, `logs`, `show`. ([#178](https://github.com/max-sixty/worktrunk/pull/178))
- **Comprehensive state overview**: `wt config state show` displays all state (default branch, switch history, markers, CI cache, logs) with `--format=json` support. ([#180](https://github.com/max-sixty/worktrunk/pull/180))

### Added

- **`git-wt` binary for Windows**: New `git-wt` binary avoids conflict with Windows Terminal's `wt` command. Build with `--features git-wt`. Shell init/install now accept `--cmd` to specify which binary name to use. ([#177](https://github.com/max-sixty/worktrunk/pull/177))
- **Diffstat in select preview**: The log preview (Tab 2) in `wt select` now shows line change statistics (+N -M) matching `wt list`'s HEAD± column format. ([#179](https://github.com/max-sixty/worktrunk/pull/179))

### Fixed

- **Windows compatibility**: Multiple test and runtime fixes for Windows including stdin timeout handling, path canonicalization, and cross-platform test behavior. ([#167](https://github.com/max-sixty/worktrunk/pull/167), [#168](https://github.com/max-sixty/worktrunk/pull/168), [#169](https://github.com/max-sixty/worktrunk/pull/169), [#170](https://github.com/max-sixty/worktrunk/pull/170), [#171](https://github.com/max-sixty/worktrunk/pull/171), [#174](https://github.com/max-sixty/worktrunk/pull/174), [#176](https://github.com/max-sixty/worktrunk/pull/176))

## 0.1.21

### Fixed

- **Windows path handling in shell templates**: Fixed path quoting in hook templates on Windows by using `cygpath` to convert native Windows paths to POSIX format for Git Bash compatibility. Template variables like `{{ worktree }}` and `{{ repo_root }}` now work correctly. ([#161](https://github.com/max-sixty/worktrunk/pull/161))
- **Hook errors show `--no-verify` hint**: When hooks fail during `wt merge`, `wt commit`, or `wt squash`, the error message now includes a hint about using `--no-verify` to skip hooks. ([4a89748](https://github.com/max-sixty/worktrunk/commit/4a89748f))

## 0.1.20

### Changed

- **`--doctor` renamed to `--full`**: The `wt list --doctor` flag is now `wt list --full`. The new name better reflects that it shows extended information (binaries status, full diff stats). ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))
- **CLI binaries status in `wt config show --full`**: Shows installation and authentication status of `gh` and `glab` CLI tools in a new BINARIES section. ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))
- **CI tool hints**: `wt list --full` shows a hint when CI status is unavailable, with specific guidance on which CLI tool to install or authenticate. ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))

### Fixed

- **GitHub StatusContext checks**: CI status now includes StatusContext checks (used by some CI systems like Jenkins, CircleCI, and external status checks) in addition to CheckRuns. ([690da88](https://github.com/max-sixty/worktrunk/commit/690da889))
- **Windows Git Bash detection with WSL**: Fixed detection of Git Bash when WSL is installed. Previously, the WSL bash shim in PATH could be found instead of Git Bash, causing hook execution failures. ([b48b0ba](https://github.com/max-sixty/worktrunk/commit/b48b0ba7))

## 0.1.19

### Added

- **`wt step for-each` command**: Run commands across all worktrees sequentially. Supports template variables (`{{ branch }}`, `{{ worktree }}`, etc.) and JSON context on stdin. Example: `wt step for-each -- git pull --autostash`. ([#138](https://github.com/max-sixty/worktrunk/pull/138))

### Changed

- **Content integration detection always enabled**: The `⊂` (content integrated) symbol now appears without requiring `--full`. Squash-merged branches are detected automatically. ([f39c442](https://github.com/max-sixty/worktrunk/commit/f39c4428))
- **SIGINT forwarding**: Ctrl+C now properly terminates child processes in hooks, preventing orphaned background commands. ([#136](https://github.com/max-sixty/worktrunk/pull/136))

### Fixed

- **Windows path handling**: Fixed path canonicalization issues on Windows that caused worktree detection failures. Uses `dunce` to handle Windows verbatim paths (`\\?\`) that git cannot process. ([#125](https://github.com/max-sixty/worktrunk/pull/125))

## 0.1.18

### Added

- **Windows support**: Git Bash with PowerShell fallback enables worktrunk on Windows. Git Bash is preferred (same bash hook syntax across platforms); PowerShell works for basic commands with limitations. ([#122](https://github.com/max-sixty/worktrunk/pull/122))
- **Winget publishing**: Release workflow now publishes to Windows Package Manager. ([079c9df](https://github.com/max-sixty/worktrunk/commit/079c9df3))

### Changed

- **Approvals command moved**: `wt config approvals` is now `wt hook approvals` since approvals manage hook commands. ([b7b1b9e](https://github.com/max-sixty/worktrunk/commit/b7b1b9e3))
- **Approval prompts show templates**: Approval prompts now display command templates (what gets saved) rather than expanded values. ([2315d26](https://github.com/max-sixty/worktrunk/commit/2315d268))
- **Preview mode renamed**: The `history` preview mode is now `log` for clarity. ([0461152](https://github.com/max-sixty/worktrunk/commit/04611524))

### Fixed

- **PR/MR source filtering**: Filter PRs by source repository instead of author, fixing false matches when multiple users have PRs with the same branch name. ([e9ccdf7](https://github.com/max-sixty/worktrunk/commit/e9ccdf77))

## 0.1.17

### Added

- **User-level hooks**: Define hooks in `~/.config/wt.toml` that run for all repositories. New `wt hook show` command displays configured hooks and their sources. ([#118](https://github.com/max-sixty/worktrunk/pull/118))
- **SSH URL support**: Git SSH URLs (e.g., `git@github.com:user/repo.git`) now work correctly for remote operations and branch name escaping. ([92c2cef](https://github.com/max-sixty/worktrunk/commit/92c2cef8))
- **Help text wrapping**: CLI help text now wraps to terminal width for better readability. ([fe981c2](https://github.com/max-sixty/worktrunk/commit/fe981c2e))

### Changed

- **JSON output redesign**: `wt list --format=json` now outputs a query-friendly format. This is a breaking change for existing JSON consumers. ([236eae8](https://github.com/max-sixty/worktrunk/commit/236eae81))
- **Status symbols**: Reorganized status column symbols for better scannability. Same-commit now distinguished from ancestor in integration detection. ([5053af8](https://github.com/max-sixty/worktrunk/commit/5053af88), [a087962](https://github.com/max-sixty/worktrunk/commit/a0879623))

### Fixed

- **ANSI state reset**: Reset terminal ANSI state before returning to shell, preventing color bleeding into subsequent commands. ([334f6d9](https://github.com/max-sixty/worktrunk/commit/334f6d99))
- **Empty staging error**: Fail early with a clear error when trying to generate a commit message with nothing staged. ([b9522bc](https://github.com/max-sixty/worktrunk/commit/b9522bc6))

## 0.1.16

### Added

- **Squash-merge integration detection**: Improved branch cleanup detection with four ordered checks to identify when branch content is already in the target branch. This enables accurate removal of squash-merged branches even after target advances. New status symbols: `·` for same commit, `⊂` for content integrated via different history. ([6325be2](https://github.com/max-sixty/worktrunk/commit/6325be28))
- **CI absence caching**: Cache "no CI found" results to avoid repeated API calls for branches without CI configured. Reduces unnecessary rate limit consumption. ([8db3928](https://github.com/max-sixty/worktrunk/commit/8db39285))
- **Shell completion tests**: Black-box snapshot tests for zsh, bash, and fish completions that verify actual completion output. ([#117](https://github.com/max-sixty/worktrunk/pull/117))

### Changed

- **Merge conflict indicator**: Changed from `⊘` to `⚔` (crossed swords) for better visual distinction from the rebase symbol. ([f3b96a8](https://github.com/max-sixty/worktrunk/commit/f3b96a83))

### Documentation

- **Hook JSON context**: Document all JSON fields available to hooks on stdin with examples for Python and other languages. ([af80589](https://github.com/max-sixty/worktrunk/commit/af805898))
- **CI caching**: Document that CI results are cached for 30-60 seconds and how to use `wt config cache` to manage the cache. ([4804913](https://github.com/max-sixty/worktrunk/commit/48049132))
- **Status column clarifications**: Clarify that the Status column contains multiple subcolumns with priority ordering. ([1f9bb38](https://github.com/max-sixty/worktrunk/commit/1f9bb38f))

## 0.1.15

### Added

- **`wt hook` command**: New command for running lifecycle hooks directly. Moved hook execution from `wt step` to `wt hook` for cleaner semantic separation. ([#113](https://github.com/max-sixty/worktrunk/pull/113))
- **Named hook execution**: Run specific named commands with `wt hook <type> <name>` (e.g., `wt hook pre-merge test`). Includes shell completion for hook names from project config. ([#114](https://github.com/max-sixty/worktrunk/pull/114))

### Fixed

- **Zsh completion syntax**: Fixed `_describe` syntax in zsh shell completions. ([6ae9d0f](https://github.com/max-sixty/worktrunk/commit/6ae9d0f9))
- **Fish shell wrapper**: Fixed stderr redirection in fish shell wrapper. ([0301d4b](https://github.com/max-sixty/worktrunk/commit/0301d4bf))
- **CI status for local branches**: Only check CI for branches with upstream tracking configured. ([6273ccd](https://github.com/max-sixty/worktrunk/commit/6273ccdb))
- **Git error messages**: Include executed git command in error messages for easier debugging. ([200eea4](https://github.com/max-sixty/worktrunk/commit/200eea43))

## 0.1.14

### Added

- **Pre-remove hook**: New `pre-remove` hook runs before worktree removal, enabling cleanup tasks like stopping devcontainers. Thanks to [@pwntester](https://github.com/pwntester) in [#101](https://github.com/max-sixty/worktrunk/issues/101). ([#107](https://github.com/max-sixty/worktrunk/pull/107))
- **JSON context on stdin**: Hooks now receive worktree context as JSON on stdin, enabling hooks in any language (Python, Node, Ruby, etc.) to access repo information. ([#109](https://github.com/max-sixty/worktrunk/pull/109))
- **`wt config create --project`**: New flag to generate `.config/wt.toml` project config files directly. ([#110](https://github.com/max-sixty/worktrunk/pull/110))

### Fixed

- **Shell completion bypass**: Fixed lazy shell completion to use `command` builtin, bypassing the shell function that was causing `_clap_dynamic_completer_wt` errors. Thanks to [@cquiroz](https://github.com/cquiroz) in [#102](https://github.com/max-sixty/worktrunk/issues/102). ([#105](https://github.com/max-sixty/worktrunk/pull/105))
- **Remote-only branch completions**: `wt remove` completions now exclude remote-only branches (which can't be removed) and show a helpful error with hint to use `wt switch`. ([#108](https://github.com/max-sixty/worktrunk/pull/108))
- **Detached HEAD hooks**: Pre-remove hooks now work correctly on detached HEAD worktrees. ([#111](https://github.com/max-sixty/worktrunk/pull/111))
- **Hook `{{ target }}` variable**: Fixed template variable expansion in standalone hook execution. ([#106](https://github.com/max-sixty/worktrunk/pull/106))
