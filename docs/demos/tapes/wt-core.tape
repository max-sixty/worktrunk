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
Type "source ~/.config/fish/completions/wt.fish"
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

# 3. Create new worktree with post-start
Type "wt switch --create api"
Enter
Sleep 2.5s

# 4. Show full list with CI status
Type "wt list --full"
Enter
Sleep 3s

# 5. Switch to billing using Tab cycling through branch options
# NOTE: Maintainer marked this as excellent - showcases completion well
# - "wt switch b" + Tab → shows billing/beta menu (400ms to review options)
# - Tab → cycles through options, selecting billing (300ms to see selection)
# - Enter → accepts selection (dismisses pager)
# - 50ms then Enter again → executes command
Type "wt switch b"
Tab
Sleep 400ms
Tab
Sleep 300ms
Enter
Sleep 50ms
Enter
Sleep 1.5s

# 6. Remove the merged worktree (pre-remove hook runs)
Type "wt remove"
Enter
Sleep 5s

Type "exit"
Enter
Sleep 500ms
