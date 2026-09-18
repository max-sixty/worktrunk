# Demo Development

## Directory structure

```
docs/demos/
  build            # Unified build script
  tapes/           # All VHS tape files (templated)
  shared/          # Python library, themes, fixtures
  tests/           # Tests for the build script and tape conventions
  .deps/vhs/       # VHS fork clone and binary (gitignored, built on demand)

docs/public/assets/  # Output GIFs (gitignored, shared with fetch-assets)
  docs/            # Doc site demos (1600x900; wt-core-mobile is 576x432)
    light/         # Light theme variants
    dark/          # Dark theme variants
  social/          # Social media demos (1200x700)
    light/         # Light theme only
```

Tape files use template variables (`{{FONTSIZE}}`, `{{WIDTH}}`, `{{HEIGHT}}`) so the same tape produces different sizes for docs vs social.

## Regenerating demos

```bash
./docs/demos/build docs      # Doc site demos (1600x900, light + dark)
./docs/demos/build social    # Social media demos (1200x700, light only)
./docs/demos/build docs --text  # Text output instead of GIFs
```

Text output captures terminal frames as plain text. Works for all demos, though interactive demos (TUI navigation, Zellij tabs) produce less useful output since the visual interactions don't translate to text.

Regenerate a single demo:

```bash
./docs/demos/build social --only wt-switch
./docs/demos/build docs --only wt-merge
./docs/demos/build docs --only wt-core-mobile
```

**Available demos:**

| Target | Demos |
|--------|-------|
| docs | wt-core, wt-core-mobile, wt-switch, wt-list, wt-commit, wt-statusline, wt-merge, wt-switch-picker, wt-zellij-omnibus |
| social | wt-switch, wt-statusline, wt-list, wt-list-remove, wt-hooks, wt-devserver, wt-commit, wt-merge, wt-switch-picker, wt-core, wt-zellij-omnibus |

## Snapshot testing

```bash
./docs/demos/build docs --snapshot           # Generate all snapshots
./docs/demos/build docs --snapshot --only wt-list  # Single demo
```

Snapshots capture command output (not terminal rendering) and are committed to `docs/demos/snapshots/`. Use them to catch regressions like new hints creeping in.

**How to use:**
1. After changing wt output, regenerate snapshots: `./docs/demos/build docs --snapshot`
2. Review the diff - small changes (commit hashes, minor formatting) are expected
3. Commit the updated snapshots alongside your changes

**What changes are expected:**
- Commit hashes change each run (demo repo is recreated)
- Column widths may shift slightly

**What changes indicate regressions:**
- New hints or warnings appearing
- Output format changes you didn't intend
- New lines or missing output

**TUI demo validation:**

TUI demos (Zellij, Claude UI) can't use text snapshots because VHS only captures the outer terminal, not content inside terminal multiplexers. Instead, they use OCR-based validation:

1. After recording, specific frames are extracted from the GIF using ffmpeg
2. Tesseract OCR extracts text from those frames
3. The text is validated for expected/forbidden patterns
4. Validation runs automatically when building TUI demos with defined checkpoints

Checkpoints are defined in `docs/demos/shared/validation.py`. To add validation to a TUI demo:
1. Identify key frame numbers by examining the GIF (25fps, so frame 75 = 3 seconds; a negative frame counts back from the last, for the state a recording ends in)
2. Define checkpoint patterns in `validation.py` with frame numbers, expected patterns, and forbidden patterns

`wt-switch`, `wt-switch-picker`, `wt-statusline`, and `wt-zellij-omnibus` have checkpoints. Other TUI demos are skipped until checkpoints are added.

**Measure the window; don't derive it.** A tape's Sleep directives don't fix where its content lands: command execution varies run to run, so the same tape gives GIFs whose frame counts differ by tens of frames. Anchor to the end (a negative `start`) where the content is near it, and prefer a window that stays generous under that drift. OCR each candidate range before committing to it — `extract_frames` plus `ocr_image` over `range(n-500, n, 20)` prints where a pattern actually lives.

**A window is calibrated for one target's terminal size.** One tape records at each target's own size, so a line's on-screen lifetime differs between them: the docs terminal is ~33 rows and the social one ~24. `wt merge`'s generated commit message survives to the end of the docs recording and scrolls off the social one within a second. Set `targets=("docs",)` on a checkpoint that depends on that, rather than widening the window until it catches an 18-frame band in both.

**Prerequisites for TUI validation:** `ffmpeg` and `tesseract` must be installed.

**Limitations:**
- Tab completion sequences are not replayed; only `Type "command"` + `Enter` patterns are extracted
- TUI demos without defined checkpoints are skipped
- OCR accuracy depends on font rendering quality

## Prerequisites

