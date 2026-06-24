# Design: reclaim table width when the `wt switch` picker's preview toggles

Status: proposal (no production code that should land; the branch carries a
working prototype for evidence, described under "Prototype"). It answers one
question about the `wt switch` interactive picker:

When the preview pane is hidden with `alt-p`, the freed horizontal space should
go to the table. Today it doesn't: in the side-by-side (Right) layout the table
is laid out for ~half the terminal up front and stays at that width, so hiding
the preview leaves the right half blank instead of showing more columns. How
should the table reclaim that width, and how do we avoid moving the columns the
user is already reading?

## Summary of recommendations

Render the table at **full terminal width regardless of layout**, and let
skim's existing split clip it at the preview boundary (option 3 below). The
preview, when shown, covers the right-hand columns; `alt-p` widens skim's list
pane and the same rows reveal those columns. This is the only option in which a
column never moves on toggle, because the layout is computed exactly once. It
needs no reload, no re-layout, no cursor handling, and no new key. Two
supporting options on the picker's `SkimOptionsBuilder`:

- `no_hscroll(true)` so a fuzzy match deep in a row's search key can't scroll
  the leading columns out of view while the row overflows the half-width pane.
- An empty `ellipsis` (already the library default under
  `default-features = false`) so the clip is a clean cut, not a per-row `..`.

The two re-layout options (1 and 2) are buildable but pay for cleaner edges with
machinery the picker would otherwise never need, and neither holds every column
still: re-laying out at full width keeps the *leading* columns fixed but can
shift the middle and right ones whenever the flexible Summary column is present
or a low-priority column reappears.

## How the table is sized today

`src/commands/picker/mod.rs` reads the terminal once, picks Right or Down from
its dimensions, and bakes both the preview-window geometry and the table column
width into `SkimOptions` *before* skim takes over the terminal (module
docstring, `mod.rs:19-44`). The column width is computed from the layout:

```rust
// mod.rs (before this proposal)
let skim_list_width = match state.initial_layout {
    PreviewLayout::Right => terminal_width / 2,
    PreviewLayout::Down => terminal_width,
}
.saturating_sub(2);
```

That `skim_list_width` flows to `collect::collect` as `list_width`
(`mod.rs:1023`), which renders every row's column grid at that width. The
`alt-p` bind is `alt-p:toggle-preview` (`mod.rs:957`). skim's `TogglePreview`
flips `preview_window.hidden`, rebuilds its layout template, and asks for a
re-render (`skim/src/tui/app.rs:1122-1126`); the list pane widens to the full
terminal. But the *rows* were rendered at half width, so the extra space is
empty. Reclaiming it means the rows themselves have to carry full-width content.

## What skim can and can't do at runtime

These gate which options are buildable. All confirmed against skim 4.8.0 source.

- **The preview splits the screen; it never overlays.** `LayoutTemplate::apply`
  carves the terminal into disjoint `list_area` and `preview_area` rects via
  ratatui `Layout` (`skim/src/tui/layout.rs:99-117, 165-200`). There is no
  floating preview drawn on top of a full-width list. A literal overlay (the
  table rendered full-width *underneath* the preview) would require forking
  skim's render path.
- **`toggle-preview` is the only built-in that touches `preview_window`, and it
  only flips `hidden`.** No bind changes the split ratio or the Right/Down
  orientation (`skim/src/tui/app.rs:1122-1126`; the picker's own comment at
  `mod.rs:955-956` notes skim has no `change-preview-window`).
- **An `Action::Custom` callback can still change orientation at runtime.** It
  receives `&mut App`, and `app.options`, `app.layout_template`, and
  `app.header` are all `pub` (`skim/src/tui/app.rs:76,107,111`), as are
  `Direction`, `Size`, and `PreviewLayout` (`skim/src/tui/options.rs`,
  `skim/src/tui/mod.rs`). So a callback can set
  `app.options.preview_window.{direction,size,hidden}`, rebuild
  `app.layout_template = LayoutTemplate::from_options(&app.options, app.header.height())`,
  and re-render. This is the same `&mut App` mechanism PR #3199 already uses to
  reposition the cursor after `alt-r`.
- **`reload` re-renders rows at the current pane width.** Replacing the item
  list (the `alt-r` mechanism) makes skim recompute `container_width` from the
  current `list_area` on the next render (`skim/src/tui/item_list.rs:551`). So
  re-rendering rows at a new width and reloading them is a viable way to
  re-lay-out the table without relaunching skim.
- **A single-line row wider than its pane is clipped left-anchored, with no
  marker when the ellipsis is empty.** Horizontal overflow goes through
  `apply_hscroll`, not `trim_with_ellipsis` (which only fires on multiline
  vertical cutoffs) (`skim/src/tui/item_renderer.rs:175-179, 498-551`). With
  scroll shift 0 the renderer keeps the leftmost `container_width` columns and
  appends nothing. The library default `ellipsis` is the empty string
  (`skim/src/options.rs:1167`; the `..`/`...` default is gated on the `cli`
  feature, which the picker drops via `default-features = false`).
