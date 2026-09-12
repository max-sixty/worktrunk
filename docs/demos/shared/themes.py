"""Demo recording palettes, read from the documentation site's stylesheet.

The VHS terminal, Zellij, and starship colors come from hex ``--wt-*`` custom
properties in ``docs/src/styles/custom.css``: the light theme from its
``:root`` block, the dark theme from ``:root[data-theme='dark']``. A recording
uses whatever palette the site has when the build runs. Each property a
recording uses must be declared as a lowercase six-digit hex color in both
blocks; any other form is a ``KeyError`` rather than a silently wrong color.

The VHS terminal theme maps ANSI colors to properties the way the site renders
snapshot output (``terminal_color_class`` and ``terminal_background_class`` in
``tests/integration_tests/readme_sync.rs``). A bright hue uses its normal hue's
property. Bright black and white text use ``--wt-ink-muted``, the site's gray;
black, which the site never renders, matches them. Bright white uses
``--wt-terminal-gutter``, because Worktrunk draws its gutter with a bright-white
background.
"""

import json
import re
from pathlib import Path

CUSTOM_CSS = Path(__file__).parents[2] / "src" / "styles" / "custom.css"


def _hex_properties(css: str, selector: str) -> dict[str, str]:
    """Hex ``--wt-*`` properties declared in the top-level ``selector`` block."""
    start = css.index(f"\n{selector} {{\n")
    end = css.index("\n}\n", start)
    declarations = re.findall(
        r"^\s+(--wt-[\w-]+): (#[0-9a-f]{6});$", css[start:end], re.MULTILINE
    )
    return dict(declarations)


def _site_palettes() -> dict[str, dict[str, str]]:
    css = CUSTOM_CSS.read_text()
    return {
        "light": _hex_properties(css, ":root"),
        "dark": _hex_properties(css, ":root[data-theme='dark']"),
    }


PALETTES = _site_palettes()


def _vhs_theme(theme: str) -> dict[str, str]:
    palette = PALETTES[theme]
    hues = {
        hue: palette[f"--wt-terminal-{hue}"]
        for hue in ("red", "green", "yellow", "blue", "magenta", "cyan")
    }
    gray = palette["--wt-ink-muted"]
    return {
        "name": f"worktrunk-{theme}",
        "black": gray,
        **hues,
        "white": gray,
        "brightBlack": gray,
        **{f"bright{hue.title()}": color for hue, color in hues.items()},
        "brightWhite": palette["--wt-terminal-gutter"],
        "background": palette["--wt-paper"],
        "foreground": palette["--wt-terminal-ink"],
        "cursor": palette["--wt-copper"],
        "selection": palette["--wt-gold-wash"],
    }


THEMES = {theme: _vhs_theme(theme) for theme in PALETTES}


def format_theme_for_vhs(theme: dict) -> str:
    """Format a theme dict as a VHS Set Theme command value."""
    return json.dumps(theme)
