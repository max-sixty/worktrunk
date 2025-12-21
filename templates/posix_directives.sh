# Execute {{ cmd }} command with file-based directive passing.
# Creates a temp file, passes path via WORKTRUNK_DIRECTIVE_FILE, sources it after.
# WORKTRUNK_BIN can override the binary path (for testing dev builds).
# Function name includes cmd to avoid conflicts when multiple commands are loaded.
_{{ cmd|safe_fn }}_exec() {
    local directive_file exit_code=0
    directive_file="$(mktemp)"

    WORKTRUNK_DIRECTIVE_FILE="$directive_file" command "${WORKTRUNK_BIN:-{{ cmd }}}" "$@" || exit_code=$?

    if [[ -s "$directive_file" ]]; then
        source "$directive_file"
        if [[ $exit_code -eq 0 ]]; then
            exit_code=$?
        fi
    fi

    rm -f "$directive_file"
    return "$exit_code"
}
