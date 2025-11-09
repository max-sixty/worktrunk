# worktrunk shell integration for nushell

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if (which wt | is-not-empty) or ($env.WORKTRUNK_BIN? | is-not-empty) {
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: $env.WORKTRUNK_BIN = ./target/debug/wt
    let _WORKTRUNK_CMD = (if ($env.WORKTRUNK_BIN? | is-not-empty) { $env.WORKTRUNK_BIN } else { "wt" })

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    export def --env _wt_exec [cmd?: string, ...args] {
        let command = (if ($cmd | is-empty) { $_WORKTRUNK_CMD } else { $cmd })
        let result = (do { ^$command ...$args } | complete)
        mut exec_cmd = ""

        # Split output on NUL bytes, process each chunk
        for chunk in ($result.stdout | split row "\u{0000}") {
            if ($chunk | str starts-with "__WORKTRUNK_CD__") {
                # CD directive - extract path and change directory
                let path = ($chunk | str replace --regex '^__WORKTRUNK_CD__' '')
                cd $path
            } else if ($chunk | str starts-with "__WORKTRUNK_EXEC__") {
                # EXEC directive - extract command (may contain newlines)
                $exec_cmd = ($chunk | str replace --regex '^__WORKTRUNK_EXEC__' '')
            } else if ($chunk | str length) > 0 {
                # Regular output - print it with newline
                print $chunk
            }
        }

        # Execute command if one was specified
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if ($exec_cmd != "") {
            let cmd_result = (do { nu -c $exec_cmd } | complete)
            return $cmd_result.exit_code
        }

        # Return the exit code
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
            "./target/debug/wt"
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
        let exit_code = (_wt_exec $cmd ...$internal_args)
        return $exit_code
    }
}
