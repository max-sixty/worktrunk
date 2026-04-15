# Vendor Notes

What we currently patch and what we could patch if we vendor more.

## Current patches

`vendor/skim-tuikit/` is the only vendored crate. See `Cargo.toml` `[patch.crates-io]` block for the rationale on each landed change. Run `task vendor-diff` to see the live diff against the pristine upstream tarball.

Landed:

1. **`Output::flush` uses `write_all`** (`src/output.rs`) — fixes dropped bytes on partial writes under PTY pressure. Upstream PR: [skim-rs/skim#1056](https://github.com/skim-rs/skim/pull/1056). Drop this patch once #1056 ships in a tagged release.

In flight (dispatched 2026-04-14):

2. **Symmetric smcup/rmcup in partial-height mode** — currently worked around with `SkimOptionsBuilder::no_clear_start(true)` in `src/commands/picker/mod.rs:454-457`. See [skim-rs/skim#880](https://github.com/skim-rs/skim/issues/880). Branch: `tuikit-clear-start-fix`.

## Candidate future patches

These are workarounds in our crate that exist because we couldn't change skim. None are urgent — the picker works. Listed as options in case we revisit the cost/benefit of vendoring `skim` itself (we currently vendor only `skim-tuikit`).

**Important:** vendoring skim doubles maintenance surface (rebases, CI, license tracking). Prefer upstream PRs to skim-rs/skim where possible. The list below is "what becomes possible," not "what we should do."

### High payoff

- **SGR 22 (intensity reset) handling** — `src/commands/picker/items.rs` scatters `anstyle::Reset` after every styled span in preview info lines because skim's `ANSIParser::csi_dispatch` (`skim-0.20.5/src/ansi.rs`) handles SGR codes 0/1/2/4/5/7 but silently drops 22 (the reset that `color_print`'s `</>` emits for `<bold>` and `<dim>`). Without explicit `\x1b[0m`, dim/bold bleeds across the rest of the line. A one-line fix in the parser (`22 => attr.effect &= !(Effect::BOLD | Effect::DIM)`, plus 24/25/27 for parity) removes the workaround and stops future preview messages from needing to remember it. Revisit if more users report preview formatting issues.

- **TypeId-mismatch downcast** — `src/commands/picker/mod.rs:217-220` falls back to string-matching `item.output()` because `as_any().downcast_ref::<WorktreeSkimItem>()` always fails (skim 0.20 builds the `SkimItem` trait in two compilation units with different TypeIds). Fixing in skim lets `PickerCollector::invoke()` work with real types.

- **Action context for `reload` / `refresh-preview`** — we keep two temp files purely as side-channel IPC: one for preview mode in `src/commands/picker/preview.rs`, one for the alt-r selected item in `src/commands/picker/mod.rs:435,508-511`. Both exist because skim's actions don't pass any context to the collector. A small skim API (e.g. `Action::WithContext`) would delete both files and ~150 lines.

- **Off-thread `CommandCollector::invoke`** — `src/commands/picker/mod.rs:62-66,239-250` defers git removal, branch deletion, and post-remove hooks to a background thread, otherwise skim freezes its own UI loop. Also relevant: alt-r resets the cursor to top (skim-rs/skim#1695). If skim ran `invoke()` off-thread and preserved cursor on `reload`, both go away — and we could finally document alt-r in `cli/mod.rs:598`.

### Medium payoff

- **Async preview rendering** — `src/commands/picker/pager.rs:22-24,82-152` runs delta/bat with a 2s timeout and threaded stdin/stdout piping because a stalled pager would freeze skim's UI thread. Async previews in skim remove both the timeout and the thread juggling.

- **Thread-safe preview API** — `src/commands/picker/preview_orchestrator.rs:56-58` uses `DashMap` specifically so the UI-thread `preview()` callback never contends. A skim-side async preview API removes the need for lock-free structures.

- **`invalidate_preview()` / `refresh_preview()` API** — `src/commands/picker/items.rs:160-165,183-186` shows a "Press N again to refresh" placeholder because skim has no way to re-query `preview()` after background compute lands. A trivial skim method removes the awkward UX hint.

### Low payoff

- **Cwd-independent skim** — `src/commands/picker/mod.rs:226-237` chdirs to `$HOME` before removing the current worktree because skim and subsequent git commands both fail on a deleted cwd. A cwd-cached skim would let us delete this dance.

## Workflow for adding a vendor patch

1. Make the change in `vendor/skim-tuikit/`.
2. Update the `Cargo.toml` `[patch.crates-io]` comment block with the rationale and the upstream PR/issue URL.
3. Run `task vendor-diff` and confirm the diff is minimal and readable.
4. Move the entry above from "Candidate future patches" to "Landed" with the upstream tracking link.
5. Open an upstream PR against skim-rs/skim — the goal is always to stop carrying the patch.
