# worktrunk shell integration for elvish

# Only initialize if wt is available
if (has-external wt) {
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: set E:WORKTRUNK_BIN = ./target/debug/wt
    var _WORKTRUNK_CMD = wt
    if (has-env WORKTRUNK_BIN) {
        set _WORKTRUNK_CMD = $E:WORKTRUNK_BIN
    }

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    fn _wt_exec {|@args|
        var exit-code = 0
        var output = ""
        var exec-cmd = ""

        # Capture stdout for directives, let stderr pass through to terminal
        # This preserves TTY for color detection
        # TODO: Capture actual exit code from wt command, not just success/failure
        try {
            set output = (e:$_WORKTRUNK_CMD $@args | slurp)
        } catch e {
            set exit-code = 1
            set output = $e[reason][content]
        }

        # Split output on NUL bytes, process each chunk
        var chunks = [(str:split "\x00" $output)]
        for chunk $chunks {
            if (str:has-prefix $chunk "__WORKTRUNK_CD__") {
                # CD directive - extract path and change directory
                var path = (str:trim-prefix $chunk "__WORKTRUNK_CD__")
                cd $path
            } elif (str:has-prefix $chunk "__WORKTRUNK_EXEC__") {
                # EXEC directive - extract command (may contain newlines)
                set exec-cmd = (str:trim-prefix $chunk "__WORKTRUNK_EXEC__")
            } elif (!=s $chunk "") {
                # Regular output - print it (preserving newlines)
                print $chunk
            }
        }

        # Execute command if one was specified
        if (!=s $exec-cmd "") {
            eval $exec-cmd
        }

        # Return exit code (will throw exception if non-zero)
        if (!=s $exit-code 0) {
            fail "command failed with exit code "$exit-code
        }
    }

    # Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
    fn {{ cmd_prefix }} {|@args|
        if (== (count $args) 0) {
            e:$_WORKTRUNK_CMD
            return
        }

        var subcommand = $args[0]

        if (or (eq $subcommand "switch") (eq $subcommand "remove") (eq $subcommand "merge")) {
            # Commands that need --internal for directory change support
            var rest-args = $args[1..]
            _wt_exec --internal $subcommand $@rest-args
        } else {
            # All other commands pass through directly
            e:$_WORKTRUNK_CMD $@args
        }
    }
}
