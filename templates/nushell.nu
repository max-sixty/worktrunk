# worktrunk shell integration for nushell

# Helper function to parse wt output and handle directives
export def --env _wt_exec [...args] {
    let result = (do { ^wt ...$args } | complete)

    # Parse output line by line
    let lines = ($result.stdout | lines)
    for line in $lines {
        if ($line | str starts-with "__WORKTRUNK_CD__") {
            # Extract path and change directory
            let path = ($line | str substring 16..)
            cd $path
        } else {
            # Regular output - print it
            print $line
        }
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
            let internal_args = ([$subcommand, "--internal"] | append $rest_args)
            let exit_code = (_wt_exec ...$internal_args)
            return $exit_code
        }
        _ => {
            # All other commands pass through directly
            ^wt ...$rest
        }
    }
}
