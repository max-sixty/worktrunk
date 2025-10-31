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
                # TODO: Use str replace instead of hard-coded offset (fragile if prefix changes)
                let path = ($chunk | str substring 16..)
                cd $path
            } else if ($chunk | str starts-with "__WORKTRUNK_EXEC__") {
                # EXEC directive - extract command (may contain newlines)
                # TODO: Use str replace instead of hard-coded offset (fragile if prefix changes)
                $exec_cmd = ($chunk | str substring 18..)
            } else if ($chunk | str length) > 0 {
                # Regular output - print it with newline
                print $chunk
            }
        }

        # Execute command if one was specified
        if ($exec_cmd != "") {
            nu -c $exec_cmd
        }

        # Return the exit code
        return $result.exit_code
    }

    # Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
    # Use --wrapped to pass through all flags without parsing them
    export def --env --wrapped {{ cmd_prefix }} [...rest] {
        mut use_dev = false
        mut filtered_args = []

        # Check for --dev flag and strip it
        for arg in $rest {
            if $arg == "--dev" {
                $use_dev = true
            } else {
                $filtered_args = ($filtered_args | append $arg)
            }
        }

        # Determine which command to use
        let cmd = if $use_dev {
            let build_result = (do { cargo build --quiet } | complete)
            if $build_result.exit_code != 0 {
                print "Error: cargo build failed"
                return 1
            }
            "./target/debug/wt"
        } else {
            $_WORKTRUNK_CMD
        }

        let subcommand = ($filtered_args | get 0? | default "")

        match $subcommand {
            "switch" | "remove" | "merge" => {
                # Commands that need --internal for directory change support
                let rest_args = ($filtered_args | skip 1)
                let internal_args = (["--internal", $subcommand] | append $rest_args)
                let exit_code = (_wt_exec $cmd ...$internal_args)
                return $exit_code
            }
            _ => {
                # All other commands pass through directly
                ^$cmd ...$filtered_args
            }
        }
    }
}
