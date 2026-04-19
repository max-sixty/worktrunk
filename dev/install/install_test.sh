#!/bin/sh
set -eu

# Unit tests for docs/static/install.sh.
#
# These are fast behavioral tests that run install.sh with a mocked curl on
# PATH and verify exit codes + output. They cover the error/edge paths only.
# The happy path (curl | sh yielding a working `wt`) is covered by the
# container hand-test: dev/install/test-containers.sh

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/../../docs/static/install.sh"

PASS=0
FAIL=0

pass() {
    PASS=$((PASS + 1))
    echo "  PASS: $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo "  FAIL: $1"
    if [ -n "${2:-}" ]; then
        printf '        %s\n' "$2"
    fi
}

MOCK_DIR="$(mktemp -d)"
CARGO_DIR="$(mktemp -d)"
trap 'rm -rf "$MOCK_DIR" "$CARGO_DIR"' EXIT

# Write an executable shell script to $MOCK_DIR/$1 with the given body.
mock_bin() {
    name="$1"
    body="$2"
    path="$MOCK_DIR/$name"
    printf '#!/bin/sh\n%s\n' "$body" > "$path"
    chmod +x "$path"
}

# Mock curl to find the `-o <path>` arg and write a minimal installer there
# that exits with the given code. Simulates a successful download of an
# installer whose *execution* then succeeds ($1=0) or fails ($1!=0).
mock_curl_writes_installer() {
    mock_bin curl "while [ \$# -gt 0 ]; do
    if [ \"\$1\" = \"-o\" ]; then
        shift
        printf '%s\n' '#!/bin/sh' 'exit $1' > \"\$1\"
        exit 0
    fi
    shift
done
exit 1"
}

# Run install.sh with $MOCK_DIR first on PATH and a curated environment.
# Captures exit code in $rc and combined output in $output. Pass a value for
# $OS as the first argument (default empty).
run_install() {
    os_val="${1:-}"
    set +e
    output="$(PATH="$MOCK_DIR:/usr/bin:/bin" \
        OS="$os_val" \
        HOME="$CARGO_DIR/home" \
        CARGO_HOME="$CARGO_DIR/cargo" \
        sh "$INSTALL_SH" 2>&1)"
    rc=$?
    set -e
}

echo "=== install.sh test suite ==="
echo ""

# ---------------------------------------------------------------------------
echo "--- Platform gate ---"

# Windows detection: sets OS=Windows_NT, expects exit 1 with guidance.
run_install Windows_NT
if [ "$rc" -eq 1 ] && echo "$output" | grep -q "Windows detected" \
    && echo "$output" | grep -q "install.ps1"; then
    pass "exits with PowerShell guidance when OS=Windows_NT"
else
    fail "exits with PowerShell guidance when OS=Windows_NT" "rc=$rc output: $output"
fi

echo ""

# ---------------------------------------------------------------------------
echo "--- Download / installer failures ---"

# curl fails to download: expect non-zero exit.
mock_bin curl 'exit 22'
run_install
if [ "$rc" -ne 0 ]; then
    pass "exits non-zero when curl fails"
else
    fail "exits non-zero when curl fails" "rc=$rc output: $output"
fi

# curl succeeds but writes a failing installer: expect non-zero exit.
mock_curl_writes_installer 5
run_install
if [ "$rc" -ne 0 ]; then
    pass "exits non-zero when upstream installer fails"
else
    fail "exits non-zero when upstream installer fails" "rc=$rc output: $output"
fi

echo ""

# ---------------------------------------------------------------------------
echo "--- Post-install path resolution ---"

# curl writes a no-op installer; CARGO_HOME is empty and wt is absent from
# PATH → script should warn and exit 0 (installer succeeded but wt missing).
mock_curl_writes_installer 0
mkdir -p "$CARGO_DIR/cargo"
run_install
if [ "$rc" -eq 0 ] && echo "$output" | grep -q "'wt' not found in PATH"; then
    pass "warns and exits 0 when wt is missing after install"
else
    fail "warns and exits 0 when wt is missing after install" "rc=$rc output: $output"
fi

# As above, but wt exists in CARGO_HOME/bin with an env file. The script
# should source env, find wt, and reach the post-wt-found stage. Whether the
# final `wt config shell install` actually runs depends on /dev/tty being
# openable — that's a property of the test environment, so accept either the
# sentinel (TTY) or the non-interactive fallback message (no TTY).
sentinel="$CARGO_DIR/wt.args"
mkdir -p "$CARGO_DIR/cargo/bin"
cat > "$CARGO_DIR/cargo/bin/wt" <<WT
#!/bin/sh
printf '%s\n' "\$*" > "$sentinel"
WT
chmod +x "$CARGO_DIR/cargo/bin/wt"
cat > "$CARGO_DIR/cargo/env" <<ENV
export PATH="$CARGO_DIR/cargo/bin:\$PATH"
ENV
run_install
reached_post_wt=false
if [ -f "$sentinel" ] && grep -q "config shell install" "$sentinel"; then
    reached_post_wt=true
elif echo "$output" | grep -q "Non-interactive environment"; then
    reached_post_wt=true
fi
if [ "$rc" -eq 0 ] && [ "$reached_post_wt" = true ]; then
    pass "finds wt on PATH after install and reaches shell-install stage"
else
    fail "finds wt on PATH after install and reaches shell-install stage" \
        "rc=$rc sentinel=$(cat "$sentinel" 2>/dev/null) output: $output"
fi

echo ""

# ---------------------------------------------------------------------------
echo "=== Results ==="
echo "  $PASS passed, $FAIL failed"
echo ""

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
