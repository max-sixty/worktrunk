"""VHS theme definitions matching the doc site color palette.

These themes are designed to match the CSS variables in docs/templates/_variables.html
so demos integrate seamlessly with light/dark mode switching.
"""

import json

# Light theme - matches the "Warm Workbench" light mode
# Based on the original "Warm Gold Light" theme with doc site alignment
LIGHT_THEME = {
    "name": "Warm Gold Light",
    "black": "#8c959f",
    "red": "#d73a49",
    "green": "#22863a",
    "yellow": "#d29922",
    "blue": "#0969da",
    "magenta": "#8250df",
    "cyan": "#1b7c83",
    "white": "#8c959f",
    "brightBlack": "#8c959f",
    "brightRed": "#cb2431",
    "brightGreen": "#2ea043",
    "brightYellow": "#f2cc60",
    "brightBlue": "#218bff",
    "brightMagenta": "#a475f9",
    "brightCyan": "#39c5cf",
    "brightWhite": "#8c959f",
    "background": "#FFFDF8",
    "foreground": "#1f2328",
    "cursor": "#d97706",
    "selection": "#FFF0C8",
}

# Dark theme - matches the "Warm Workbench" dark mode from _variables.html
# Colors derived from the CSS custom properties in dark mode:
#   --wt-color-bg: #1c1b1a
#   --wt-color-text: #e8e6e3
#   --wt-color-accent: #f59e0b
#   Terminal colors: --cyan: #67d4d4, --green: #4ade80, --red: #f87171, etc.
DARK_THEME = {
    "name": "Warm Workbench Dark",
    "black": "#6b7280",           # --bright-black from CSS
    "red": "#f87171",             # --red dark mode
    "green": "#4ade80",           # --green dark mode
    "yellow": "#fbbf24",          # --yellow dark mode
    "blue": "#60a5fa",            # --blue dark mode
    "magenta": "#c084fc",         # --magenta dark mode
    "cyan": "#67d4d4",            # --cyan dark mode
    "white": "#a8a29e",           # --wt-color-text-muted
    "brightBlack": "#6b7280",     # same as black
    "brightRed": "#fca5a5",       # lighter red
    "brightGreen": "#86efac",     # lighter green
    "brightYellow": "#fde047",    # lighter yellow
    "brightBlue": "#93c5fd",      # lighter blue
    "brightMagenta": "#d8b4fe",   # lighter magenta
    "brightCyan": "#a5f3fc",      # lighter cyan
    "brightWhite": "#e8e6e3",     # --wt-color-text
    "background": "#1c1b1a",      # --wt-color-bg dark mode
    "foreground": "#e8e6e3",      # --wt-color-text dark mode
    "cursor": "#f59e0b",          # --wt-color-accent dark mode
    "selection": "#422006",       # --wt-color-accent-soft dark mode
}

THEMES = {
    "light": LIGHT_THEME,
    "dark": DARK_THEME,
}


def format_theme_for_vhs(theme: dict) -> str:
    """Format a theme dict as a VHS Set Theme command value."""
    return json.dumps(theme)
