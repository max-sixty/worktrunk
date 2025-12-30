Output "{{OUTPUT_GIF}}"

Require wt
Require git

Set Shell "fish"
Set FontSize {{FONTSIZE}}
Set FontFamily "Geist Mono, Berkeley Mono, Fira Code, JetBrains Mono, Menlo, Apple Color Emoji"
Set Width {{WIDTH}}
Set Height {{HEIGHT}}
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
Env GIT_TERMINAL_PROMPT "0"
Env ANTHROPIC_API_KEY "{{ANTHROPIC_API_KEY}}"

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
# Pre-install marketplace plugin before recording
Type "claude plugin marketplace add anthropics/claude-plugins-official"
Enter
Sleep 5s
Type "clear"
Enter
Sleep 500ms
Show
Sleep 500ms

# Switch to alpha worktree first (has uncommitted changes for diff stats)
Type "wt switch alpha"
Enter
Sleep 1.5s

# Launch Claude Code to show statusline integration
# Tweet 12 is about seeing branch, diff stats, CI status at a glance in Claude's UI
# Statusline will show: ~/w/acme.alpha  alpha  +N  ●  | Opus 4.5
# Pre-enter pause for big UI takeover
Sleep 1s
Type "claude"
Enter
Sleep 8s
