# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

    # Override {{ cmd }} command with file-based directive passing.
    # Creates a temp file, passes path via WORKTRUNK_DIRECTIVE_FILE, sources it after.
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    {{ cmd }}() {
        local use_source=false
        local args=()

        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then use_source=true; else args+=("$arg"); fi
        done

        # Completion mode: call binary directly, no directive file needed.
        # This check MUST be here (not in the binary) because clap's completion
        # handler runs before argument parsing.
        if [[ -n "${COMPLETE:-}" ]]; then
            command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}"
            return
        fi

        local directive_file exit_code=0
        directive_file="$(mktemp)"

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            WORKTRUNK_DIRECTIVE_FILE="$directive_file" cargo run --bin {{ cmd }} --quiet -- "${args[@]}" || exit_code=$?
        else
            WORKTRUNK_DIRECTIVE_FILE="$directive_file" command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}" || exit_code=$?
        fi

        if [[ -s "$directive_file" ]]; then
            source "$directive_file"
            if [[ $exit_code -eq 0 ]]; then
                exit_code=$?
            fi
        fi

        rm -f "$directive_file"
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
