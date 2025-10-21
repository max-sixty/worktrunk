# worktrunk shell integration for xonsh

def _wt_exec(args):
    """Helper function to parse wt output and handle directives"""
    # Capture full output including return code
    result = !(wt @(args))

    # Parse output line by line
    if result.out:
        for line in result.out.splitlines():
            if line.startswith("__WORKTRUNK_CD__"):
                # Extract path and change directory
                path = line[17:]  # Remove prefix
                cd @(path)
            else:
                # Regular output - print it
                print(line)

    # Return the exit code
    return result.returncode

def _{{ cmd_prefix }}_wrapper(args):
    """Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge"""
    if not args:
        # No arguments, just run wt
        wt
        return

    subcommand = args[0]

    if subcommand in ["switch", "remove", "merge"]:
        # Commands that need --internal for directory change support
        rest_args = args[1:]
        return _wt_exec([subcommand, "--internal"] + rest_args)
    else:
        # All other commands pass through directly
        result = !(wt @(args))
        return result.returncode

# Register the alias
aliases['{{ cmd_prefix }}'] = _{{ cmd_prefix }}_wrapper
