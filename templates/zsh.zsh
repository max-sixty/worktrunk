# worktrunk shell integration for zsh
#
# Completions require zsh's completion system (compinit). If completions don't work:
#   autoload -Uz compinit && compinit  # add before this line in .zshrc

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

    # Override {{ cmd }} command so it can change the parent shell's directory.
    # WORKTRUNK_BIN can override the binary path (for testing dev builds).
    {{ cmd }}() {
        local use_source=false
        local -a args

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

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" \
                cargo run --bin {{ cmd }} --quiet -- "${args[@]}" || exit_code=$?
        else
            WORKTRUNK_DIRECTIVE_CD_FILE="$cd_file" \
                command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}" || exit_code=$?
        fi

        # cd file holds a raw path (no shell escaping needed).
        # `builtin cd` bypasses any user `cd` alias or function (e.g. zoxide's
        # `alias cd=__zoxide_z`) that would otherwise be substituted into this
        # function body when it's defined. See #2643.
        if [[ -s "$cd_file" ]]; then
            builtin cd -- "$(<"$cd_file")"
            local cd_exit=$?
            if [[ $exit_code -eq 0 ]]; then
                exit_code=$cd_exit
            fi
        fi

        command rm -f "$cd_file"
        return "$exit_code"
    }

    # Lazy completions - generate on first TAB, then delegate to clap's completer
    _{{ cmd }}_lazy_complete() {
        # Generate completions function once (check if clap's function exists)
        if ! (( $+functions[_clap_dynamic_completer_{{ cmd_ident }}] )); then
            # Use `command` to bypass the shell function and call the binary directly.
            # Without this, `{{ cmd }}` would call the shell function which evals
            # the completion script internally but doesn't re-emit it.
            # WORKTRUNK_COMPLETE_NAME emits the registration under the name bound
            # below; clap would otherwise name everything after its own command
            # name and the call below would hit an undefined function (#3816).
            eval "$(WORKTRUNK_COMPLETE_NAME="{{ cmd }}" COMPLETE=zsh command "${WORKTRUNK_BIN:-{{ cmd }}}" 2>/dev/null)" || return
        fi
        _clap_dynamic_completer_{{ cmd_ident }} "$@"
    }

    # Register completion (silently skip if compinit hasn't run yet).
    # We don't warn here because this script runs on every shell startup - users
    # shouldn't see warnings every time they open a terminal. Instead, `wt config
    # shell install` detects missing compinit and shows a one-time advisory.
    if (( $+functions[compdef] )); then
        compdef _{{ cmd }}_lazy_complete {{ cmd }}
        # Single-column display keeps descriptions visually associated with each branch.
        # Users can override: zstyle ':completion:*:{{ cmd }}:*' list-max ''
        zstyle ':completion:*:{{ cmd }}:*' list-max 1
        # Prevent grouping branches with identical descriptions (same timestamp) on one line.
        # Without this, "release  main  -- + 12m" instead of separate lines per branch.
        zstyle ':completion:*:*:{{ cmd }}:*' list-grouped false
    fi
fi
