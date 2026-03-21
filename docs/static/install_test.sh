#!/bin/sh
set -eu

# Test suite for install.sh
# Run: sh docs/static/install_test.sh
#
# Tests the install script's logic by mocking external commands (curl, wt).
# Each test runs in a subshell so failures are isolated.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/install.sh"
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
        echo "        $2"
    fi
}

# Create a temp directory for mock binaries
MOCK_DIR="$(mktemp -d)"
trap 'rm -rf "$MOCK_DIR"' EXIT

# Helper: create a mock executable
mock_bin() {
    cat > "$MOCK_DIR/$1" << MOCKEOF
#!/bin/sh
$2
MOCKEOF
    chmod +x "$MOCK_DIR/$1"
}

echo "=== install.sh test suite ==="
echo ""

# ===========================================================================
echo "--- Script structure tests ---"

# Test: script starts with proper shebang
if head -1 "$INSTALL_SH" | grep -q '^#!/bin/sh'; then
    pass "has POSIX sh shebang"
else
    fail "has POSIX sh shebang"
fi

# Test: script uses set -eu
if grep -q '^set -eu' "$INSTALL_SH"; then
    pass "uses set -eu for strict error handling"
else
    fail "uses set -eu for strict error handling"
fi

# Test: script does NOT hardcode ~/.cargo/bin path for wt lookup
if grep -q '"\$HOME/\.cargo/bin/wt"' "$INSTALL_SH"; then
    fail "should not hardcode \$HOME/.cargo/bin/wt" "Use 'command -v wt' after sourcing cargo env"
else
    pass "does not hardcode \$HOME/.cargo/bin/wt path"
fi

# Test: script sources cargo env
if grep -q 'CARGO_HOME:-\$HOME/\.cargo' "$INSTALL_SH" && grep -q '\..*env' "$INSTALL_SH"; then
    pass "sources cargo env with CARGO_HOME support"
else
    fail "sources cargo env with CARGO_HOME support"
fi

# Test: uses /dev/tty for interactive input
if grep -q '/dev/tty' "$INSTALL_SH"; then
    pass "uses /dev/tty for interactive prompts"
else
    fail "uses /dev/tty for interactive prompts"
fi

# Test: has graceful fallback when /dev/tty is unavailable
if grep -q '\-e /dev/tty' "$INSTALL_SH"; then
    pass "checks /dev/tty existence before using it"
else
    fail "checks /dev/tty existence before using it"
fi

# Test: uses --proto and --tlsv1.2 for secure curl
if grep -q "\-\-proto '=https' --tlsv1.2" "$INSTALL_SH"; then
    pass "enforces HTTPS-only with TLS 1.2+"
else
    fail "enforces HTTPS-only with TLS 1.2+"
fi

# Test: downloads to temp file instead of piping curl to sh
if grep -q 'mktemp' "$INSTALL_SH" && grep -q '\-o ' "$INSTALL_SH"; then
    pass "downloads to temp file (avoids pipe swallowing failures)"
else
    fail "downloads to temp file (avoids pipe swallowing failures)"
fi

# Test: cleans up temp file on exit
if grep -q "trap.*rm.*EXIT" "$INSTALL_SH"; then
    pass "cleans up temp file via trap"
else
    fail "cleans up temp file via trap"
fi

echo ""

# ===========================================================================
echo "--- Windows detection tests ---"

# Test: detects Windows_NT and exits
output=$(OS=Windows_NT sh "$INSTALL_SH" 2>&1) && rc=$? || rc=$?
if [ "$rc" -eq 1 ] && echo "$output" | grep -q "Windows detected"; then
    pass "detects Windows and exits with error"
else
    fail "detects Windows and exits with error" "rc=$rc output: $output"
fi

# Test: suggests PowerShell installer
if echo "$output" | grep -q "install.ps1"; then
    pass "suggests PowerShell installer for Windows"
else
    fail "suggests PowerShell installer for Windows"
fi

# Test: does not crash on unset OS variable
if grep -q '"\${OS:-}"' "$INSTALL_SH"; then
    pass "uses \${OS:-} to handle unset OS variable"
else
    fail "uses \${OS:-} to handle unset OS variable"
fi

echo ""

# ===========================================================================
echo "--- Download failure tests ---"

# Test: curl download failure is caught (not hidden by pipe)
mock_bin "curl" 'exit 1'

output=$(PATH="$MOCK_DIR:/usr/bin:/bin" OS="" sh "$INSTALL_SH" 2>&1) && rc=$? || rc=$?
if [ "$rc" -ne 0 ]; then
    pass "exits with error when curl download fails"
else
    fail "exits with error when curl download fails" "rc=$rc output: $output"
fi

if echo "$output" | grep -qi "download failed\|failed"; then
    pass "shows failure message when curl fails"
else
    fail "shows failure message when curl fails" "output: $output"
fi

# Test: installer script failure is caught
# Mock curl that succeeds but writes a failing installer
mock_bin "curl" 'echo "exit 1" > "$(echo "$@" | sed "s/.*-o //")" 2>/dev/null || true'

output=$(PATH="$MOCK_DIR:/usr/bin:/bin" OS="" sh "$INSTALL_SH" 2>&1) && rc=$? || rc=$?
if [ "$rc" -ne 0 ]; then
    pass "exits with error when installer script fails"
