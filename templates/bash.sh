# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/{{ cmd_prefix }}
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-{{ cmd_prefix }}}"

{{ posix_shim }}

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
        local use_source=false
        local args=()
        local saved_cmd="$_WORKTRUNK_CMD"

        # Check for --source flag and strip it
        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then
                use_source=true
            else
                args+=("$arg")
            fi
        done

        # If --source was specified, build and use local debug binary
        if [[ "$use_source" == true ]]; then
            if ! cargo build --quiet; then
                _WORKTRUNK_CMD="$saved_cmd"
                return 1
            fi
            _WORKTRUNK_CMD="./target/debug/{{ cmd_prefix }}"
        fi

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if [[ -z "${NO_COLOR:-}" && -z "${CLICOLOR_FORCE:-}" ]]; then
            if [[ -t 1 ]]; then export CLICOLOR_FORCE=1; fi
        fi

        # Always use --internal mode for directive support
        _wt_exec --internal "${args[@]}"

        # Restore original command
        local result=$?
        _WORKTRUNK_CMD="$saved_cmd"
        return $result
    }

    # Dynamic completion function
    _{{ cmd_prefix }}_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}"

        # Call wt complete with current command line
        local completions=$(command "$_WORKTRUNK_CMD" complete "${COMP_WORDS[@]}" 2>/dev/null)
        COMPREPLY=($(compgen -W "$completions" -- "$cur"))
    }

    # Register dynamic completion
    complete -F _{{ cmd_prefix }}_complete {{ cmd_prefix }}
fi
