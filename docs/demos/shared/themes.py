"""VHS theme definitions matching the doc site color palette.

These themes are designed to match the CSS variables in docs/templates/_variables.html
so demos integrate seamlessly with light/dark mode switching.
"""

import json

# Light theme - matches the "Warm Workbench" light mode
# Terminal colors from docs/templates/_variables.html (light mode)
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
    "background": "#FFFBF0",
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
    "black": "#6b7280",  # --bright-black from CSS
    "red": "#f87171",  # --red dark mode
    "green": "#4ade80",  # --green dark mode
    "yellow": "#fbbf24",  # --yellow dark mode
    "blue": "#60a5fa",  # --blue dark mode
    "magenta": "#c084fc",  # --magenta dark mode
    "cyan": "#67d4d4",  # --cyan dark mode
    "white": "#a8a29e",  # --wt-color-text-muted
    "brightBlack": "#6b7280",  # same as black
    "brightRed": "#fca5a5",  # lighter red
    "brightGreen": "#86efac",  # lighter green
    "brightYellow": "#fde047",  # lighter yellow
    "brightBlue": "#93c5fd",  # lighter blue
    "brightMagenta": "#d8b4fe",  # lighter magenta
    "brightCyan": "#a5f3fc",  # lighter cyan
    "brightWhite": "#e8e6e3",  # --wt-color-text
    "background": "#1c1b1a",  # --wt-color-bg dark mode
    "foreground": "#e8e6e3",  # --wt-color-text dark mode
    "cursor": "#f59e0b",  # --wt-color-accent dark mode
    "selection": "#422006",  # --wt-color-accent-soft dark mode
}

THEMES = {
    "light": LIGHT_THEME,
    "dark": DARK_THEME,
}


def format_theme_for_vhs(theme: dict) -> str:
    """Format a theme dict as a VHS Set Theme command value."""
    return json.dumps(theme)
