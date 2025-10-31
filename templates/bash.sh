# worktrunk shell integration for {{ shell_name }}

# Only initialize if wt is available (in PATH or via WORKTRUNK_BIN)
if command -v wt >/dev/null 2>&1 || [[ -n "${WORKTRUNK_BIN:-}" ]]; then
    # Use WORKTRUNK_BIN if set, otherwise default to 'wt'
    # This allows testing development builds: export WORKTRUNK_BIN=./target/debug/wt
    _WORKTRUNK_CMD="${WORKTRUNK_BIN:-wt}"

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    _wt_exec() {
        local exec_cmd="" chunk exit_code
        local exit_code_file
        exit_code_file=$(mktemp) || { echo "Failed to create temp file" >&2; return 1; }

        # Parse stdout for directives (NUL-delimited)
        # Let stderr pass through to terminal (preserves TTY for color detection)
        # Write exit code to temp file since it can't be captured from process substitution
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
        done < <(command "$_WORKTRUNK_CMD" "$@"; echo "$?" > "$exit_code_file")

        # Read exit code from temp file
        exit_code=$(cat "$exit_code_file" 2>/dev/null || echo 0)

        # Cleanup temp file
        \rm -f "$exit_code_file"

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
        local use_dev=false
        local args=()
        local saved_cmd="$_WORKTRUNK_CMD"

        # Check for --dev flag and strip it
        for arg in "$@"; do
            if [[ "$arg" == "--dev" ]]; then
                use_dev=true
            else
                args+=("$arg")
            fi
        done

        # If --dev was specified, build and use local debug binary
        if [[ "$use_dev" == true ]]; then
            if ! cargo build --quiet >/dev/null 2>&1; then
                echo "Error: cargo build failed" >&2
                _WORKTRUNK_CMD="$saved_cmd"
                return 1
            fi
            _WORKTRUNK_CMD="./target/debug/wt"
        fi

        # Dispatch based on subcommand
        if [[ ${{ '{' }}#args[@]} -gt 0 ]]; then
            local subcommand="${args[0]}"

            case "$subcommand" in
                switch|remove|merge)
                    # Commands that need --internal for directory change support
                    _wt_exec --internal "${args[@]}"
                    ;;
                *)
                    # All other commands pass through directly
                    command "$_WORKTRUNK_CMD" "${args[@]}"
                    ;;
            esac
        else
            # No arguments, just run the command
            command "$_WORKTRUNK_CMD"
        fi

        # Restore original command
        local result=$?
        _WORKTRUNK_CMD="$saved_cmd"
        return $result
    }

    # Dynamic completion function
    _{{ cmd_prefix }}_complete() {
        local cur="${COMP_WORDS[COMP_CWORD]}"

        # Call wt complete with current command line
        local completions=$(command "$_WORKTRUNK_CMD" complete "${COMP_WORDS[@]}" 2>/dev/null)
        COMPREPLY=($(compgen -W "$completions" -- "$cur"))
    }

    # Register dynamic completion
    complete -F _{{ cmd_prefix }}_complete {{ cmd_prefix }}
fi
