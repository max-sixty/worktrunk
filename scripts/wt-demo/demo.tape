Output "{{OUTPUT_GIF}}"

Require wt
Require git

Set Shell "zsh"
Set FontSize 19
Set FontFamily "Geist Mono, Berkeley Mono, Fira Code, JetBrains Mono, Menlo, Apple Color Emoji"
Set Width 1280
Set Height 720
Set Theme { "name": "GitHub Light Custom", "black": "#8c959f", "red": "#d73a49", "green": "#22863a", "yellow": "#d29922", "blue": "#0969da", "magenta": "#8250df", "cyan": "#1b7c83", "white": "#8c959f", "brightBlack": "#8c959f", "brightRed": "#cb2431", "brightGreen": "#2ea043", "brightYellow": "#f2cc60", "brightBlue": "#218bff", "brightMagenta": "#a475f9", "brightCyan": "#39c5cf", "brightWhite": "#8c959f", "background": "#ffffff", "foreground": "#1f2328", "cursor": "#0969da", "selection": "#ddf4ff" }
Set Padding 18
Set LineHeight 1.12
Set TypingSpeed 28ms
Set CursorBlink false
Set Framerate 30

Env DEMO_REPO "{{DEMO_REPO}}"
Env RUSTUP_HOME "{{REAL_HOME}}/.rustup"
Env CARGO_HOME "{{REAL_HOME}}/.cargo"
Env HOME "{{DEMO_HOME}}"
Env STARSHIP_CONFIG "{{STARSHIP_CONFIG}}"

Hide
Type "export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8"
Enter
Type "export PATH=$HOME/bin:$PATH"
Enter
Type 'eval "$(starship init zsh)" >/dev/null 2>&1'
Enter
Type 'eval "$(wt config shell init zsh)" >/dev/null 2>&1'
Enter
Type "cd $DEMO_REPO"
Enter
Type "clear"
Enter
Show

Type "wt list"
Enter
Sleep 1.6s

Type "wt switch --create feature/reports"
Enter
Sleep 1.6s

Type 'echo "- Q4 report ready" >> notes.md'
Enter
Sleep 0.8s

Type "wt list"
Enter
Sleep 1.6s

Type "wt merge"
Enter
Sleep 6.5s

Type "wt list --branches --full"
Enter
Sleep 1.5s

Sleep 3s
Type "exit"
Enter
Sleep 700ms