- **The scroll shift can be non-zero under an active query.** `apply_hscroll`
  shifts to keep the matched range visible (`item_renderer.rs:423-464`). The
  picker matches against `text()`, which is a short search key (branch name plus
  path), not the wide rendered row (`mod.rs`/`items.rs:163-201`). A query
  matching the path portion sits far along that key, so the shift can become
  positive and scroll the leading columns out of view. `no_hscroll(true)` forces
  the shift to 0 (`item_renderer.rs:423-424`), pinning every row to its left
  edge.

## How the column layout responds to width

This is the heart of the user's worry that reclaiming width would "change the
location of the initial columns." `src/commands/list/layout.rs` allocates
columns by priority from a `remaining` budget seeded with the width, then sorts
into display order and assigns left-to-right `start` positions
(`layout.rs:757-1009`). The decisive property: every column's *width* except
Summary and Message is content-driven or a fixed estimate, independent of the
terminal width (`layout.rs:453-493, 637-730`). Display order, with each column's
width behavior:

| # | Column | Header | Width |
|---|--------|--------|-------|
| 1 | Gutter | | fixed (2) |
| 2 | Branch | `Branch` | content (shrinkable) |
| 3 | Status | `Status` | content (≤8) |
| 4 | WorkingDiff | `HEAD±` | content (`+999 -999`) |
| 5 | AheadBehind | `main↕` | content (`↑99 ↓99`) |
| 6 | BranchDiff | `main…±` | content, `--full` only |
| 7 | Summary | `Summary` | **flexible (10-70)** |
| 8 | Upstream | `Remote⇅` | content |
| 9 | CiStatus | `CI` | content |
| 10 | Path | `Path` | content, on mismatch |
| 11 | Url | `URL` | content |
| 12 | Commit | `Commit` | fixed (8) |
| 13 | Time | `Age` | fixed (4) |
| 14 | Message | `Message` | **flexible (10-100)** |

So widening the table from `W/2` to `W` does three things, all to the right of
the leading block: low-priority columns that didn't fit (Commit, Time, Path,
Url) reappear; Summary and Message expand toward their caps; and on a true
re-layout, any column sitting after Summary in display order gets a larger
`start` because Summary in front of it grew. The leading columns
(Gutter through AheadBehind) keep identical positions because their widths don't
depend on width and they're allocated first.

Two consequences:

- The user's fear is right for the *middle and right* columns under a
  re-layout, and only when Summary is present: Upstream, CiStatus, Path, Url,
  Commit, Time, and Message can all shift right as Summary expands. The picker
  shows Summary only when `[commit.generation]` is configured and
  `[list] summary` is on, so a summaries-off picker re-layouts purely additively
  (Message is last, so its growth shifts nothing).
- The fear is wrong for the leading columns under any option. They never move.

The option that sidesteps the question entirely is the one that computes the
layout once, at full width, and never recomputes it.

## The three options

### Option 1: re-layout the whole table on toggle

Bind `alt-p` to a callback that flips `hidden`, re-renders every row at the new
pane width, and `reload`s them. Buildable: `reload` re-renders at the current
width (`item_list.rs:551`) and the `alt-r` path is the precedent for swapping
the item list mid-session.

- **Code.** A new verb on `PickerCollector` (or a second collector) that
  re-runs the column layout and row rendering at the toggled width, plus cursor
  restoration so the reload doesn't snap to the top (reuse
  `reposition_cursor_action`, `mod.rs:425-466`). The rows' progressively-arrived
  cells (status, diffs, CI) live in `rendered: Arc<Mutex<String>>` strings built
  at one width; re-deriving them at another width needs the live `ListItem`
  model kept current, not just the frozen skeleton snapshot the picker holds
  today. That is the real cost.
- **Constraint.** Leading columns stay put; middle and right columns shift when
  Summary is present (see above).
- **UX.** A reload clears and re-streams the list, so the toggle flickers and
  the cursor jumps (mitigated, not free). Heavier than today's instant
  `hidden`-flip.

### Option 2: toggle orientation (Right ↔ Down) and re-layout

Add a key that switches the preview between side-by-side and below, re-laying
out the table for the new list width. Buildable at runtime via an
`Action::Custom` callback mutating `app.options.preview_window` and rebuilding
`app.layout_template` (see "What skim can and can't do at runtime"); no relaunch
needed.

This is option 1 plus an orientation switch: changing orientation changes the
list width, so it carries the same re-render-and-reload machinery and the same
column-shift behavior, and adds a second control and a second piece of mutable
preview state. It widens the picker's surface the most for the least direct
answer to the width question. Worth recording as feasible, not worth building
first.

