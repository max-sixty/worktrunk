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
Env GIT_TERMINAL_PROMPT "0"

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

# 1. Show current worktrees
Type "wt list"
Enter
Sleep 2s

# 2. Switch to alpha worktree
Type "wt switch alpha"
Enter
Sleep 1s

# 3. Show the hooks config (bat syntax highlighting via cat wrapper)
Type "cat .config/wt.toml"
Enter
Sleep 2s

# 4. Create new worktree with post-start
Type "wt switch --create api"
Enter
Sleep 2.5s

# 5. Show full list with CI status
Type "wt list --full"
Enter
Sleep 2.5s

# 6. Switch to billing (already merged)
Type "wt switch billing"
Enter
Sleep 1.5s

# 7. Remove the merged worktree (pre-remove hook runs)
Type "wt remove"
Enter
Sleep 5s

Type "exit"
Enter
Sleep 500ms
