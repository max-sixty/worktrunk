# Shared directive parser for POSIX shells (bash, zsh, oil).
# Streams worktrunk's NUL-delimited output in real-time via FIFO while keeping
# stderr attached to the TTY for child processes (colors, progress bars).
#
# Note: Named without leading underscore to avoid being filtered by shell
# snapshot systems (e.g., Claude Code) that exclude private functions.
# Note: Uses ${_WORKTRUNK_CMD:-{{ cmd_prefix }}} fallback because shell snapshot
# systems may capture functions but not environment variables.
wt_exec() {
    local exec_cmd="" chunk="" exit_code=0 tmp_dir="" fifo_path="" runner_pid=""

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
    command "${_WORKTRUNK_CMD:-{{ cmd_prefix }}}" "$@" >"$fifo_path" &
    runner_pid=$!

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
    /bin/rm -f "$fifo_path"
    /bin/rmdir "$tmp_dir" 2>/dev/null || true

    # Execute deferred command if specified (its exit code takes precedence)
    if [[ -n "$exec_cmd" ]]; then
        eval "$exec_cmd"
        exit_code=$?
    fi

    return "${exit_code:-0}"
}
