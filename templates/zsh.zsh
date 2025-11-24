# worktrunk shell integration for zsh

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise resolve binary path
    # Must resolve BEFORE defining shell function, so lazy completion can call binary directly
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/{{ cmd_prefix }}
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-$(command -v {{ cmd_prefix }})}"

{{ posix_shim }}

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
        # Initialize _WORKTRUNK_CMD if not set (e.g., after shell snapshot restore)
        if [[ -z "$_WORKTRUNK_CMD" ]]; then
            _WORKTRUNK_CMD="${WORKTRUNK_BIN:-$(command -v {{ cmd_prefix }})}"
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

        # Force colors if stderr is a TTY (directive mode outputs to stderr)
        # Respects NO_COLOR and explicit CLICOLOR_FORCE
        if [[ -z "${NO_COLOR:-}" && -z "${CLICOLOR_FORCE:-}" ]]; then
            if [[ -t 2 ]]; then export CLICOLOR_FORCE=1; fi
        fi

        # Always use --internal mode for directive support
        wt_exec --internal "${args[@]}"

        # Restore original command
        local result=$?
        _WORKTRUNK_CMD="$saved_cmd"
        return $result
    }

    # Lazy completions via compdef - no fpath setup needed
    # Regenerates on first TAB in each shell session
    if (( $+functions[compdef] )); then
        _{{ cmd_prefix }}_lazy_complete() {
            # Load dynamic completion script once
            unfunction _{{ cmd_prefix }}_lazy_complete 2>/dev/null || true

            # Generate & eval the real completer from the binary
            local _script
            _script="$(COMPLETE=zsh "$_WORKTRUNK_CMD" 2>/dev/null)" || return
            eval "$_script"

            # Delegate to clap's generated function
            if (( $+functions[_clap_dynamic_completer_{{ cmd_prefix }}] )); then
                _clap_dynamic_completer_{{ cmd_prefix }} "$@"
            fi
        }

        compdef _{{ cmd_prefix }}_lazy_complete {{ cmd_prefix }}
    fi
fi
