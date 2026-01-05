# worktrunk shell integration for fish

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if type -q {{ cmd }}; or test -n "$WORKTRUNK_BIN"

    # Override {{ cmd }} command with file-based directive passing.
    # Creates a temp file, passes path via WORKTRUNK_DIRECTIVE_FILE, evals it after.
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    #
    # Note: We use `eval (cat ... | string collect)` instead of `source` because:
    # 1. fish's `source` doesn't propagate `exit` to the parent function
    # 2. `eval (cat ...)` without `string collect` splits on newlines, breaking multiline directives
    # With `string collect`, `exit 42` properly exits the function with code 42.
    function {{ cmd }}
        set -l use_source false
        set -l args

        for arg in $argv
            if test "$arg" = "--source"; set use_source true; else; set -a args $arg; end
        end

        test -n "$WORKTRUNK_BIN"; or set -l WORKTRUNK_BIN (type -P {{ cmd }})
        set -l directive_file (mktemp)

        # --source: use cargo run (builds from source)
        if test $use_source = true
            WORKTRUNK_DIRECTIVE_FILE=$directive_file cargo run --bin {{ cmd }} --quiet -- $args
        else
            WORKTRUNK_DIRECTIVE_FILE=$directive_file command $WORKTRUNK_BIN $args
        end
        set -l exit_code $status

        if test -s "$directive_file"
            eval (cat "$directive_file" | string collect)
            if test $exit_code -eq 0
                set exit_code $status
            end
        end

        rm -f "$directive_file"
        return $exit_code
    end

    # Completions are in ~/.config/fish/completions/wt.fish (installed by `wt config shell install`)
end
