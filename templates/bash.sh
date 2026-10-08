# worktrunk shell integration for {{ shell_name }}

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

    # Override {{ cmd }} command so it can change the parent shell's directory.
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

        local cd_file exit_code=0
        cd_file="$(mktemp)"

        # The tail command lets Bash run RETURN cleanup during native SIGINT
        # unwind. The outer function returns the resulting CD/command status;
        # returning from inside a RETURN trap loses that status on Bash 3.2.
        _{{ cmd_ident }}_run() {
            local return_trap
            return_trap="$(builtin trap -p RETURN)"
            builtin trap '
                exit_code=$?
                if [[ -s "$cd_file" ]]; then
                    local cd_exit=0
                    builtin cd -- "$(<"$cd_file")" || cd_exit=$?
                    if [[ $exit_code -eq 0 ]]; then
                        exit_code=$cd_exit
                    fi
                fi
                command rm -f "$cd_file"
                builtin unset -f _{{ cmd_ident }}_run
                builtin trap - RETURN
                if [[ -n "$return_trap" ]]; then
                    builtin eval "$return_trap"
                fi
            ' RETURN
            WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" command "$@"
        }

        # Prepare both paths before the tail command: RETURN runs during
        # native interruption only when no statement follows the child.
        local -a execution=("${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}")
        if [[ "$use_source" == true ]]; then
            execution=(cargo run --bin {{ cmd }} --quiet -- "${args[@]}")
        fi
        # A failed child must reach RETURN cleanup even under `set -e`.
        _{{ cmd_ident }}_run "${execution[@]}" || :
        return "$exit_code"
    }

    # Lazy completions - generate on first TAB, then delegate to clap's completer
    _{{ cmd }}_lazy_complete() {
        # Generate completions function once (check if clap's function exists)
        if ! declare -F _clap_complete_{{ cmd_ident }} >/dev/null; then
            # Use `command` to bypass the shell function and call the binary directly.
            # Without this, `{{ cmd }}` would call the shell function which evals
            # the completion script internally but doesn't re-emit it.
            # WORKTRUNK_COMPLETE_NAME emits the registration under the name bound
            # below; clap would otherwise name everything after its own command
            # name and the call below would hit an undefined function (#3816).
            eval "$(WORKTRUNK_COMPLETE_NAME="{{ cmd }}" COMPLETE=bash command "${WORKTRUNK_BIN:-{{ cmd }}}" 2>/dev/null)" || return
        fi
        _clap_complete_{{ cmd_ident }} "$@"
    }

    complete -o nospace -o bashdefault -F _{{ cmd }}_lazy_complete {{ cmd }}
fi