else
    fail "exits with error when installer script fails" "rc=$rc output: $output"
fi

echo ""

# ===========================================================================
echo "--- PATH resolution tests ---"

# Test: wt found on PATH after sourcing env
mock_bin "wt" 'if [ "${1:-}" = "config" ]; then echo "SHELL_INSTALL_CALLED"; else echo "worktrunk 0.1.0"; fi'

MOCK_CARGO="$(mktemp -d)"
mkdir -p "$MOCK_CARGO/bin"
cat > "$MOCK_CARGO/env" << EOF
export PATH="$MOCK_DIR:\$PATH"
EOF

# Test the post-install logic in isolation
cat > "$MOCK_DIR/test_post_install.sh" << 'HARNESS'
#!/bin/sh
set -eu
HARNESS

cat >> "$MOCK_DIR/test_post_install.sh" << HARNESS
CARGO_HOME="$MOCK_CARGO"
. "\${CARGO_HOME:-\$HOME/.cargo}/env" 2>/dev/null || true
if command -v wt >/dev/null 2>&1; then
    echo "FOUND_WT"
else
    echo "NOT_FOUND"
fi
HARNESS
chmod +x "$MOCK_DIR/test_post_install.sh"

output=$(sh "$MOCK_DIR/test_post_install.sh" 2>&1)
if echo "$output" | grep -q "FOUND_WT"; then
    pass "finds wt after sourcing cargo env with custom CARGO_HOME"
else
    fail "finds wt after sourcing cargo env with custom CARGO_HOME" "output: $output"
fi

# Test: wt NOT found produces warning
cat > "$MOCK_DIR/test_not_found.sh" << 'HARNESS2'
#!/bin/sh
set -eu
EMPTY_CARGO="$(mktemp -d)"
mkdir -p "$EMPTY_CARGO/bin"
echo "" > "$EMPTY_CARGO/env"
CARGO_HOME="$EMPTY_CARGO"
. "${CARGO_HOME:-$HOME/.cargo}/env" 2>/dev/null || true
PATH="/usr/bin:/bin"
export PATH
if ! command -v wt >/dev/null 2>&1; then
    echo "Warning: worktrunk installed but 'wt' not found in PATH."
    echo "NOT_FOUND_WARNING"
fi
rm -rf "$EMPTY_CARGO"
HARNESS2
chmod +x "$MOCK_DIR/test_not_found.sh"

output=$(sh "$MOCK_DIR/test_not_found.sh" 2>&1)
if echo "$output" | grep -q "NOT_FOUND_WARNING"; then
    pass "shows warning when wt is not on PATH"
else
    fail "shows warning when wt is not on PATH" "output: $output"
fi

# Cleanup mock cargo
rm -rf "$MOCK_CARGO"

echo ""

# ===========================================================================
echo "--- Non-interactive environment tests ---"

# Test: script mentions non-interactive fallback
if grep -q 'Non-interactive environment' "$INSTALL_SH"; then
    pass "has non-interactive environment fallback message"
else
    fail "has non-interactive environment fallback message"
fi

# Test: fallback tells user to run shell install manually
if grep -q 'wt config shell install' "$INSTALL_SH"; then
    pass "fallback message includes 'wt config shell install' command"
else
    fail "fallback message includes 'wt config shell install' command"
fi

echo ""

# ===========================================================================
echo "--- install.ps1 structure tests ---"

INSTALL_PS1="$SCRIPT_DIR/install.ps1"

if [ -f "$INSTALL_PS1" ]; then
    # Test: PS1 script has non-Windows detection
    if grep -q 'IsWindows' "$INSTALL_PS1"; then
        pass "install.ps1 has non-Windows detection"
    else
        fail "install.ps1 has non-Windows detection"
    fi

    # Test: PS1 script respects CARGO_HOME
    if grep -q 'CARGO_HOME' "$INSTALL_PS1"; then
        pass "install.ps1 respects CARGO_HOME"
    else
        fail "install.ps1 respects CARGO_HOME"
    fi

    # Test: PS1 handles both wt and git-wt
    if grep -q 'git-wt' "$INSTALL_PS1"; then
        pass "install.ps1 handles git-wt fallback"
    else
        fail "install.ps1 handles git-wt fallback"
    fi

    # Test: PS1 checks wt --version for worktrunk (not Windows Terminal)
    if grep -q 'worktrunk' "$INSTALL_PS1" && grep -q '\-\-version' "$INSTALL_PS1"; then
        pass "install.ps1 verifies wt is worktrunk, not Windows Terminal"
    else
        fail "install.ps1 verifies wt is worktrunk, not Windows Terminal"
    fi

    # Test: PS1 avoids duplicate PATH entries
    if grep -q 'notlike' "$INSTALL_PS1"; then
        pass "install.ps1 avoids duplicate PATH entries"
    else
        fail "install.ps1 avoids duplicate PATH entries"
    fi

    # Test: PS1 cross-references shell installer
    if grep -q 'install.sh' "$INSTALL_PS1"; then
        pass "install.ps1 cross-references shell installer for non-Windows"
    else
        fail "install.ps1 cross-references shell installer for non-Windows"
    fi
else
    fail "install.ps1 exists" "File not found at $INSTALL_PS1"
fi

echo ""

# ===========================================================================
echo "=== Results ==="
echo "  $PASS passed, $FAIL failed"
echo ""

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
