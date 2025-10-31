# worktrunk shell integration for zsh

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if command -v wt >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/wt
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-wt}"

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
        # Exit code semantics: Returns wt's exit code, not the executed command's.
        # This allows detecting wt failures (e.g., branch creation errors).
        # The executed command runs for side effects; its failure is logged but doesn't affect exit code.
        if [[ -n "$exec_cmd" ]]; then
            if ! eval "$exec_cmd"; then
                echo "Warning: Command execution failed (exit code $?)" >&2
            fi
        fi

        return $exit_code
    }

    # Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
    {{ cmd_prefix }}() {
        local subcommand="$1"

        case "$subcommand" in
            switch|remove|merge)
                # Commands that need --internal for directory change support
                shift
                _wt_exec --internal "$subcommand" "$@"
                ;;
            *)
                # All other commands pass through directly
                command "$_WORKTRUNK_CMD" "$@"
                ;;
        esac
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
