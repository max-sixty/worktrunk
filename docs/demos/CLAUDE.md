# Demo Development

## Directory structure

```
docs/demos/
  build            # Unified build script
  tapes/           # All VHS tape files (templated)
  out/             # Output GIFs (gitignored)
  shared/          # Python library, themes, fixtures
  vhs-keystrokes/  # Custom VHS binary (gitignored, built on demand)
```

Tape files use template variables (`{{FONTSIZE}}`, `{{WIDTH}}`, `{{HEIGHT}}`) so the same tape produces different sizes for docs vs Twitter.

## Regenerating demos

```bash
./docs/demos/build docs      # Doc site demos (1600x900, light + dark)
./docs/demos/build twitter   # Twitter demos (1200x700, light only)
```

Regenerate a single demo:

```bash
./docs/demos/build twitter --only wt-switch
./docs/demos/build docs --only wt-merge
```

**Available demos:**

| Target | Demos |
|--------|-------|
| docs | wt-core, wt-merge, wt-select, wt-zellij-omnibus |
| twitter | wt-switch, wt-statusline, wt-list, wt-list-remove, wt-hooks, wt-devserver, wt-commit, wt-merge, wt-select-short, wt-core, wt-zellij, wt-zellij-omnibus |

## vhs-keystrokes setup (REQUIRED for wt-select demos)

The `wt-select` demos require a custom VHS fork with keystroke overlay. **Claude must build this binary before regenerating demos.**

Check if the binary exists:

```bash
ls docs/demos/vhs-keystrokes/vhs-keystrokes
```

If missing, **build it** (requires Go):

```bash
cd docs/demos
git clone -b keypress-overlay https://github.com/max-sixty/vhs.git vhs-keystrokes
cd vhs-keystrokes && go build -o vhs-keystrokes .
```

The binary is gitignored. Build scripts skip wt-select GIF recording if missing—**always build vhs-keystrokes first** when regenerating demos.

## Light/dark theme variants

The docs build generates both light and dark GIF variants:
- `wt-core.gif` / `wt-core-dark.gif`
- `wt-merge.gif` / `wt-merge-dark.gif`
- `wt-select.gif` / `wt-select-dark.gif`

Twitter build generates light only (Twitter doesn't support theme-switching media queries).

Theme definitions are in `docs/demos/shared/themes.py`, matching the CSS variables in `_variables.html`.

## Debugging a demo environment

Use `--shell` to spawn an interactive fish shell with the demo environment:

```bash
./docs/demos/build twitter --only wt-switch --shell
```

This builds the demo and drops you into a fish shell with `HOME`, `PATH`, starship, and wt shell integration all configured. You're already in the demo repo and ready to test:

```fish
# Now you can manually test:
claude                                    # See what happens on first launch
wt switch --create foo                    # Create a worktree
wt switch --execute claude --create bar   # Test the demo command
```

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
- `$HOME/.config/worktrunk/config.toml` - Worktrunk user config (approved commands)
- `$HOME/w/acme/.config/wt.toml` - Project hooks config

Key fields in `.claude.json` for suppressing notifications:
- `officialMarketplaceAutoInstalled: true` - should suppress marketplace auto-install
- `numStartups: 100` - makes Claude think it's been run many times
- `hasCompletedOnboarding: true` - skips onboarding

## Extracting frames from a GIF for inspection

```bash
mkdir -p /tmp/frames
magick docs/demos/out/wt-switch.gif -coalesce /tmp/frames/frame_%03d.png

# View a specific frame
open /tmp/frames/frame_200.png
```

## Cleaning up stale demo processes

**NEVER run `pkill -f zellij`** — this kills the user's own Zellij session, not just demo processes.

If stale Zellij processes from previous demo runs are causing issues, either:
- Let them die on their own (they'll timeout)
- Target only demo processes: `pkill -f "zellij.*wt-demos"`
- Remove the demo directory and rebuild: `rm -rf /private/tmp/wt-demos`
