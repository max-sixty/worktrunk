Output "{{OUTPUT_GIF}}"

Require wt
Require git

Set Shell "fish"
Set FontSize 19
Set FontFamily "Geist Mono, Berkeley Mono, Fira Code, JetBrains Mono, Menlo, Apple Color Emoji"
Set Width 1600
Set Height 800
Set Theme {{THEME}}
Set Padding 18
Set LineHeight 1.12
Set TypingSpeed 28ms
Set CursorBlink false
Set Framerate 30

Env DEMO_REPO "{{DEMO_REPO}}"
Env RUSTUP_HOME "{{REAL_HOME}}/.rustup"
Env CARGO_HOME "{{REAL_HOME}}/.cargo"
Env HOME "{{DEMO_HOME}}"
Env XDG_CONFIG_HOME "{{DEMO_HOME}}/.config"
Env STARSHIP_CONFIG "{{STARSHIP_CONFIG}}"

Hide
Type "set -g fish_autosuggestion_enabled 0"
Enter
Type "set -gx LANG en_US.UTF-8; set -gx LC_ALL en_US.UTF-8"
Enter
Type "set -gx PATH {{TARGET_DEBUG}} $HOME/bin $PATH"
Enter
Type "starship init fish | source"
Enter
Type "wt config shell init fish | source"
Enter
Type "cd $DEMO_REPO"
Enter
Type "clear"
Enter
Sleep 500ms
Show
Set KeyStrokes Show
Sleep 500ms

# Launch wt select
Type "wt select"
Enter
Sleep 2s

# Navigate to doctor branch (has large main…± diff: +320 -14)
Down
Sleep 1s
Down
Sleep 1.5s

# Switch to main…± tab (panel 3) to see committed diff from main
Type "3"
Sleep 1.5s

# Browse the diff with Ctrl+D (shows scrolling through large diff)
Ctrl+d
Sleep 1s
Ctrl+d
Sleep 1s
Ctrl+d
Sleep 1.5s

# Filter to narrow down and select
Type "str"
Sleep 1.5s
Enter
Sleep 2s

# Show we're now in the worktree
Sleep 500ms

Type "exit"
Enter
Sleep 500ms
