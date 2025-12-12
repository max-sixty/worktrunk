# Capture stdout (shell script), eval in parent shell. stderr streams to terminal.
# WORKTRUNK_BIN can override the binary path (for testing dev builds).
wt_exec() {
    local script exit_code=0
    script="$(command "${WORKTRUNK_BIN:-{{ cmd }}}" "$@")" || exit_code=$?

    if [[ -n "$script" ]]; then
        eval "$script"
        if [[ $exit_code -eq 0 ]]; then
            exit_code=$?
        fi
    fi

    return "$exit_code"
}
