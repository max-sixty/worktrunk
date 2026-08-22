# Common requirements and environment - Source after Output
#
# Hide must be the first executable directive. VHS only runs pre-roll before
# starting its recorder when the first non-Set/Output/Require directive is Hide.
Hide

Require wt
Require git

Env IS_DEMO "1"
Env GIT_TERMINAL_PROMPT "0"
Env GIT_PAGER "{{GIT_PAGER}}"
Env DEMO_REPO "{{DEMO_REPO}}"
Env RUSTUP_HOME "{{REAL_HOME}}/.rustup"
Env CARGO_HOME "{{REAL_HOME}}/.cargo"
Env HOME "{{DEMO_HOME}}"
Env XDG_CONFIG_HOME "{{DEMO_HOME}}/.config"
Env STARSHIP_CONFIG "{{STARSHIP_CONFIG}}"

Type "set -g fish_autosuggestion_enabled 0"
Enter
Type "source $HOME/.config/fish/colors.fish"
Enter
Type "set -gx LANG en_US.UTF-8; set -gx LC_ALL en_US.UTF-8"
Enter
Type "set -gx PATH {{TARGET_DEBUG}} $HOME/.local/bin $PATH"
Enter
Type "starship init fish | source"
Enter
Type "wt config shell init fish | source"
Enter
Type "source ~/.config/fish/completions/wt.fish"
Enter
Type "cd $DEMO_REPO"
Enter
