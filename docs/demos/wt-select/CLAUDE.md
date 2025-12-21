# wt select Demo Recording

## Running the Demo

```bash
./docs/demos/wt-select/build
```

Creates:
- `docs/demos/wt-select/out/wt-select.gif` - Light theme demo
- `docs/demos/wt-select/out/wt-select-dark.gif` - Dark theme demo
- Demo repo at `docs/demos/wt-select/out/.demo-select/` (gitignored)

Theme colors are defined in `docs/demos/shared/themes.py` to match the doc site's CSS variables.

## How It Works

Uses the **unified demo infrastructure** (`prepare_demo_repo()` from `shared/lib.py`), same as wt-core and wt-merge demos. The repo is a synthetic "acme" Rust project with alpha/beta/hooks branches designed to showcase column variety.

Branch setup (from shared infrastructure):
- **alpha** - Large working tree changes, unpushed commits, PR CI
- **beta** - Staged changes, behind main, branch CI
- **hooks** - Staged+unstaged changes, no remote

The demo navigates to alpha to show the large committed diff in the main…± panel.

## Viewing Results

**Do NOT use `open` on the GIF** - that's for the user to do manually.

Inline viewing options:
- Quick Look: `qlmanage -p docs/demos/wt-select/out/wt-select.gif`
- iTerm2: `imgcat docs/demos/wt-select/out/wt-select.gif`

## Prerequisites

- `wt` (worktrunk) installed and in PATH
- `starship` for prompt
- **Custom VHS fork** with keystroke overlay (**required** - standard VHS won't work)

### Building the VHS Fork

The demo requires a custom VHS fork that displays keystroke overlays. **You must build this before running the demo:**

```bash
cd docs/demos/wt-select
git clone -b keypress-overlay https://github.com/max-sixty/vhs.git vhs-keystrokes
cd vhs-keystrokes
go build -o vhs-keystrokes .
```

The build script looks for the binary at `docs/demos/wt-select/vhs-keystrokes/vhs-keystrokes`.

**Why custom VHS?** The fork adds a large keystroke overlay in the center of the screen, showing what keys are being pressed. This is essential for demo GIFs where viewers need to see navigation keys (↓, Ctrl+D, etc.).

Override path with: `VHS_KEYSTROKES=/path/to/binary ./build`

### Keystroke Timing Calibration

The keystroke overlay timing is controlled by `keystrokeDelayMS` in `ffmpeg.go`:

```go
keystrokeDelayMS  = 500.0   // Delay to sync with terminal rendering
```

**How this was calibrated:**
1. The overlay must appear synchronized with when the terminal responds to the keystroke
2. Initial value (600ms) showed keystrokes appearing ~240ms LATE (after terminal changed)
3. Frame-by-frame GIF analysis (25fps = 40ms/frame) revealed the exact offset
4. Reduced to 500ms achieves perfect sync - keystroke and terminal change on same frame

**To recalibrate if needed:**
```bash
# Extract frames from GIF
ffmpeg -i demo.gif -vsync 0 /tmp/gif-frames/frame_%04d.png

# Compare frames to find when terminal changes vs when keystroke appears
# Adjust keystrokeDelayMS: increase if keystroke appears too early, decrease if too late
```

## Files

- `build` - Main build script (uses shared infrastructure from `docs/demos/shared/`)
- `demo.tape` - VHS tape file with recording script
- `out/` - Output directory (gitignored)

Starship config comes from shared `docs/demos/shared/fixtures/`.
