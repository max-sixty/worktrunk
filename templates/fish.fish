# worktrunk shell integration for fish

# Only initialize if {{ cmd_prefix }} is available (in PATH or via WORKTRUNK_BIN)
if type -q {{ cmd_prefix }}; or set -q WORKTRUNK_BIN
    # Use WORKTRUNK_BIN if set, otherwise default to '{{ cmd_prefix }}'
    # This allows testing development builds: set -x WORKTRUNK_BIN ./target/debug/wt
    if set -q WORKTRUNK_BIN
        set -g _WORKTRUNK_CMD $WORKTRUNK_BIN
    else
        set -g _WORKTRUNK_CMD {{ cmd_prefix }}
    end

    # Helper function to parse wt output and handle directives
    # Directives are NUL-terminated to support multi-line commands
    #
    # IMPORTANT: No "begin...end" block on the left side of the pipe!
    # Fish runs only one piece of fish script at a time; a block on the LHS
    # would serialize the pipeline (RHS wouldn't run until LHS finishes).
    # By keeping the LHS as a plain external command, the RHS can consume
    # the stream concurrently, preserving temporal ordering.
    function _wt_exec
        set -l exec_cmd ""

        # Debug mode: set WORKTRUNK_DEBUG=1 to see what's happening
        set -l debug_mode 0
        if set -q WORKTRUNK_DEBUG
            set debug_mode 1
            echo "[DEBUG] Starting _wt_exec with args: $argv" >&2
        end

        # CRITICAL: Do NOT add "2>&1" here!
        # We want child stderr to remain a TTY (via Stdio::inherit() in Rust)
        # so that child processes stream output in real-time with colors/progress bars.
        # Only worktrunk's stdout (directives + messages) flows through this pipe.
        set -l chunk_count 0
        command $_WORKTRUNK_CMD $argv | while read -z chunk
            set chunk_count (math $chunk_count + 1)
            if test $debug_mode -eq 1
                echo "[DEBUG] Chunk $chunk_count length: "(string length -- $chunk) >&2
                echo "[DEBUG] Chunk $chunk_count first 50 chars: "(string sub -l 50 -- $chunk) >&2
            end

            if string match -q '__WORKTRUNK_CD__*' -- $chunk
                # CD directive - extract path and change directory
                set -l path (string replace '__WORKTRUNK_CD__' '' -- $chunk)
                if test $debug_mode -eq 1
                    echo "[DEBUG] CD directive: $path" >&2
                end
                if not cd $path
                    echo "Error: Failed to change directory to $path" >&2
                end
            else if string match -q '__WORKTRUNK_EXEC__*' -- $chunk
                # EXEC directive - extract command (may contain newlines)
                set exec_cmd (string replace '__WORKTRUNK_EXEC__' '' -- $chunk)
                if test $debug_mode -eq 1
                    echo "[DEBUG] EXEC directive: $exec_cmd" >&2
                end
            else if test -n "$chunk"
                # Regular output - print it with newline
                if test $debug_mode -eq 1
                    echo "[DEBUG] Message chunk" >&2
                end
                printf '%s\n' $chunk
            end
        end

        # CRITICAL: Capture $pipestatus IMMEDIATELY after the pipe, before ANY other commands!
        # Every command (including 'if', 'test', 'echo') clobbers $pipestatus
        set -l codes $pipestatus
        set -l exit_code $codes[1]

        if test $debug_mode -eq 1
            echo "[DEBUG] Finished reading, total chunks: $chunk_count" >&2
            echo "[DEBUG] Pipeline status codes: $codes" >&2
            echo "[DEBUG] Exit code: $exit_code" >&2
        end

        # Execute command if one was specified
        # Exit code semantics: Returns wt's exit code, not the executed command's.
        # This allows detecting wt failures (e.g., branch creation errors).
        # The executed command runs for side effects; its failure is logged but doesn't affect exit code.
        if test -n "$exec_cmd"
            if not eval $exec_cmd
                echo "Warning: Command execution failed (exit code $status)" >&2
            end
        end

        return $exit_code
    end

    # Override {{ cmd_prefix }} command to add --internal flag for switch, remove, and merge
    function {{ cmd_prefix }}
        set -l use_dev false
        set -l args
        set -l saved_cmd $_WORKTRUNK_CMD

        # Check for --dev flag and strip it
        for arg in $argv
            if test "$arg" = "--dev"
                set use_dev true
            else
                set -a args $arg
            end
        end

        # If --dev was specified, build and use local debug binary
        if test $use_dev = true
            if not cargo build --quiet >/dev/null 2>&1
                echo "Error: cargo build failed" >&2
                set _WORKTRUNK_CMD $saved_cmd
                return 1
            end
            set _WORKTRUNK_CMD ./target/debug/wt
        end

        # Dispatch based on subcommand
        if test (count $args) -gt 0
            set -l subcommand $args[1]

            switch $subcommand
                case switch remove merge
                    # Commands that need --internal for directory change support
                    _wt_exec --internal $args
                case '*'
                    # All other commands pass through directly
                    command $_WORKTRUNK_CMD $args
            end
        else
            # No arguments, just run the command
            command $_WORKTRUNK_CMD
        end

        # Restore original command
        set -l result $status
        set _WORKTRUNK_CMD $saved_cmd
        return $result
    end

    # Dynamic completion function
    function __{{ cmd_prefix }}_complete
        # Call {{ cmd_prefix }} complete with current command line
        set -l cmd (commandline -opc)
        command $_WORKTRUNK_CMD complete $cmd 2>/dev/null
    end

    # Register dynamic completions
{{ dynamic_completions }}
end
