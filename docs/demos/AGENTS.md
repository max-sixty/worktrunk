# Demo development

Tapes live in docs/demos/tapes/; shared setup, themes, fixtures, and OCR checkpoints live in shared/. The build script produces gitignored docs/public/assets/ output. The separate assets repository holds published GIFs.

## Recording and validation

- Build a recording with `./docs/demos/build docs --only <name>` or `./docs/demos/build social --only <name>`. Docs produces light and dark variants; social produces light only. Each theme needs a fresh demo environment because tapes mutate it.
- Use --snapshot for command-output regression snapshots. TUI recordings need GIF inspection and OCR checkpoints in shared/validation.py because text snapshots cannot see nested terminal content. Measure checkpoint windows in the actual GIF; frame counts drift with execution time and terminal size.
- Use --shell to inspect a prepared demo environment interactively. The build fetches its VHS fork and other dependencies; Claude demos use the authenticated account.
- Inspect sampled frames across the finished GIF for split commands, warnings, wrong panes, and layout glitches. Confirm visible controls and text at representative states before publishing.
- Publish with task publish-assets only when authorized; task fetch-assets retrieves published files. Local builds and fetched assets share docs/public/assets/.

The VHS fork checkout under docs/demos/.deps/vhs/ is gitignored. Changes to the fork do not ship with this repository; publish them in that fork explicitly when authorized.

## Tape behavior

Keep displayed commands natural; put recording-only flags in setup. Pause long enough to read results, with short gaps between related keystrokes. The tape's opening command is typed before Show so the first frame contains it complete. Hidden keystrokes land together on the first visible frame.

For Alt keybindings, the VHS fork passes macOptionIsMeta to ttyd; quote digit keys as Alt+"8". If a key seems wrong, verify its bytes in a recording before changing the tape.

The mock forge data is written by write_gh_mock_data in shared/lib.py and served by fixtures/gh-mock.sh. A parseable origin, successful gh --version and gh auth status, and matching DEMO_PROJECT_ID are required before CI cells appear. Staggered response delays demonstrate progressive rows; keep the longest within the tape's wait.

The picker recording uses prepare_picker and its own smaller text size to fit the CI, summary, and preview columns. Its filter searches PR metadata as well as branch names, and narrowing preserves cursor row index; use an unambiguous query before cursor movement.

The site palette in docs/src/styles/custom.css feeds shared/themes.py, so re-record demos after palette changes. Do not kill generic Zellij processes while cleaning up a demo run; identify the demo process specifically.
