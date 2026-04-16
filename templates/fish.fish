# worktrunk shell integration for fish
#
# This is the full function definition, output by `{{ cmd }} config shell init fish`.
# It's sourced at runtime by the wrapper in ~/.config/fish/functions/{{ cmd }}.fish.

# Override {{ cmd }} command with split directive passing.
# Creates two temp files: one for cd (raw path) and one for exec (shell).
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
    set -l exec_file (mktemp)

    # --source: use cargo run (builds from source)
    if test $use_source = true
        env WORKTRUNK_DIRECTIVE_CD_FILE=$cd_file WORKTRUNK_DIRECTIVE_EXEC_FILE=$exec_file \
            cargo run --bin {{ cmd }} --quiet -- $args
    else
        env WORKTRUNK_DIRECTIVE_CD_FILE=$cd_file WORKTRUNK_DIRECTIVE_EXEC_FILE=$exec_file \
            $WORKTRUNK_BIN $args
    end
    set -l exit_code $status

    # cd file holds a raw path — read with fish builtin (no cat subprocess,
    # safe even if CWD was removed by worktree removal).
    if test -s "$cd_file"
        set -l target (string trim < "$cd_file")
        cd -- "$target"
        set -l cd_exit $status
        if test $exit_code -eq 0
            set exit_code $cd_exit
        end
    end

    # exec file holds arbitrary shell (e.g. from --execute)
    if test -s "$exec_file"
        set -l directive (string collect < "$exec_file")
        eval $directive
        set -l src_exit $status
        if test $exit_code -eq 0
            set exit_code $src_exit
        end
    end

    command rm -f "$cd_file" "$exec_file"
    return $exit_code
end

# Completions are in ~/.config/fish/completions/{{ cmd }}.fish (installed by `{{ cmd }} config shell install`)
