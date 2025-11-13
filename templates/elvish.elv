# worktrunk shell integration for elvish

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if (or (has-external {{ cmd_prefix }}) (has-env WORKTRUNK_BIN)) {
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: set E:WORKTRUNK_BIN = ./target/debug/{{ cmd_prefix }}
    var _WORKTRUNK_CMD = {{ cmd_prefix }}
    if (has-env WORKTRUNK_BIN) {
        set _WORKTRUNK_CMD = $E:WORKTRUNK_BIN
    }

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    # NOTE: Uses 'slurp' which buffers output. Elvish's I/O model doesn't provide
    # good primitives for byte-level streaming of process output while also capturing
    # it for directive processing. Exit codes are now captured correctly.
    fn wt_exec {|@args|
        var exit-code = 0
        var output = ""
        var exec-cmd = ""

        # Capture stdout for directives, let stderr pass through to terminal
        # This preserves TTY for color detection
        try {
            set output = (e:$_WORKTRUNK_CMD $@args | slurp)
        } catch e {
            # Extract exit code, defaulting to 1 if unavailable
            set exit-code = 1
            if (has-key $e[reason] exit-status) {
                set exit-code = $e[reason][exit-status]
            }
            # Capture any error content
            if (has-key $e[reason] content) {
                set output = $e[reason][content]
            }
        }

        # Split output on NUL bytes, process each chunk
        var chunks = [(str:split "\x00" $output)]
        for chunk $chunks {
            if (str:has-prefix $chunk "__WORKTRUNK_CD__") {
                # CD directive - extract path and change directory
                var path = (str:trim-prefix $chunk "__WORKTRUNK_CD__")
                if (path:is-dir $path) {
                    cd $path
                } else {
                    echo "Error: Not a directory: "$path >&2
                }
            } elif (str:has-prefix $chunk "__WORKTRUNK_EXEC__") {
                # EXEC directive - extract command (may contain newlines)
                set exec-cmd = (str:trim-prefix $chunk "__WORKTRUNK_EXEC__")
            } elif (!=s $chunk "") {
                # Regular output - print it (preserving newlines)
                print $chunk
            }
        }

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if (!=s $exec-cmd "") {
            try {
                # Security: Command comes from wt --internal output; eval is intentional
                eval $exec-cmd
                set exit-code = 0
            } catch e {
                # Extract exit code, defaulting to 1 if unavailable
                set exit-code = 1
                if (has-key $e[reason] exit-status) {
                    set exit-code = $e[reason][exit-status]
                }
            }
        }

        # Return exit code (will throw exception if non-zero)
        if (!=s $exit-code 0) {
            fail "command failed with exit code "$exit-code
        }
    }

    # Override {{ cmd_prefix }} command to add --internal flag
    fn {{ cmd_prefix }} {|@args|
        var use-source = $false
        var filtered-args = []
        var saved-cmd = $_WORKTRUNK_CMD

        # Check for --source flag and strip it
        for arg $args {
            if (eq $arg "--source") {
                set use-source = $true
            } else {
                set filtered-args = [$@filtered-args $arg]
            }
        }

        # If --source was specified, build and use local debug binary
        if $use-source {
            try {
                e:cargo build --quiet 2>&1 | slurp
            } catch e {
                echo "Error: cargo build failed" >&2
                fail "cargo build failed"
            }
            set _WORKTRUNK_CMD = ./target/debug/{{ cmd_prefix }}
        }

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if (and (not (has-env NO_COLOR)) (not (has-env CLICOLOR_FORCE))) {
            if (isatty stdout) {
                set E:CLICOLOR_FORCE = 1
            }
        }

        # Always use --internal mode for directive support
        wt_exec --internal $@filtered-args

        # Restore original command
        set _WORKTRUNK_CMD = $saved-cmd
    }
}
