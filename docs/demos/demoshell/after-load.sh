# Bash rcfile for Demoshell's "Start an app" command; see README.md.
. "$HOME/.bashrc"
# Demoshell strips this prefix from typed commands and reads OSC 133 exit
# markers to verify command success. Bash needs its own marker after each command.
PS1='arm64js:\[\e[32m\]\W\[\e[0m\] $ '
# The first prompt has no completed command. Install reporting there so Studio
# does not mistake an initial exit marker for the start app exiting.
_demoshell_first_prompt() {
    PROMPT_COMMAND='printf "\033]133;D;%s\007" "$?"'
    unset -f _demoshell_first_prompt
}
PROMPT_COMMAND=_demoshell_first_prompt
cd "$HOME/acme"
clear
echo "A git repository with two worktrees in flight. Follow the guide to the right."