**Requires Go** — The VHS fork is built from source ([install Go](https://go.dev/dl/)).

**Requires ffmpeg with libass** — The keystroke overlay uses ASS subtitles, and Homebrew's regular `ffmpeg` formula is built without libass while holding the linked name, so installing or upgrading `ffmpeg` at any point takes the overlay away. `check_ffmpeg_libass` puts the full build in front of it on PATH for the run when it finds the linked one can't draw subtitles, and otherwise exits with:

```bash
brew install ffmpeg-full
```

External dependencies are downloaded/built automatically on first run:
- **VHS** — Custom fork with keystroke overlay (cloned and built from source)
- **Claude Code binary** — Downloaded from Anthropic's release bucket
- **Zellij plugin** — Downloaded from GitHub releases

Demos that launch Claude Code (`wt-switch`, `wt-statusline`, `wt-zellij-omnibus`) require authentication. On macOS, the recorder reuses the current `claude auth login` credential from the user's Keychain. Other environments can provide an OAuth token or API key:

```bash
export CLAUDE_CODE_OAUTH_TOKEN=...
# or
export ANTHROPIC_API_KEY=...
```

Recording uses the authenticated account's Claude usage.

## Publishing demos

After building, publish to the assets repo:

```bash
task publish-assets
```

This copies `docs/public/assets/{docs,social}/` to the `worktrunk-assets` repo (sibling directory), commits, and pushes. The script clones the repo via `gh` if missing.

To fetch published assets (without rebuilding):

```bash
task fetch-assets
```

Both build and fetch output to the same location (`docs/public/assets/`), so local builds override fetched assets.

## Modifying the VHS fork

The VHS fork is cloned to `docs/demos/.deps/vhs/` and built automatically. To modify it:

```bash
# 1. Make changes in the cloned repo
cd docs/demos/.deps/vhs
# ... edit files ...

# 2. Rebuild and test
go build -o vhs .
cd ../../../..
./docs/demos/build docs --only wt-switch-picker

# 3. Commit and push to the fork
cd docs/demos/.deps/vhs
git add -A
git commit -m "Description"
git push origin keypress-overlay
```

**CRITICAL**: Push changes to `origin keypress-overlay`. The directory is gitignored—changes only persist in the fork repo.

### Keystroke overlay timing

Each keystroke event is timed by the video's own clock: `Record` writes one
frame per tick and only while recording, so `recordedMS` in `vhs.go` turns that
frame count into the timeline the finished GIF plays on, and an event's time is
where it lands in the output. A `Hide` stretch records no frames and so
contributes nothing. Measured, a keypress and the frame that answers it are
within one frame of each other (25fps = 40ms), so nothing is added on top.

**Keys pressed while the recording is hidden all land on the first visible
frame**, since a hidden stretch records no frames to separate them. Every docs
demo types its opening command before `Show` on purpose — the first frame
carries the whole command, which
`test_docs_demos_open_on_a_complete_command_before_execution` pins — so that
command arrives in the overlay in one go, matching the prompt already on
screen. Anything the overlay should track key by key happens after `Show`.

To check the alignment of a recorded GIF, extract its frames and compare the
frame where the overlay changes against the frame where the screen answers:

```bash
ffmpeg -i demo.gif -fps_mode passthrough /tmp/gif-frames/frame_%04d.png
```

## The mocked forge

`fixtures/gh-mock.sh` answers every `gh` call from a file under
`$HOME/.local/share/gh-mock` that `write_gh_mock_data` (in `shared/lib.py`)
writes after the branches exist. Each file's first line is how long the mock
waits before answering; the rest is the JSON body.

Three things have to hold or the mock is never reached, and each one silently
empties the CI column rather than failing:

- **A parseable origin.** wt's CI detection parses the remote URL for an
  owner/repo before it will call `gh` at all, and a bare filesystem path doesn't
  parse. `prepare_base_repo` sets origin to `DEMO_ORIGIN_URL` and rewrites only
  *push* to the local bare repo via `url.<bare>.pushInsteadOf` — plain
  `git remote get-url` applies `insteadOf` but not `pushInsteadOf`, so rewriting
  fetch too would hand wt the local path back.
- **`gh --version` and `gh auth status` both exiting 0.** `CiToolsStatus::detect`
  gates every forge call on them.
- **`DEMO_PROJECT_ID` matching the URL.** It keys the approvals file, so a URL
  change without it leaves every project command unapproved.

The per-branch delays in `DEMO_PRS` are what make the CI column *stream*: wt
runs one `gh pr list` per branch concurrently, so staggered delays land the cells
one at a time behind a frame that already painted from local git. Keep the
largest under the tape's post-command sleep — `wt list` renders progressively but
still can't finish until every call returns.

The PR bodies and comment threads also ride these responses. `detect_github`
primes the picker's on-disk comments cache from the `gh pr list` payload, so the
`comments` tab renders with no second call.

## wt-switch-picker demo goals (interactive picker)

The wt-switch-picker demo showcases the interactive picker (`wt switch` without
args). The list itself is the subject here, so `prepare_picker` gives it sixteen
worktrees — the shared set plus `PICKER_EXTRA_BRANCHES`, where every other demo
keeps the shared four — and its table reads like a repo someone works in.

It is also the one demo that records at its own size, `SIZE_DOCS_PICKER`: the
same 1600x900 canvas as the rest in smaller text, which buys 139x34 instead of
102x25. At the usual size the columns worth watching — CI status and the branch
summary — fall off the right edge, and the preview pane is too short to page a
diff through. `prepare_picker` is also the reason the Summary column renders at
all: it writes the user config, and the column is gated on an LLM command being
configured. `fixtures/llm-mock.sh` answers those calls, picking a summary from
the paths in the diff, so each branch's row is its own sentence.

Variety to preserve across all columns:

| Column | Demonstration |
|--------|---------------|
| CI | PR number colored by state (`#4` failing, `#5` changes-requested, `#2` running, `#1` stale head) vs bare `#` (branch CI, passing and failing) vs none |
| HEAD± | Large unstaged diff (+106 -14), small (+1), none |
| Status | Staged changes (+), unstaged (!), untracked (?), ahead/behind (↕) |
| main↕ | Some branches ahead-only, some ahead-and-behind |
| main…± | Meaningful merge-base diffstats (small to 300+ lines) |

Branch setup:
- **alpha** — Large working tree changes, unpushed commits (so its PR head reads
  as stale), and the ten-comment thread the `comments` tab pages through
- **beta** — Staged changes, behind main, PR with CI running
- **hooks** — Staged+unstaged changes, no remote, so no CI at all
- **`PICKER_EXTRA_BRANCHES`** — each carries one commit of its own; a branch left
  on main's tip renders as an empty row *and* borrows main's branch CI, since
  branch CI is keyed by commit

Two keystroke hazards the tape works around, both of which pick the wrong
worktree rather than failing:

- **Narrowing the list keeps the cursor's row index**, not the row. A query
  matching several rows leaves the cursor on whichever row now sits at that
  index, and clearing the query restores the old index. Either type a query that
  matches exactly one row, or type it before moving the cursor at all.
- **The filter matches PR number, title and author too**, not just the branch
  name and path. Adding a PR to a branch can make a previously unambiguous query
  match several rows.

## Alt keybindings in tapes

`Alt+p`, `Alt+"8"` and the rest reach the program only because `buildTtyCmd` in
the VHS fork passes `-t macOptionIsMeta=true` to ttyd. Without it xterm.js hands
macOS Option to the browser's own composition and the program receives the
unmodified character — so an `Alt+p` in a tape types a literal `p` into the
picker's query, with no error anywhere. Verify a modifier reaches the program by
recording a tape that runs `cat > file`, sending the key, and reading the bytes:
alt-p is `1b70`, a bare `p` is `70`.

VHS's parser takes a string, `Enter` or `Tab` after `Alt+`, so a digit needs
quoting: `Alt+"8"`, not `Alt+8` (which fails to parse).

## Light/dark theme variants

The docs build generates both light and dark GIF variants in separate directories under `docs/public/assets/docs/` (the layout at the top of this file):
- `light/wt-core.gif` / `dark/wt-core.gif`
- `light/wt-core-mobile.gif` / `dark/wt-core-mobile.gif` (576×432 responsive homepage source)
- `light/wt-merge.gif` / `dark/wt-merge.gif`
- `light/wt-switch-picker.gif` / `dark/wt-switch-picker.gif`

Each theme starts from a freshly prepared demo environment because recording a tape changes its repository and worktrees.

Social build generates light only (social media doesn't support theme-switching media queries).

Each recording's environment carries its theme. The VHS terminal, Zellij, and the starship prompt take their colors from `docs/demos/shared/themes.py`, which reads the `--wt-*` custom properties in `docs/src/styles/custom.css` when the build runs, so a site palette change reaches the GIFs on the next recording. Claude Code and delta switch to their own light or dark theme.

## Debugging a demo environment

Use `--shell` to spawn an interactive fish shell with the demo environment:

```bash
./docs/demos/build social --only wt-switch --shell
```

This builds the demo and drops you into a fish shell with `HOME`, `PATH`, starship, and wt shell integration all configured. You're already in the demo repo and ready to test:

```fish
# Now you can manually test:
claude                                    # See what happens on first launch
wt switch --create foo                    # Create a worktree
wt switch --execute claude --create bar   # Test the demo command
```

After fish exits, the debug environment remains at the path printed when the shell starts. Remove that directory manually when you no longer need it.

## Timing guidelines

Demo GIFs should feel natural—not rushed, but not lingering. The goal is to let viewers read and understand each step before moving on.

| Context | Duration | Rationale |
|---------|----------|-----------|
| Simple output (one-liner) | 1.5s | Just enough to scan a short result |
| List/table output | 2–2.5s | Tables need more time to scan visually |
| Multi-line text (config, log) | 3s | Dense text requires reading time |
| Long operations (merge, hooks) | Match actual | Use real duration; don't artificially shorten |
| LLM operations | 4s | Show thinking + generated output |
| Transitions (cd, switch) | 1–1.5s | Brief pause after context change |
| Quick sequences (keystrokes) | 0.1–0.5s | Related actions feel like one gesture |
| Tab completion (shows menu) | 400ms | Pause after Tab when menu appears for viewer to see options |
| Tab completion (cycles selection) | 300ms | Pause after Tab cycles to show selected option |
| Tab completion (auto-completes) | 0 | No pause needed when Tab completes to single result |
| Tab completion (before Enter) | 50ms | Required after final Tab/selection before Enter; lets fish settle |
| Tab cycling → execute | Enter, 50ms, Enter | When Tab cycling with pager open: first Enter accepts, second executes |
| End hold (before exit) | 2–4s | Let final state sink in |
| Pre-enter pause | 1s | For commands where output clears visible area: TUI takeover (`claude`) or heavy output (`wt merge`). |
| Claude UI startup | 6s | Big visual change; wait for UI to render and settle |

**Principles:**

1. **Focus on output, not typing.** TypingSpeed is fast (28ms). Time is for reading results.
2. **Match reality for slow operations.** If `wt merge` takes 8s, sleep 8s. Don't fake speed.
3. **Group related actions.** Multiple keystrokes (↓↓) can be rapid; pause after the group.
4. **End with breathing room.** Viewers need a moment to absorb the final state.
5. **Twitter context.** These are viewed on phones in noisy feeds—slightly longer is better than too fast.
6. **Type what users would type.** If a flag is needed for technical reasons (e.g., `--color=always` for VHS), handle it in the background setup (env var, git config) so the demo shows the natural command. Never show flags users wouldn't normally type.

## Key files in the demo environment

After spawning the shell, these files control Claude Code behavior:

- `$HOME/.claude.json` - Claude Code global config (onboarding flags, marketplace settings)
- `$HOME/.claude/settings.json` - Claude Code settings (statusLine config)
- `$HOME/.config/worktrunk/config.toml` - Worktrunk user config
- `$HOME/w/acme/.config/wt.toml` - Project hooks config

Key fields in `.claude.json` for suppressing notifications:
- `officialMarketplaceAutoInstalled: true` - should suppress marketplace auto-install
- `numStartups: 100` - makes Claude think it's been run many times
- `hasCompletedOnboarding: true` - skips onboarding
- `announcementImpressions` - suppresses launch promotions
- `passesUpsellSeenCount`, `passesLastSeenRemaining`, and `hasVisitedPasses` - suppress the guest-pass promotion, including after eligibility refresh

## Viewing GIF results

**Do NOT use `open` on the GIF** — that's for the user to do manually.

Inline viewing options:
```bash
# Quick Look (macOS)
qlmanage -p docs/public/assets/docs/light/wt-switch-picker.gif

# iTerm2 inline images
imgcat docs/public/assets/docs/light/wt-switch-picker.gif
```

## Reviewing demo GIFs

After building demos, use a subagent to review for visual errors before publishing.

**Extract frames and review:**
```bash
rm -rf /tmp/frames && mkdir -p /tmp/frames
magick path/to/demo.gif -coalesce /tmp/frames/frame_%04d.png
```

Then spawn a haiku subagent with these instructions:

```
Review this demo GIF for visual errors.

Read frames sampled throughout the recording — every 50th frame covers a
~2000 frame GIF in ~40 images. Use the Read tool on:
/tmp/frames/frame_0050.png, frame_0100.png, frame_0150.png, ... etc.

Look for:
1. SPLIT COMMANDS: Text split across panes (e.g., "gi" in one pane, "t diff" in another)
2. ERRORS/WARNINGS: Shell errors like "Unknown command", red error text, warning messages
3. WRONG LOCATION: Commands or output appearing in unexpected pane/tab
4. VISUAL GLITCHES: Partial characters, cursor artifacts, broken layouts

Report each issue with:
- Frame number(s)
- Description
- Affected text

If a frame shows an error like "Unknown command: t", examine nearby frames
(±5) to understand the cause — likely a timing bug where a command was split.
```

## Cleaning up stale demo processes

**NEVER run `pkill -f zellij`** — this kills the user's own Zellij session, not just demo processes.

If stale Zellij processes from previous demo runs are causing issues, either:
- Let them die on their own (they'll timeout)
- Target only demo processes: `pkill -f "zellij.*wt-demos"`
- Remove the demo directory and rebuild: `rm -rf /private/tmp/wt-demos`
