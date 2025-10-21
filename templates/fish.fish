# worktrunk shell integration for fish

# Helper function to parse wt output and handle directives
function _wt_exec
    set -l output (command wt $argv 2>&1)
    set -l exit_code $status

    # Parse output line by line
    for line in (string split \n -- $output)
        if string match -q '__WORKTRUNK_CD__*' -- $line
            # Extract path and change directory
            cd (string replace '__WORKTRUNK_CD__' '' -- $line)
        else
            # Regular output - print it
            echo $line
        end
    end

    return $exit_code
end

# Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
function {{ cmd_prefix }}
    set -l subcommand $argv[1]

    switch $subcommand
        case switch remove merge
            # Commands that need --internal for directory change support
            _wt_exec $subcommand --internal $argv[2..-1]
        case '*'
            # All other commands pass through directly
            command wt $argv
    end
end

# Dynamic completion function
function __{{ cmd_prefix }}_complete
    # Call wt complete with current command line
    set -l cmd (commandline -opc)
    command wt complete $cmd 2>/dev/null
end

# Register dynamic completions
complete -c {{ cmd_prefix }} -n '__fish_seen_subcommand_from switch' -f -a '(__{{ cmd_prefix }}_complete)' -d 'Branch'
complete -c {{ cmd_prefix }} -n '__fish_seen_subcommand_from push' -f -a '(__{{ cmd_prefix }}_complete)' -d 'Target branch'
complete -c {{ cmd_prefix }} -n '__fish_seen_subcommand_from merge' -f -a '(__{{ cmd_prefix }}_complete)' -d 'Target branch'
