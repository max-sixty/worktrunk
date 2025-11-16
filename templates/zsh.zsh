# worktrunk shell integration for zsh

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/{{ cmd_prefix }}
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-{{ cmd_prefix }}}"

{{ posix_shim }}

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
        # Initialize _WORKTRUNK_CMD if not set (e.g., after shell snapshot restore)
        if [[ -z "$_WORKTRUNK_CMD" ]]; then
            _WORKTRUNK_CMD="${WORKTRUNK_BIN:-{{ cmd_prefix }}}"
        fi

        local use_source=false
        local -a args
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
        wt_exec --internal "${args[@]}"

        # Restore original command
        local result=$?
        _WORKTRUNK_CMD="$saved_cmd"
        return $result
    }

    # Register Clap-based completions (auto-updates after wt upgrades)
    if (( $+functions[compdef] )); then
        local completion_script
        completion_script=$(COMPLETE=zsh "$_WORKTRUNK_CMD" 2>/dev/null)
        eval "$completion_script"
    fi
fi
