# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

{{ posix_shim }}

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
        # Commands that need direct terminal access (bypass directive mode)
        # ui uses exec() to replace process with zellij - can't capture stdout
        if [[ "$1" == "ui" ]]; then
            command "${WORKTRUNK_BIN:-{{ cmd_prefix }}}" "$@"
            return $?
        fi

        local use_source=false
        local args=()

        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then use_source=true; else args+=("$arg"); fi
        done

        # Force colors if stderr is a TTY (respects NO_COLOR/CLICOLOR_FORCE)
        if [[ -z "${NO_COLOR:-}" && -z "${CLICOLOR_FORCE:-}" && -t 2 ]]; then
            export CLICOLOR_FORCE=1
        fi

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            local script exit_code=0
            script="$(cargo run --quiet -- --internal "${args[@]}")" || exit_code=$?
            if [[ -n "$script" ]]; then
                eval "$script"
                if [[ $exit_code -eq 0 ]]; then
                    exit_code=$?
                fi
            fi
            return "$exit_code"
        fi

        wt_exec --internal "${args[@]}"
    }

    # Lazy completions - generate on first TAB, then delegate to clap's completer
    _{{ cmd_prefix }}_lazy_complete() {
        # Generate completions function once (check if clap's function exists)
        if ! declare -F _clap_complete_{{ cmd_prefix }} >/dev/null; then
            eval "$(COMPLETE=bash "${WORKTRUNK_BIN:-{{ cmd_prefix }}}" 2>/dev/null)" || return
        fi
        _clap_complete_{{ cmd_prefix }} "$@"
    }

    complete -o nospace -o bashdefault -F _{{ cmd_prefix }}_lazy_complete {{ cmd_prefix }}
fi
