# worktrunk shell integration for zsh
#
# Completions require zsh's completion system (compinit). If completions don't work:
#   autoload -Uz compinit && compinit  # add before this line in your .zshrc

# Only initialize if {{ cmd }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then

{{ posix_shim }}

    # Override {{ cmd }} command
    {{ cmd }}() {
        local use_source=false
        local -a args

        for arg in "$@"; do
            if [[ "$arg" == "--source" ]]; then use_source=true; else args+=("$arg"); fi
        done

        # Completion mode: call binary directly, bypassing wt_exec.
        # This check MUST be here (not in the binary) because clap's completion
        # handler runs before argument parsing.
        # The binary outputs completion candidates, not directives to source.
        if [[ -n "${COMPLETE:-}" ]]; then
            command "${WORKTRUNK_BIN:-{{ cmd }}}" "${args[@]}"
            return
        fi

        # --source: use cargo run (builds from source)
        if [[ "$use_source" == true ]]; then
            local directive_file exit_code=0
            directive_file="$(mktemp)"
            WORKTRUNK_DIRECTIVE_FILE="$directive_file" cargo run --bin {{ cmd }} --quiet -- "${args[@]}" || exit_code=$?
            if [[ -s "$directive_file" ]]; then
                source "$directive_file"
                if [[ $exit_code -eq 0 ]]; then
                    exit_code=$?
                fi
            fi
            rm -f "$directive_file"
            return "$exit_code"
        fi

        _{{ cmd|safe_fn }}_exec "${args[@]}"
    }

    # Lazy completions - generate on first TAB, then delegate to clap's completer
    _{{ cmd }}_lazy_complete() {
        # Generate completions function once (check if clap's function exists)
        if ! (( $+functions[_clap_dynamic_completer_{{ cmd }}] )); then
            # Use `command` to bypass the shell function and call the binary directly.
            # Without this, `{{ cmd }}` would call the shell function which evals
            # the completion script internally but doesn't re-emit it.
            #
            # The -V flag creates an unsorted group, preserving our recency-based
            # ordering instead of zsh's default alphabetical sort.
            # Note: _describe's -V does NOT take an argument - it just sets a flag.
            # The _describe function internally passes -o nosort to compadd.
            # TODO(clap): Ideally clap_complete would preserve ordering natively.
            # See: https://github.com/clap-rs/clap/issues/5752
            eval "$(COMPLETE=zsh command "${WORKTRUNK_BIN:-{{ cmd }}}" 2>/dev/null | sed "s/_describe 'values'/_describe -V 'values'/")" || return
        fi
        _clap_dynamic_completer_{{ cmd }} "$@"
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
