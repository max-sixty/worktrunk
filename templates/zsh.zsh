# worktrunk shell integration for zsh
#
# Completions require zsh's completion system (compinit). If completions don't work:
#   autoload -Uz compinit && compinit  # add before this line in your .zshrc

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

{{ posix_shim }}

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
        local use_source=false
        local -a args

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
        if ! (( $+functions[_clap_dynamic_completer_{{ cmd_prefix }}] )); then
            # Generate completion script, then:
            # 1. Strip compdef lines (we handle registration; fails in completion context)
            # 2. Replace 'wt --' with 'command wt --' to bypass shell function
            local _script
            _script="$(COMPLETE=zsh command "${WORKTRUNK_BIN:-{{ cmd_prefix }}}" 2>/dev/null)" || return
            _script="${_script//{{ cmd_prefix }} --/command {{ cmd_prefix }} --}"
            eval "$(echo "$_script" | grep -v '^#\?compdef ')" || return
        fi
        _clap_dynamic_completer_{{ cmd_prefix }} "$@"
    }

    # Register completion (silently skip if compinit hasn't run yet or if
    # registration fails in nested contexts like wt_exec).
    # We don't warn here because this script runs on every shell startup - users
    # shouldn't see warnings every time they open a terminal. Instead, `wt config
    # shell install` detects missing compinit and shows a one-time advisory.
    if (( $+functions[compdef] )); then
        compdef _{{ cmd_prefix }}_lazy_complete {{ cmd_prefix }} 2>/dev/null || :
    fi
fi
