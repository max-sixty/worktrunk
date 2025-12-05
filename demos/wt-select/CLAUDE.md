# wt select Demo Recording

## Running the Demo

```bash
./demos/wt-select/build
```

Creates:
- `demos/wt-select/out/wt-select.gif` - Animated demo
- Demo repo at `demos/wt-select/out/.demo-select/` (gitignored)

## Demo Goals

The demo showcases `wt select` with **realistic variety in all columns**:

```
Branch         Status         HEAD±    main↕     main…±  Remote⇅  CI  Age
@ main               ^                                              ○   now
+ streaming      +           +54   -5                               ●   now
+ doctor             ↕                  ↑1  ↓1  +320  -14           ●   2d
+ llm-templates   !  ↕        +8        ↑1  ↓1  +263 -192               3d
```

Key demonstration points:
- **CI column**: hollow ○ (branch CI) vs filled ● (PR CI) vs none
- **HEAD± column**: Large staged diff (+54), small unstaged (+8), none
- **Status column**: Staged changes (+), unstaged (!), ahead/behind (↕)
- **main↕ column**: Some branches ahead-only, some ahead-and-behind
- **main…± column**: Meaningful commit diffs (small to 300+ lines)

## How It Works

**IMPORTANT: The setup is carefully orchestrated. The sequence in `prepare_repo()` matters!**

Uses **actual commits from this repository** cherry-picked onto v0.1.11:

- **Base**: v0.1.11 tag (005db9ad)
- **Branches via cherry-pick** (simple names, no `/` to avoid path mismatch):
  - `streaming` - cf667917 (Handle BrokenPipe)
  - `doctor` - e286e847 (Add --doctor option, +320/-14)
  - `llm-templates` - 74fe46ff (Enhance squash messages, +263/-192)

Special setup tricks:
1. **Soft reset** streaming to main creates large staged HEAD± diff
2. **Manual code additions** add more staged/unstaged changes
3. **Fake CI cache** with future timestamps prevents expiration during recording

## CI Cache Trick

CI status is cached in git config. To show CI without GitHub access:
1. Write fake cache entries directly to git config
2. Use **future timestamp** (1 hour ahead) so cache never expires
3. VHS recording reads cached status

Without the future timestamp, cache expires during recording → tries to fetch → fails → clears cache.

## Viewing Results

**Do NOT use `open` on the GIF** - that's for the user to do manually.

Inline viewing options:
- Quick Look: `qlmanage -p dev/wt-select-demo/out/wt-select-demo.gif`
- iTerm2: `imgcat dev/wt-select-demo/out/wt-select-demo.gif`

## Prerequisites

- `wt` (worktrunk) installed and in PATH
- Custom `vhs` fork with keystroke overlay (see below)
- `starship` for prompt

## VHS Fork with Keystroke Overlay

This demo uses a **custom fork of VHS** with keystroke overlay support:

**GitHub:** https://github.com/max-sixty/vhs/tree/keypress-overlay

The fork adds a large keystroke overlay that appears in the center of the screen, showing what keys are being pressed. This is essential for demo GIFs where viewers need to see what's being typed.

To build (from the `demos/wt-select/` directory):
```bash
git clone -b keypress-overlay https://github.com/max-sixty/vhs.git vhs-keystrokes
cd vhs-keystrokes
go build -o vhs-keystrokes .
```

The build script looks for the binary at `demos/wt-select/vhs-keystrokes/vhs-keystrokes` by default.
Override with: `VHS_KEYSTROKES=/path/to/binary ./build`

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

- `build` - Main build script
- `demo.tape` - VHS tape file with recording script
- `fixtures/` - Starship config and other fixtures
- `out/` - Output directory (gitignored)

## Updating Commits

To update the cherry-picked commits, edit `CHERRY_PICKS` in `build`:

```python
CHERRY_PICKS = {
    "branch-name": ("commit-hash", days_ago),
    ...
}
```

Test cherry-picks apply cleanly before updating:

```bash
cd /tmp && git clone --quiet /path/to/worktrunk test-repo
cd test-repo && git checkout v0.1.11
git cherry-pick --no-commit <new-commit-hash>
```

**Note:** Use simple branch names without `/` (e.g., `streaming` not `feature/streaming`) to avoid path mismatch issues in wt list.
