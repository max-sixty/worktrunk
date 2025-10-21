# worktrunk shell integration for elvish

# Helper function to parse wt output and handle directives
fn _wt_exec {|@args|
    var exit-code = 0
    var output = ""

    # Capture output and handle potential non-zero exit
    try {
        set output = (e:wt $@args 2>&1 | slurp)
    } catch e {
        set exit-code = 1
        set output = $e[reason][content]
    }

    # Parse output line by line
    var lines = [(str:split "\n" $output)]
    for line $lines {
        if (str:has-prefix $line "__WORKTRUNK_CD__") {
            # Extract path and change directory
            var path = (str:trim-prefix $line "__WORKTRUNK_CD__")
            cd $path
        } else {
            # Regular output - print it
            echo $line
        }
    }

    # Return exit code (will throw exception if non-zero)
    if (!=s $exit-code 0) {
        fail "command failed with exit code "$exit-code
    }
}

# Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
fn {{ cmd_prefix }} {|@args|
    if (== (count $args) 0) {
        e:wt
        return
    }

    var subcommand = $args[0]

    if (or (eq $subcommand "switch") (eq $subcommand "remove") (eq $subcommand "merge")) {
        # Commands that need --internal for directory change support
        var rest-args = $args[1..]
        _wt_exec $subcommand --internal $@rest-args
    } else {
        # All other commands pass through directly
        e:wt $@args
    }
}
