"""VHS themes coordinated with the documentation site's color palette.

The base surfaces and accents follow ``docs/src/styles/custom.css``. ANSI hues
are tuned separately for legible terminal captures in each theme.
"""

import json

# Light theme — based on the default ``--wt-*`` palette in custom.css.
LIGHT_THEME = {
    "name": "Warm Gold Light",
    "black": "#6b7280",  # --bright-black
    "red": "#dc2626",  # --red
    "green": "#357a59",  # --green (desaturated from website's #1b7f4b)
    "yellow": "#ca8a04",  # --yellow
    "blue": "#2563eb",  # --blue
    "magenta": "#9333ea",  # --magenta
    "cyan": "#3d7f7f",  # --cyan (muted from website's #0a8080)
    "white": "#8c959f",
    "brightBlack": "#6b7280",  # --bright-black
    "brightRed": "#ef4444",
    "brightGreen": "#4a9b76",
    "brightYellow": "#eab308",
    "brightBlue": "#3b82f6",
    "brightMagenta": "#a855f7",
    "brightCyan": "#5a9e9e",
    "brightWhite": "#8c959f",
    "background": "#f7f3eb",  # --wt-paper
    "foreground": "#27231f",  # --wt-ink
    "cursor": "#d85d22",  # --wt-orange
    "selection": "#f7d6c1",  # --sl-color-accent-low
}

# Dark theme — based on the ``data-theme='dark'`` palette in custom.css.
DARK_THEME = {
    "name": "Warm Workbench Dark",
    "black": "#6b7280",  # --bright-black from CSS
    "red": "#f87171",  # --red dark mode
    "green": "#4ade80",  # --green dark mode
    "yellow": "#fbbf24",  # --yellow dark mode
    "blue": "#60a5fa",  # --blue dark mode
    "magenta": "#c084fc",  # --magenta dark mode
    "cyan": "#67d4d4",  # --cyan dark mode
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
    "cursor": "#ef8a50",  # --wt-orange
    "selection": "#49200f",  # --sl-color-accent-low
}

THEMES = {
    "light": LIGHT_THEME,
    "dark": DARK_THEME,
}


def format_theme_for_vhs(theme: dict) -> str:
    """Format a theme dict as a VHS Set Theme command value."""
    return json.dumps(theme)
