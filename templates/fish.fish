# worktrunk shell integration for fish

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if type -q {{ cmd_prefix }}; or set -q WORKTRUNK_BIN
    # TODO: Consider time-of-use pattern like bash/zsh instead of init-time setup.
    # Fish lacks ${:-} syntax; would require verbose workaround at each call site.
    if not set -q WORKTRUNK_BIN
        set -gx WORKTRUNK_BIN (type -p {{ cmd_prefix }})
    end

    # Capture stdout (shell script), eval in parent shell. stderr streams to terminal.
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    function wt_exec
        set -l script (command $WORKTRUNK_BIN $argv | string collect)
        set -l exit_code $pipestatus[1]

        if test -n "$script"
            eval $script
            if test $exit_code -eq 0
                set exit_code $status
            end
        end

        return $exit_code
    end

    # Override {{ cmd_prefix }} command to add --internal flag
    function {{ cmd_prefix }}
        set -l use_source false
        set -l args

        for arg in $argv
            if test "$arg" = "--source"; set use_source true; else; set -a args $arg; end
        end

        # Force colors if stderr is a TTY (respects NO_COLOR/CLICOLOR_FORCE)
        if not set -q NO_COLOR; and not set -q CLICOLOR_FORCE; and isatty stderr
            set -x CLICOLOR_FORCE 1
        end

        # --source: use cargo run (builds from source)
        if test $use_source = true
            set -l script (cargo run --quiet -- --internal $args | string collect)
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
