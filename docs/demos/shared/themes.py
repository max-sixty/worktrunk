"""VHS themes coordinated with the documentation site's color palette.

Each comment names the custom property in ``docs/src/styles/custom.css`` that
the value tracks. Every ANSI hue here predates the Starlight rebuild: the ones
the site has since changed quote its current hex, and the ones it kept are
verbatim copies. Surfaces sit within a few units of their property; the cursor
and selection accents are tuned brighter for legible terminal captures.
"""

import json

# Light theme — based on the default ``--wt-*`` palette in custom.css.
LIGHT_THEME = {
    "name": "Warm Gold Light",
    "black": "#6b7280",  # --wt-terminal-gutter (pre-Starlight; site is #a8a29e)
    "red": "#dc2626",  # --wt-terminal-red (pre-Starlight; site is #b42318)
    "green": "#357a59",  # --wt-terminal-green (pre-Starlight; site is #256b4a)
    "yellow": "#ca8a04",  # --wt-terminal-yellow (pre-Starlight; site is #8a5a00)
    "blue": "#2563eb",  # --wt-terminal-blue (pre-Starlight; site is #1d4ed8)
    "magenta": "#9333ea",  # --wt-terminal-magenta (pre-Starlight; site is #7e22ce)
    "cyan": "#3d7f7f",  # --wt-terminal-cyan (pre-Starlight; site is #0f6f73)
    "white": "#8c959f",
    "brightBlack": "#6b7280",  # --wt-terminal-gutter (pre-Starlight; site is #a8a29e)
    "brightRed": "#ef4444",
    "brightGreen": "#4a9b76",
    "brightYellow": "#eab308",
    "brightBlue": "#3b82f6",
    "brightMagenta": "#a855f7",
    "brightCyan": "#5a9e9e",
    "brightWhite": "#8c959f",
    "background": "#f7f3eb",  # --wt-paper
    "foreground": "#27231f",  # --wt-ink
    "cursor": "#d85d22",  # --wt-copper
    "selection": "#f7d6c1",  # --wt-gold-wash (--sl-color-accent-low)
}

# Dark theme — based on the ``data-theme='dark'`` palette in custom.css.
DARK_THEME = {
    "name": "Warm Workbench Dark",
    "black": "#6b7280",  # --wt-terminal-gutter
    "red": "#f87171",  # --wt-terminal-red
    "green": "#4ade80",  # --wt-terminal-green (pre-Starlight; site is #6ee7a2)
    "yellow": "#fbbf24",  # --wt-terminal-yellow (pre-Starlight; site is #facc15)
    "blue": "#60a5fa",  # --wt-terminal-blue (pre-Starlight; site is #93c5fd)
    "magenta": "#c084fc",  # --wt-terminal-magenta (pre-Starlight; site is #d8b4fe)
    "cyan": "#67d4d4",  # --wt-terminal-cyan
    "white": "#a8a29e",
    "brightBlack": "#6b7280",  # same as black
    "brightRed": "#fca5a5",  # lighter red
    "brightGreen": "#86efac",  # lighter green
    "brightYellow": "#fde047",  # lighter yellow
    "brightBlue": "#93c5fd",  # lighter blue
    "brightMagenta": "#d8b4fe",  # lighter magenta
    "brightCyan": "#a5f3fc",  # lighter cyan
    "brightWhite": "#eee8de",  # --wt-ink
    "background": "#1d1a18",  # --wt-paper
    "foreground": "#eee8de",  # --wt-ink
    "cursor": "#ef8a50",  # --wt-copper
    "selection": "#49200f",  # --wt-gold-wash (--sl-color-accent-low)
}

THEMES = {
    "light": LIGHT_THEME,
    "dark": DARK_THEME,
}


def format_theme_for_vhs(theme: dict) -> str:
    """Format a theme dict as a VHS Set Theme command value."""
    return json.dumps(theme)
