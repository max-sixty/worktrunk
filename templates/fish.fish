# worktrunk shell integration for fish
#
# This is the full function definition, output by `{{ cmd }} config shell init fish`.
# It's sourced at runtime by the wrapper in ~/.config/fish/functions/{{ cmd }}.fish.

# Override {{ cmd }} command so it can change the parent shell's directory.
# WORKTRUNK_BIN can override the binary path (for testing dev builds).
function {{ cmd }}
    set -l use_source false
    set -l args

    for arg in $argv
        if test "$arg" = "--source"; set use_source true; else; set -a args $arg; end
    end

    test -n "$WORKTRUNK_BIN"; or set -l WORKTRUNK_BIN (type -P {{ cmd }} 2>/dev/null)
    if test -z "$WORKTRUNK_BIN"
        echo "{{ cmd }}: command not found" >&2
        return 127
    end
    set -l cd_file (mktemp)

    # Fish cancels an interactive function when its child dies from SIGINT.
    # Its post-command event still runs before the next prompt, so it owns
    # directive application and cleanup on both ordinary and interrupted exits.
    set -l cleanup_function _{{ cmd }}_cleanup_(string replace -a / _ -- "$cd_file")
    function $cleanup_function --on-event fish_postexec --inherit-variable cd_file --inherit-variable cleanup_function
        set -l cd_exit 0
        # cd file holds a raw path — read with fish builtin (no cat subprocess,
        # safe even if CWD was removed by worktree removal).
        if test -s "$cd_file"
            set -l target (string trim < "$cd_file")
            # `builtin cd` bypasses any user `cd` override (e.g. the zoxide.fish
            # plugin replaces `cd` with a query function that mishandles the `--`
            # separator). Matches the bash/zsh wrappers. (#3159)
            builtin cd -- "$target"
            set cd_exit $status
        end

        command rm -f "$cd_file"
        functions -e $cleanup_function
        return $cd_exit
    end

    # --source: use cargo run (builds from source)
    if test $use_source = true
        env WORKTRUNK_DIRECTIVE_CD_FILE=$cd_file \
            cargo run --bin {{ cmd }} --quiet -- $args
    else
        env WORKTRUNK_DIRECTIVE_CD_FILE=$cd_file \
            $WORKTRUNK_BIN $args
    end
    set -l exit_code $status

    $cleanup_function
    set -l cd_exit $status
    if test $exit_code -eq 0
        set exit_code $cd_exit
    end
    return $exit_code
end

# Completions are in ~/.config/fish/completions/{{ cmd }}.fish (installed by `{{ cmd }} config shell install`)
