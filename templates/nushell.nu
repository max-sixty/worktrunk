# worktrunk shell integration for nushell

# Override {{ cmd }} command with file-based directive passing.
# Creates a temp file, passes path via WORKTRUNK_DIRECTIVE_FILE, executes directives after.
# WORKTRUNK_BIN can override the binary path (for testing dev builds).
#
# Note: Nushell's `source` is parse-time only, so we can't source dynamic paths.
# Instead, we read the directive file and execute cd commands directly.
def --env --wrapped {{ cmd }} [...args: string] {
    let worktrunk_bin = if ($env.WORKTRUNK_BIN? | is-not-empty) {
        $env.WORKTRUNK_BIN
    } else {
        (which {{ cmd }} | get 0.path)
    }

    let directive_file = (mktemp)

    let result = do {
        with-env { WORKTRUNK_DIRECTIVE_FILE: $directive_file } {
            ^$worktrunk_bin ...$args
        }
    } | complete

    if ($directive_file | path exists) and (open $directive_file --raw | str trim | is-not-empty) {
        let directive = open $directive_file --raw | str trim
        # Parse directive: worktrunk emits "cd <path>" for directory changes
        if ($directive | str starts-with "cd ") {
            let target_dir = $directive | str substring 3..
            cd $target_dir
        }
    }

    rm -f $directive_file

    if $result.exit_code != 0 {
        error make { msg: $"{{ cmd }} exited with code ($result.exit_code)" }
    }
}
