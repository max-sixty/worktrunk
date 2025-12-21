Output "{{OUTPUT_GIF}}"

Require wt
Require git

Set Shell "fish"
Set FontSize 24
Set FontFamily "Geist Mono, Berkeley Mono, Fira Code, JetBrains Mono, Menlo, Apple Color Emoji"
Set Width 1600
Set Height 900
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
Env NEXTEST_STATUS_LEVEL "none"
Env NEXTEST_FINAL_STATUS_LEVEL "flaky"
Env NEXTEST_NO_FAIL_FAST "1"

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
Sleep 500ms

Type "wt list"
Enter
Sleep 1.6s

Type "wt switch --create notes"
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
Sleep 8s

Type "wt list --branches --full"
Enter
Sleep 1.5s

Sleep 4.5s
Type "exit"
Enter
Sleep 700ms
