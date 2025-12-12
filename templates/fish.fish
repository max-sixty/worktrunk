# worktrunk shell integration for fish

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if type -q {{ cmd }}; or test -n "$WORKTRUNK_BIN"
    # Capture stdout (shell script), eval in parent shell. stderr streams to terminal.
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    #
    # We use pipeline capture (`| read`) instead of command substitution (`(...)`)
    # because fish's command substitution runs in its own pipeline where stderr
    # doesn't inherit caller redirects. With pipeline capture, stderr flows through
    # normally and respects redirects like `wt --help &>file`.
    function wt_exec
        test -n "$WORKTRUNK_BIN"; or set -l WORKTRUNK_BIN (type -P {{ cmd }})

        # Pipeline capture: stderr streams through, stdout captured via read
        # -z (null delimiter) ensures we capture all lines, not just the first
        command $WORKTRUNK_BIN $argv | string collect --allow-empty | read --local -z script
        set -l exit_code $pipestatus[1]

        if test -n "$script"
            eval $script
            if test $exit_code -eq 0
                set exit_code $status
            end
        end

        return $exit_code
    end

    # Override {{ cmd }} command to add --internal flag
    function {{ cmd }}
        set -l use_source false
        set -l args

        for arg in $argv
            if test "$arg" = "--source"; set use_source true; else; set -a args $arg; end
        end

        # --source: use cargo run (builds from source)
        if test $use_source = true
            cargo run --bin {{ cmd }} --quiet -- --internal $args | string collect --allow-empty | read --local -z script
            set -l exit_code $pipestatus[1]
            if test -n "$script"
                eval $script
                if test $exit_code -eq 0
                    set exit_code $status
                end
            end
            return $exit_code
        end

        wt_exec --internal $args
    end

    # Completions are in ~/.config/fish/completions/wt.fish (installed by `wt config shell install`)
end
