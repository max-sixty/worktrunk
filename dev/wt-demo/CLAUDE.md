# Demo Recording

## Running the Demo

```console
./dev/wt-demo-build
```

Creates:
- `dev/wt-demo/out/wt-demo.gif` - Animated demo (~1.9 MB)
- `dev/wt-demo/out/run.txt` - Text transcript
- `dev/wt-demo/out/record.log` - Recording log

The script creates a fresh temp repo under `dev/wt-demo/out/.demo-*/`, seeds 4 worktrees + 2 extra branches, shows `wt list`, creates a worktree, edits a file, merges with `wt merge`, then shows `wt list --branches --full`.

## Viewing Results

**Do NOT use `open` on the GIF** - that's for the user to do manually.

Inline viewing options:
- Quick Look: `qlmanage -p dev/wt-demo/out/wt-demo.gif`
- iTerm2: `imgcat dev/wt-demo/out/wt-demo.gif`

For Claude Code: read `dev/wt-demo/out/run.txt` to see text output (cannot view GIFs directly).

## Prerequisites

- `wt` (worktrunk) installed and in PATH
- `vhs` for recording
- `starship` for prompt
- `llm` CLI with Claude model configured (for commit message generation)
- `cargo-nextest` for running tests
- Python 3

## Files

- `demo.tape` - VHS tape file with recording script
- `fixtures/` - Extracted content files (README, lib.rs, etc.)
- `out/` - Output directory (gitignored)

## Defaults

Light theme (GitHub-light-inspired palette), starship prompt, 1600x720 window.
