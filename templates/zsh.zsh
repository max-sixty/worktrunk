# worktrunk shell integration for zsh

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if command -v {{ cmd_prefix }} >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/{{ cmd_prefix }}
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-{{ cmd_prefix }}}"

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    _wt_exec() {
        local exec_cmd="" chunk exit_code
        local exit_code_file stdout_file
        exit_code_file=$(mktemp) || { echo "Failed to create temp file" >&2; return 1; }
        stdout_file=$(mktemp) || { /bin/rm -f "$exit_code_file" 2>/dev/null; echo "Failed to create temp file" >&2; return 1; }

        # Run command and capture output to file to avoid process substitution issues
        # Process substitution in zsh can hang in non-interactive mode
        # Let stderr pass through to terminal (preserves TTY for color detection)
        command "$_WORKTRUNK_CMD" "$@" > "$stdout_file"
        echo "$?" > "$exit_code_file"

        # Parse stdout for directives (NUL-delimited)
        # The || [[ -n "$chunk" ]] handles non-NUL-terminated output (e.g., error messages)
        while IFS= read -r -d '' chunk || [[ -n "$chunk" ]]; do
            if [[ "$chunk" == __WORKTRUNK_CD__* ]]; then
                # CD directive - extract path and change directory
                local path="${chunk#__WORKTRUNK_CD__}"
                \cd "$path"
            elif [[ "$chunk" == __WORKTRUNK_EXEC__* ]]; then
                # EXEC directive - extract command (may contain newlines)
                exec_cmd="${chunk#__WORKTRUNK_EXEC__}"
            else
                # Regular output - print it with newline
                printf '%s\n' "$chunk"
            fi
        done < "$stdout_file"

        # Read exit code from temp file
        exit_code=$(cat "$exit_code_file" 2>/dev/null || echo 0)

        # Cleanup temp files (use absolute path for --no-rcs compatibility)
        /bin/rm -f "$exit_code_file" "$stdout_file" 2>/dev/null || true

        # Execute command if one was specified
        # Security: Command is user-provided from -x flag; eval is intentional.
        # NUL-termination allows multi-line commands.
        # Exit code semantics: If wt fails, returns wt's exit code (command never executes).
        # If wt succeeds but command fails, returns the command's exit code.
        if [[ -n "$exec_cmd" ]]; then
            eval "$exec_cmd"
            exit_code=$?
        fi

        return $exit_code
    }

    # Override {{ cmd_prefix }} command to add --internal flag
    {{ cmd_prefix }}() {
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

        # Force colors if wrapper's stdout is a TTY (respects NO_COLOR and explicit CLICOLOR_FORCE)
        if [[ -z "${NO_COLOR:-}" && -z "${CLICOLOR_FORCE:-}" ]]; then
            if [[ -t 1 ]]; then export CLICOLOR_FORCE=1; fi
        fi

        # Always use --internal mode for directive support
        _wt_exec --internal "${args[@]}"

        # Restore original command
        local result=$?
        _WORKTRUNK_CMD="$saved_cmd"
        return $result
    }

    # Dynamic completion function for zsh
    _{{ cmd_prefix }}_complete() {
        local -a completions
        local -a words

        # Get current command line as array
        words=("${(@)words}")

        # Call wt complete with current command line
        completions=(${(f)"$(command "$_WORKTRUNK_CMD" complete "${words[@]}" 2>/dev/null)"})

        # Add completions
        compadd -a completions
    }

    # Register dynamic completion (only if compdef is available)
    # In non-interactive shells or test environments, the completion system may not be loaded
    if (( $+functions[compdef] )); then
        compdef _{{ cmd_prefix }}_complete {{ cmd_prefix }}
    fi
fi