### Option 3: full-width table always, preview covers the right (recommended)

Render the table at full terminal width in both layouts. When the preview is
shown (Right), skim renders the full-width row into the half-width list pane and
clips the overflow at the boundary. `alt-p` widens the pane to full and the same
rows reveal their right-hand columns. The layout is computed once, so no column
ever moves; there is no reload, no re-render, no cursor handling, and no new
key. The toggle is exactly today's instant `hidden`-flip; the only change is
that the rows now carry full-width content for the wider pane to show.

This is the user's "put the preview over the RHS of the table" idea. skim can't
literally overlay (it splits), but clipping a full-width row at the split
boundary is visually identical to an overlay, minus a bisected cell at the
boundary: with `no_hscroll(true)` and an empty ellipsis the cut is clean and
left-anchored.

- **Code.** Three small edits in `mod.rs`: `skim_list_width =
  terminal_width.saturating_sub(2)` unconditionally; `.no_hscroll(true)`;
  `.ellipsis(String::new())` (explicit, so a future skim default change can't
  reintroduce `..`). The `--prs` grid aligns to `skim_list_width` and follows
  for free.
- **Constraint.** Satisfied in full: the layout is computed once at full width,
  so no column moves on toggle, including the middle and right ones that a
  re-layout would shift.
- **UX.** The reveal is instant and stable. The cost is in the preview-shown
  (default) state: the table is clipped at the preview boundary, which can
  bisect a cell (e.g. Summary text cut mid-word) instead of ending on a clean
  column edge the way today's half-width layout does. The prototype shows this
  is unobtrusive.

## Recommendation

Build option 3. It is the smallest change, it adds no runtime machinery, and it
is the only option that honors the stated constraint completely rather than
partly. Its one cost, a possibly-bisected cell where the preview meets the
table, is cosmetic and is the direct visual consequence of the behavior the user
asked for.

Option 1 is the fallback if a clean right edge in the preview-shown state turns
out to matter more than the bisected cell: it keeps the leading columns fixed
and fits the visible columns neatly into the half, at the price of reload
machinery and the live-model bookkeeping needed to re-render rows at two widths.
Option 2 is feasible and worth remembering (an `Action::Custom` can change
orientation at runtime, contrary to the picker's current comment), but it is the
heaviest option for the least direct payoff.

## Prototype

The branch carries option 3 as a working prototype (the three edits above, with
`PROTOTYPE` comments). Captured from `wt switch --branches` at 170 columns
(Right layout), against the worktrunk repo.

Preview shown. The table is laid out at full width and clipped at the preview
boundary; Summary is cut mid-word with no `..`:

```
    Branch                              Status        HEAD±    main↕  Summary       │1: HEAD± | 2: log | 3: main…± | 4: remote⇅ …
> @ design-picker-toggle-width           !  –      +16   -5           Simplify pick▐│
  ^ main                                    ^|💬                                   ▐│○ Loading working-tree diff…
  + fix-picker-gutter                       ↕|💬              ↑2  ↓1  Add no_hscrol▐│
  + picker-scrollbar                        ⊂ 💬              ↑2  ↓2  Add scrollbar▐│
```

After `alt-p`. The list pane is full width; the leading columns (Branch, Status,
HEAD±, main↕, Summary) are in the same positions, and Remote⇅, CI, URL, and
Commit are revealed on the right:

```
    Branch                              Status        HEAD±    main↕  Summary                                             Remote⇅  CI     URL                     Commit
> @ design-picker-toggle-width           !  –      +16   -5           Simplify picker list width calculation for full-w…                  http://127.0.0.1:14628  93cfa7▐
  ^ main                                    ^|💬                                                                             |            http://127.0.0.1:12107  93cfa7▐
  + fix-picker-gutter                       ↕|💬              ↑2  ↓1  Add no_hscroll option to skim picker configuration     |     #3213  http://127.0.0.1:13661  73c288▐
  + picker-scrollbar                        ⊂ 💬              ↑2  ↓2  Add scrollbar to picker item list                                   http://127.0.0.1:13533  ac8436▐
```

With `no_hscroll(true)` and the query `worktrunk.skim` (which matches the path
portion of the search key, far along it), every row still renders from column 0;
the leading columns stay anchored rather than scrolling to the match:

```
> worktrunk.skim                                                                    │1: HEAD± | 2: log | 3: main…± | 4: remote⇅ …
    Branch                              Status        HEAD±    main↕  Summary       │Enter: switch | Tab/alt-1…7: preview …
> + skim-windows                         !  ✗ 💬   +81 -100       ↓2  Refactor previ│
  + skim-features                           ⊂                    ↓15                │ src/commands/picker/mod.rs     |   4 --
```
