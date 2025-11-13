# Shared directive parser for POSIX shells (bash, zsh, oil).
# Reads worktrunk's NUL-delimited output in real-time via a FIFO so progress
# and hint messages stream immediately while keeping stderr attached to the TTY.
# Note: Named without leading underscore to avoid being filtered by shell
# snapshot systems (e.g., Claude Code) that exclude private functions.
# Note: Uses ${_WORKTRUNK_CMD:-{{ cmd_prefix }}} fallback because shell snapshot
# systems may capture functions but not environment variables.
wt_exec() {
    local exec_cmd="" chunk="" exit_code=0 tmp_dir="" exit_file="" fifo_path="" runner_pid=""

    tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/wt.XXXXXX") || {
        echo "Failed to create temp directory for worktrunk shim" >&2
        return 1
    }

    exit_file="$tmp_dir/exit-code"
    fifo_path="$tmp_dir/stdout.fifo"

    if ! mkfifo "$fifo_path"; then
        echo "Failed to create FIFO for worktrunk shim" >&2
        /bin/rm -rf "$tmp_dir"
        return 1
    fi

    # Run command in background to enable real-time streaming via FIFO.
    (
        command "${_WORKTRUNK_CMD:-{{ cmd_prefix }}}" "$@"
        printf '%s' "$?" >"$exit_file"
    ) >"$fifo_path" &
    runner_pid=$!
    # Remove job from shell's job table to prevent job control notifications
    disown "$runner_pid" 2>/dev/null || true

    # Read directives as they arrive; keep looping even if the final chunk isn't NUL-terminated.
    while IFS= read -r -d '' chunk || [[ -n "$chunk" ]]; do
        if [[ "$chunk" == __WORKTRUNK_CD__* ]]; then
            local path="${chunk#__WORKTRUNK_CD__}"
            \cd "$path"
        elif [[ "$chunk" == __WORKTRUNK_EXEC__* ]]; then
            exec_cmd="${chunk#__WORKTRUNK_EXEC__}"
        else
            # Regular output - print it with newline (ignore empty chunks)
            [[ -n "$chunk" ]] && printf '%s\n' "$chunk"
        fi
    done <"$fifo_path"

    # Ensure the background runner has exited before reading artifacts.
    if [[ -n "$runner_pid" ]]; then
        wait "$runner_pid" >/dev/null 2>&1 || true
    fi

    if [[ -f "$exit_file" ]]; then
        read -r exit_code < "$exit_file"
    fi

    /bin/rm -f "$exit_file" "$fifo_path"
    /bin/rmdir "$tmp_dir" 2>/dev/null || true

    if [[ -n "$exec_cmd" ]]; then
        eval "$exec_cmd"
        exit_code=$?
    fi

    return "$exit_code"
}
