# Shared directive parser for POSIX shells (bash, zsh, oil).
# Streams worktrunk's NUL-delimited output in real-time via FIFO while keeping
# stderr attached to the TTY for child processes (colors, progress bars).
#
# Note: Named without leading underscore to avoid being filtered by shell
# snapshot systems (e.g., Claude Code) that exclude private functions.
# Note: Uses ${_WORKTRUNK_CMD:-{{ cmd_prefix }}} fallback because shell snapshot
# systems may capture functions but not environment variables.
wt_exec() {
    # Disable job control notifications in zsh (prevents "[1] 12345" / "[1] + done" messages)
    # For bash, this is handled at the backgrounding site using fd redirection
    if [[ -n "${ZSH_VERSION:-}" ]]; then
        setopt LOCAL_OPTIONS NO_MONITOR
    fi

    local exec_cmd="" chunk="" exit_code=0 tmp_dir="" fifo_path="" runner_pid=""

    # Cleanup handler for signals and normal exit
    _wt_cleanup() {
        # Kill background process if still running
        if [[ -n "$runner_pid" ]] && kill -0 "$runner_pid" 2>/dev/null; then
            kill "$runner_pid" 2>/dev/null || true
        fi
        # Remove temp files
        /bin/rm -f "$fifo_path" 2>/dev/null || true
        /bin/rmdir "$tmp_dir" 2>/dev/null || true
    }

    # On SIGINT: cleanup and exit immediately with 130
    trap '_wt_cleanup; return 130' INT

    # Create temp directory with FIFO for streaming output
    tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/wt.XXXXXX") || {
        echo "Failed to create temp directory for worktrunk shim" >&2
        return 1
    }
    fifo_path="$tmp_dir/stdout.fifo"

    if ! mkfifo "$fifo_path"; then
        echo "Failed to create FIFO for worktrunk shim" >&2
        /bin/rm -rf "$tmp_dir"
        return 1
    fi

    # Run worktrunk in background, piping stdout to FIFO
    # (stderr stays attached to TTY for child process colors/progress)
    # For bash, redirect stderr for the { & } to suppress job notifications,
    # but keep command's stderr on the original fd
    if [[ -n "${BASH_VERSION:-}" ]]; then
        exec 9>&2
        { command "${_WORKTRUNK_CMD:-{{ cmd_prefix }}}" "$@" >"$fifo_path" 2>&9 & } 2>/dev/null
        runner_pid=$!
        exec 9>&-
    else
        command "${_WORKTRUNK_CMD:-{{ cmd_prefix }}}" "$@" >"$fifo_path" &
        runner_pid=$!
    fi

    # Parse directives as they stream in
    while IFS= read -r -d '' chunk || [[ -n "$chunk" ]]; do
        if [[ "$chunk" == __WORKTRUNK_CD__* ]]; then
            # Directory change directive
            local path="${chunk#__WORKTRUNK_CD__}"
            \cd "$path"
        elif [[ "$chunk" == __WORKTRUNK_EXEC__* ]]; then
            # Command execution directive (deferred until after worktrunk exits)
            exec_cmd="${chunk#__WORKTRUNK_EXEC__}"
        else
            # Regular output - print to stdout
            [[ -n "$chunk" ]] && printf '%s\n' "$chunk"
        fi
    done <"$fifo_path"

    # Wait for worktrunk to complete and capture its exit code
    wait "$runner_pid" >/dev/null 2>&1 || exit_code=$?

    # Cleanup
    trap - INT
    _wt_cleanup

    # Execute deferred command if specified (its exit code takes precedence)
    if [[ -n "$exec_cmd" ]]; then
        eval "$exec_cmd"
        exit_code=$?
    fi

    return "${exit_code:-0}"
}
