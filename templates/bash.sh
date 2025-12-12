# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

{{ posix_shim }}

    # Override {{ cmd }} command to add --internal flag
    {{ cmd }}() {
        local use_source=false
        local args=()

        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then use_source=true; else args+=("$arg"); fi
        done

        # Completion mode: call binary directly, bypassing --internal and wt_exec.
        # This check MUST be here (not in the binary) because clap's completion
        # handler runs before argument parsing - we can't detect --internal there.
        # The binary outputs completion candidates, not shell script to eval.
        if [[ -n "${COMPLETE:-}" ]]; then
            command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}"
            return
        fi

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            local script exit_code=0
            script="$(cargo run --bin {{ cmd }} --quiet -- --internal "${args[@]}")" || exit_code=$?
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
    _{{ cmd }}_lazy_complete() {
        # Generate completions function once (check if clap's function exists)
        if ! declare -F _clap_complete_{{ cmd }} >/dev/null; then
            # Use `command` to bypass the shell function and call the binary directly.
            # Without this, `{{ cmd }}` would call the shell function which evals
            # the completion script internally but doesn't re-emit it.
            eval "$(COMPLETE=bash command "${WORKTRUNK_BIN:-{{ cmd }}}" 2>/dev/null)" || return
        fi
        _clap_complete_{{ cmd }} "$@"
    }

    complete -o nospace -o bashdefault -F _{{ cmd }}_lazy_complete {{ cmd }}
fi
