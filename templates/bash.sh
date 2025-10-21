# worktrunk shell integration for {{ shell_name }}

# Helper function to parse wt output and handle directives
_wt_exec() {
    local output line exit_code
    output="$(command wt "$@" 2>&1)"
    exit_code=$?

    # Parse output line by line
    while IFS= read -r line; do
        if [[ "$line" == __WORKTRUNK_CD__* ]]; then
            # Extract path and change directory
            \cd "${line#__WORKTRUNK_CD__}"
        else
            # Regular output - print it
            echo "$line"
        fi
    done <<< "$output"

    return $exit_code
}

# Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
{{ cmd_prefix }}() {
    local subcommand="$1"

    case "$subcommand" in
        switch|remove|merge)
            # Commands that need --internal for directory change support
            shift
            _wt_exec "$subcommand" --internal "$@"
            ;;
        *)
            # All other commands pass through directly
            command wt "$@"
            ;;
    esac
}

# Dynamic completion function
_{{ cmd_prefix }}_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"

    # Call wt complete with current command line
    local completions=$(command wt complete "${COMP_WORDS[@]}" 2>/dev/null)
    COMPREPLY=($(compgen -W "$completions" -- "$cur"))
}

# Register dynamic completion
complete -F _{{ cmd_prefix }}_complete {{ cmd_prefix }}
