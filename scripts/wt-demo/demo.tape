Output "{{OUTPUT_GIF}}"

Require wt
Require git

Set Shell "bash"
Set FontSize 19
Set FontFamily "Menlo, SF Mono, JetBrains Mono, Consolas, Apple Color Emoji"
Set Width 1280
Set Height 720
Set Theme { "name": "GitHub Light Custom", "black": "#24292f", "red": "#d73a49", "green": "#22863a", "yellow": "#d29922", "blue": "#0969da", "magenta": "#8250df", "cyan": "#1b7c83", "white": "#6e7781", "brightBlack": "#57606a", "brightRed": "#cb2431", "brightGreen": "#2ea043", "brightYellow": "#f2cc60", "brightBlue": "#218bff", "brightMagenta": "#a475f9", "brightCyan": "#39c5cf", "brightWhite": "#1f2328", "background": "#ffffff", "foreground": "#1f2328", "cursor": "#0969da", "selection": "#ddf4ff" }
Set Padding 18
Set LineHeight 1.12
Set TypingSpeed 28ms
Set CursorBlink false
Set Framerate 30

Env DEMO_REPO "{{DEMO_REPO}}"
Env STARSHIP_CONFIG "{{STARSHIP_CONFIG}}"

Hide
Type "export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8"
Enter
Type 'eval "$(starship init bash)" >/dev/null 2>&1'
Enter
Type 'eval "$(wt init bash)" >/dev/null 2>&1'
Enter
Type "cd $DEMO_REPO"
Enter
Type "clear"
Enter
Show

Type "wt list --branches --full"
Enter
Sleep 1.6s

Type "wt switch --create feature/reports --base main"
Enter
Sleep 1.6s

Type 'echo "- Q4 report ready" >> notes.md'
Enter
Sleep 0.8s

Type "wt merge"
Enter
Sleep 2.2s

Type "wt list --branches --full"
Enter
Sleep 1.5s

Type "exit"
Enter
Sleep 700ms
