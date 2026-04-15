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

## Skim 4.x upgrade impact (verified 2026-04-14 against v4.6.0)

We currently pin skim 0.20.5. The 4.x line is a substantial rewrite — most of our candidate patches above become unnecessary, but the upgrade itself is a project. Findings from reading `github.com/skim-rs/skim` at tag `v4.6.0` (clone at `~/workspace/skim`):

### Architecture changes relevant to our patches

- **tuikit is gone.** 4.x depends on `ratatui = "0.30"` + `crossterm` directly; there is no `skim-tuikit` crate in its dep graph. `src/tui/` is the internal TUI module. Our `vendor/skim-tuikit/` patch tree becomes orphaned — nothing to rebase onto.
- **ANSI parser replaced.** The homegrown `ANSIParser` is gone; `ansi-to-tui = "8.0.1"` handles ANSI in `src/tui/{header,input,preview,util}.rs`. SGR handling moves to that crate's responsibility.
- **Preview model rewritten.** `src/tui/preview.rs` uses `portable_pty` for command-based previews, runs them on a dedicated thread (`thread_handle: Option<JoinHandle<()>>`), and renders via `tui_term::vt100` + `ratatui`. UI thread is no longer blocked on preview compute.
- **Public event channel.** `Skim::event_sender() -> tokio::sync::mpsc::Sender<Event>` (src/skim.rs:321) hands out a sender any background thread can push into. `Event::RunPreview` is a public variant of the public `Event` enum (src/tui/event.rs:144).
- **Async action callbacks.** `ActionCallback::new` / `new_sync` accept async or sync closures receiving `&mut App` (src/tui/event.rs:54–108). Custom actions get direct mutable access to app state instead of going through side channels.
- **`SkimItem::preview()` unchanged** — still `fn preview(&self, _context: PreviewContext) -> ItemPreview`. Sync, same variants (`Text`/`AnsiText`/`Command`/`Global`/…).
- **`SkimItem::display()` changed.** Returns `ratatui::text::Line<'_>` instead of `AnsiString<'_>`. Our `WorktreeSkimItem::display()` impl needs rewriting against ratatui.
- **`SkimItem::get_index` / `set_index` removed** (4.0). We don't implement them, so zero impact.
- **Default matcher algorithm changed** to "Arinae" (typo-resistant off by default). Our ranking snapshots and picker-perf benchmarks will shift.
- **`AsAny` blanket impl in skim's lib** (src/lib.rs:101–116). The documented downcast example in the new `SkimItem` docstring uses it successfully — the TypeId-mismatch bug we hit is **likely** fixed by the consolidated crate structure. Needs empirical verification on our actual item type.

### Per-patch fate under a 4.x upgrade

