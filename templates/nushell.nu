# worktrunk shell integration for nushell

# Only initialize if wt is available
if (which wt | is-not-empty) {
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: $env.WORKTRUNK_BIN = ./target/debug/wt
    let _WORKTRUNK_CMD = (if ($env.WORKTRUNK_BIN? | is-not-empty) { $env.WORKTRUNK_BIN } else { "wt" })

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    export def --env _wt_exec [...args] {
        let result = (do { ^$_WORKTRUNK_CMD ...$args } | complete)
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
                # Regular output - print it (preserving newlines)
                print -n $chunk
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
        let subcommand = ($rest | get 0? | default "")

        match $subcommand {
            "switch" | "remove" | "merge" => {
                # Commands that need --internal for directory change support
                let rest_args = ($rest | skip 1)
                let internal_args = (["--internal", $subcommand] | append $rest_args)
                let exit_code = (_wt_exec ...$internal_args)
                return $exit_code
            }
            _ => {
                # All other commands pass through directly
                ^$_WORKTRUNK_CMD ...$rest
            }
        }
    }
}
