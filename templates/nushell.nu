# worktrunk shell integration for nushell

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if (which {{ cmd_prefix }} | is-not-empty) or ($env.WORKTRUNK_BIN? | is-not-empty) {
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: $env.WORKTRUNK_BIN = ./target/debug/{{ cmd_prefix }}
    let _WORKTRUNK_CMD = (if ($env.WORKTRUNK_BIN? | is-not-empty) { $env.WORKTRUNK_BIN } else { "{{ cmd_prefix }}" })

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    # NOTE: Uses 'complete' which buffers output. Nushell's streaming model doesn't
    # provide primitives for byte-level reading of process output, so real-time
    # streaming is not currently possible. Exit codes are captured correctly.
    export def --env wt_exec [cmd?: string, ...args] {
        let command = (if ($cmd | is-empty) { $_WORKTRUNK_CMD } else { $cmd })

        # Run command and capture result
        # stderr passes through to terminal for TTY detection
        let result = (do { ^$command ...$args } | complete)
        mut exec_cmd = ""

        # Split output on NUL bytes, process each chunk
        for chunk in ($result.stdout | split row "\u{0000}") {
            if ($chunk | str starts-with "__WORKTRUNK_CD__") {
                # CD directive - extract path and change directory
                let path = ($chunk | str replace --regex '^__WORKTRUNK_CD__' '')
                if ($path | path exists) and ($path | path type) == "dir" {
                    cd $path
                } else {
                    print $"Error: Not a directory: ($path)" | str trim
                }
            } else if ($chunk | str starts-with "__WORKTRUNK_EXEC__") {
                # EXEC directive - extract command (may contain newlines)
                $exec_cmd = ($chunk | str replace --regex '^__WORKTRUNK_EXEC__' '')
            } else if ($chunk | str length) > 0 {
                # Regular output - print it
                print $chunk
            }
        }

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if ($exec_cmd != "") {
            # Security: Command comes from wt --internal output; eval is intentional
            let cmd_result = (do { nu -c $exec_cmd } | complete)
            return $cmd_result.exit_code
        }

        # Return the actual exit code from the command
        return $result.exit_code
    }

    # Override {{ cmd_prefix }} command to add --internal flag
    # Use --wrapped to pass through all flags without parsing them
    export def --env --wrapped {{ cmd_prefix }} [...rest] {
        mut use_source = false
        mut filtered_args = []

        # Check for --source flag and strip it
        for arg in $rest {
            if $arg == "--source" {
                $use_source = true
            } else {
                $filtered_args = ($filtered_args | append $arg)
            }
        }

        # Determine which command to use
        let cmd = if $use_source {
            let build_result = (do { cargo build --quiet } | complete)
            if $build_result.exit_code != 0 {
                print "Error: cargo build failed"
                return 1
            }
            "./target/debug/{{ cmd_prefix }}"
        } else {
            $_WORKTRUNK_CMD
        }

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if ($env.NO_COLOR? | is-empty) and ($env.CLICOLOR_FORCE? | is-empty) {
            if (do -i { term size } | is-not-empty) {
                load-env { CLICOLOR_FORCE: "1" }
            }
        }

        # Always use --internal mode for directive support
        let internal_args = (["--internal"] | append $filtered_args)
        let exit_code = (wt_exec $cmd ...$internal_args)
        return $exit_code
    }
}