| Patch | Status in 4.x | Action on upgrade |
|---|---|---|
| **Preview refresh / invalidate** (this memo's topic) | **Solved natively.** Push `Event::RunPreview` via `Skim::event_sender()` when cache lands. | Drop from candidate list; rewrite picker to use `event_sender()`. |
| **Async preview / thread-safe preview API** | **Solved.** Previews run on dedicated thread + PTY. | Drop. `DashMap` for cache is still fine but no longer motivated by UI-thread contention. |
| **SGR 22 (intensity reset) handling** | **Offloaded to `ansi-to-tui` 8.0.1.** Our skim-side fix is no longer applicable. Needs empirical check that `color_print`'s `</>` output renders correctly through `ansi-to-tui`. | Re-test the preview-tab header + `compute_*_preview` output. If `ansi-to-tui` handles 22 correctly, delete the `Reset` scatter in `items.rs`. |
| **Action context for `reload` / `refresh-preview`** | **Solved.** `ActionCallback::new_sync(|app| …)` gives direct `&mut App`. | Drop the two temp-file side channels (`src/commands/picker/preview.rs`, `src/commands/picker/mod.rs:435,508-511`). Wire a real callback. |
| **TypeId-mismatch downcast** | **Probably solved** by consolidated compilation + blanket `AsAny`. | Test first: if `as_any().downcast_ref::<WorktreeSkimItem>()` returns `Some`, delete the `output()`-string-match fallback in `src/commands/picker/mod.rs:217-220`. |
| **Off-thread `CommandCollector::invoke`** | **Probably solved.** The trait may have been restructured around `ActionCallback` / async runtime. Needs concrete check of the new collector API. | Re-evaluate against the 4.x `App` + `ActionCallback` surface. |
| **Cwd-independent skim** | Unverified — no release note mentions it. | Check empirically after upgrade. |
| **Landed: `Output::flush` write_all (tuikit)** | **Orphaned.** `src/output.rs` in 4.x is a different file (final selection output, not terminal output). Terminal output is `ratatui::Terminal` over `CrosstermBackend<BufWriter<Stderr>>` — our tuikit fix doesn't apply. Upstream PR [#1056](https://github.com/skim-rs/skim/pull/1056) is against the 0.x line. | Drop the vendored `skim-tuikit` tree entirely. |
| **In-flight: smcup/rmcup symmetry in partial-height mode** | Orphaned same reason — tuikit is gone. | Re-evaluate the symptom against 4.x's ratatui-based alt-screen handling before reinventing. |

### Net cost/benefit

- **If we upgrade to 4.x first:** every open skim-side patch except possibly cwd-independence becomes unnecessary. `vendor/skim-tuikit/` can be deleted. Carrying zero vendor patches against skim is achievable.
- **If we patch 0.20.5 now:** the preview-refresh fix is ~15 vendored LOC in `previewer.rs` + `model/mod.rs`. Cheap to carry until we migrate.
- **4.x migration cost on our side** (not the vendor tree): rewrite `WorktreeSkimItem::display()` for `ratatui::text::Line`, re-snapshot picker ranking tests (matcher default changed), re-test ANSI rendering through `ansi-to-tui`, re-wire actions to `ActionCallback` (potentially big simplification, potentially churn). Non-trivial but bounded.

### Stability assessment — don't vendor yet (2026-04-14)

Considered vendoring skim 4.x ourselves with loosened dep pins. Decided **not yet**. Reasons:

- **Upstream is already fixing the pinning problem.** [skim-rs/skim#1050](https://github.com/skim-rs/skim/issues/1050) was filed 2026-04-13 and closed 2026-04-14 with maintainer LoricAndre committing to loosen pins on widely-used deps and ship a release "today or tomorrow." Our primary rationale for vendoring (unpinning `=X.Y.Z` constraints on ~17 deps) evaporates if that release delivers. Re-check after next release — if pins are livable, migrate against crates.io skim with zero vendor cost.
- **4.x is a young rewrite, stabilizing but not stable.** Bug intake by month (`bug`-labeled issues, 4.x era): 25 (Jan), 12 (Feb), 5 (Mar), 2 (Apr half-month). Declining sharply. But the cohort touches exactly our surface area: PTY preview hangs ([#951](https://github.com/skim-rs/skim/issues/951)), PTY changes cwd ([#950](https://github.com/skim-rs/skim/issues/950)), runaway thread loop ([#949](https://github.com/skim-rs/skim/issues/949)), matcher flake ([#947](https://github.com/skim-rs/skim/issues/947)), preview runs once per invocation ([#944](https://github.com/skim-rs/skim/issues/944)), some ANSI codes wrong in preview ([#964](https://github.com/skim-rs/skim/issues/964)). All closed, all within the last 8 weeks.
- **Open concern**: [#988](https://github.com/skim-rs/skim/issues/988) `MatchedItem should be Weak<dyn SkimItem>` — lifecycle/cycle bug, still open, affects how picker items live and die. Track before migrating.
- **Release cadence is ~2/week.** Eight 4.x releases in April alone. Vendoring a crate that ships this often means rebasing our fork every few days — materially more expensive than the current `vendor/skim-tuikit/` tree (which hasn't shipped since Aug 2025, hence zero rebase cost). Vendoring an abandoned crate is cheap; vendoring an actively-developed one is not.
- **Bus factor of 1.** LoricAndre is doing nearly all substantive PRs. Responsive today, but if responsiveness vanishes, we'd inherit a live project.

### Recommended sequence

1. **This week**: watch for the next skim release (v4.7.0-ish) carrying the #1050 pin-loosening. Check whether our dep graph resolves against unpinned crates.io 4.x.
2. **Next 4–8 weeks**: let the 4.x bug tail drain further and watch bug-intake rate. If it stays at ≤2/month, 4.x is stabilized enough to build on.
3. **When ready, migrate without vendoring**: do the picker-side rewrite (display → `Line`, actions → `ActionCallback`, re-snapshot rankings, retest ANSI). Depend on crates.io skim. This is the best outcome — zero vendor patches against skim.
4. **Fallback if upstream re-pins or MSRV conflicts**: the ~17-line unpin patch is trivially small and always available. We lose nothing by deferring.

**For the current preview-refresh bug** that motivated this memo: ship the internal cache-refresh design (Option B+D2+C2+placeholder from the design doc, ~90 LOC, no vendor change). Accept the keypress-to-redraw limitation. By the time we'd have a skim vendor patch merged and rebased, 4.x will likely be mature enough to migrate to natively — and migration gives us `event_sender()` + `Event::RunPreview` for free.

### Verification method

All architectural claims in this section were verified by reading source at tag `v4.6.0` in `~/workspace/skim`. To re-verify after future skim releases:

```bash
cd ~/workspace/skim && git fetch upstream --tags && git worktree add /tmp/skim-<ver> v<ver>
# then grep for: pub event_sender, Event::RunPreview, SkimItem trait, ansi_to_tui usage
```

Bug-intake check for stability re-assessment:

```bash
gh issue list --repo skim-rs/skim --limit 200 --state all --json number,createdAt,state,labels \
  --jq '[.[] | select(.labels | map(.name) | index("bug"))] | group_by(.createdAt[:7]) | map({month: .[0].createdAt[:7], count: length})'
```
