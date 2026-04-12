# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

    # Override {{ cmd }} command with split directive passing.
    # Creates two temp files: one for cd (raw path) and one for exec (shell).
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    {{ cmd }}() {
        local use_source=false
        local args=()

        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then use_source=true; else args+=("$arg"); fi
        done

        # Completion mode: call binary directly, no directive files needed.
        # This check MUST be here (not in the binary) because clap's completion
        # handler runs before argument parsing.
        if [[ -n "${COMPLETE:-}" ]]; then
            command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}"
            return
        fi

        local cd_file exec_file exit_code=0
        cd_file="$(mktemp)"
        exec_file="$(mktemp)"

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" WORKTRUNK_DIRECTIVE_EXEC_FILE="$exec_file" \
                cargo run --bin {{ cmd }} --quiet -- "${args[@]}" || exit_code=$?
        else
            WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" WORKTRUNK_DIRECTIVE_EXEC_FILE="$exec_file" \
                command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}" || exit_code=$?
        fi

        # cd file holds a raw path (no shell escaping needed)
        if [[ -s "$cd_file" ]]; then
            cd -- "$(<"$cd_file")"
            local cd_exit=$?
            if [[ $exit_code -eq 0 ]]; then
                exit_code=$cd_exit
            fi
        fi

        # exec file holds arbitrary shell (e.g. from --execute)
        if [[ -s "$exec_file" ]]; then
            source "$exec_file"
            local src_exit=$?
            if [[ $exit_code -eq 0 ]]; then
                exit_code=$src_exit
            fi
        fi

        rm -f "$cd_file" "$exec_file"
        return "$exit_code"
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
