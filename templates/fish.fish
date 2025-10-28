# worktrunk shell integration for fish

# Only initialize if {{ cmd_prefix }} is available
if type -q {{ cmd_prefix }}
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
    # Note: Uses psub for process substitution to preserve NUL bytes.
    # This is reliable for simple read-only cases but psub has known
    # limitations in complex scenarios (see fish-shell issue #1040).
    # Current usage is safe as we only read from psub output sequentially.
    function _wt_exec
        set -l exec_cmd ""
        set -l exit_code_file (mktemp)
        or begin
            echo "Failed to create temp file" >&2
            return 1
        end

        # Debug mode: set WORKTRUNK_DEBUG=1 to see what's happening
        set -l debug_mode 0
        if set -q WORKTRUNK_DEBUG
            set debug_mode 1
            echo "[DEBUG] Starting _wt_exec with args: $argv" >&2
        end

        # Use psub (process substitution) to preserve NUL bytes
        # Command substitution $(...)  strips NUL bytes, but psub preserves them
        # Redirect directly from psub output, and save exit code to temp file
        set -l chunk_count 0
        while read -z chunk
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
                # Regular output - print it (preserving newlines)
                if test $debug_mode -eq 1
                    echo "[DEBUG] Message chunk" >&2
                end
                printf '%s' $chunk
            end
        end < (begin; command $_WORKTRUNK_CMD $argv; echo $status > $exit_code_file; end | psub)

        if test $debug_mode -eq 1
            echo "[DEBUG] Finished reading, total chunks: $chunk_count" >&2
        end

        # Read exit code from temp file
        set -l exit_code (cat $exit_code_file 2>/dev/null; or echo 0)
        rm -f $exit_code_file

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
        set -l subcommand $argv[1]

        switch $subcommand
            case switch remove merge
                # Commands that need --internal for directory change support
                _wt_exec --internal $subcommand $argv[2..-1]
            case '*'
                # All other commands pass through directly
                command $_WORKTRUNK_CMD $argv
        end
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
