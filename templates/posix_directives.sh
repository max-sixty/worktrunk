# Shared directive parser for POSIX shells (bash, zsh, oil).
# Reads worktrunk's NUL-delimited output in real-time via a FIFO so progress
# and hint messages stream immediately while keeping stderr attached to the TTY.
_wt_exec() {
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

    (
        command "$_WORKTRUNK_CMD" "$@"
        printf '%s' "$?" >"$exit_file"
    ) >"$fifo_path" &
    runner_pid=$!

    # Read directives as they arrive; keep looping even if the final chunk isn't NUL-terminated.
    while IFS= read -r -d '' chunk || [[ -n "$chunk" ]]; do
        if [[ "$chunk" == __WORKTRUNK_CD__* ]]; then
            local path="${chunk#__WORKTRUNK_CD__}"
            \cd "$path"
        elif [[ "$chunk" == __WORKTRUNK_EXEC__* ]]; then
            exec_cmd="${chunk#__WORKTRUNK_EXEC__}"
        else
            printf '%s\n' "$chunk"
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
