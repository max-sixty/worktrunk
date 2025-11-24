# worktrunk shell integration for {{ shell_name }}

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

    # Lazy completion loader - loads real completions on first tab-press
    # This avoids ~11ms binary invocation at shell startup
    _wt_lazy_complete() {
        # Only try to install completions once
        if [[ -z "${_WT_COMPLETION_LOADED:-}" ]]; then
            _WT_COMPLETION_LOADED=1
            if completion_script=$(COMPLETE=bash "${_WORKTRUNK_CMD:-{{ cmd_prefix }}}" 2>/dev/null); then
                eval "$completion_script"
            else
                # Failed to load - remove completion registration
                complete -r {{ cmd_prefix }} 2>/dev/null || true
                return 1
            fi
        fi

        # Delegate to real completion function if it was installed
        if declare -F _clap_complete_{{ cmd_prefix }} >/dev/null 2>&1; then
            _clap_complete_{{ cmd_prefix }}
        fi
    }
    # nosort requires Bash 4.4+ (macOS ships with 3.2)
    if [[ ${BASH_VERSINFO[0]} -gt 4 || (${BASH_VERSINFO[0]} -eq 4 && ${BASH_VERSINFO[1]} -ge 4) ]]; then
        complete -o nospace -o bashdefault -o nosort -F _wt_lazy_complete {{ cmd_prefix }}
    else
        complete -o nospace -o bashdefault -F _wt_lazy_complete {{ cmd_prefix }}
    fi
fi
