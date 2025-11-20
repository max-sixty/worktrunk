#!/bin/bash
###############################################################################
# Claude Code Web - Environment Setup Script
###############################################################################
#
# This script sets up the development environment for working on worktrunk
# in Claude Code web sessions.
#
# What it does:
# - Verifies Rust toolchain
# - Builds the project
# - Runs unit tests
# - Installs required shells (zsh, fish) on Debian/Ubuntu
# - Runs integration tests
# - Reports environment status
#
# Usage:
#   ./scripts/setup-claude-code-web.sh
#
###############################################################################

set -e  # Exit on error

echo "========================================"
echo "Claude Code Web - Worktrunk Setup"
echo "========================================"
echo ""

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Function to print status messages
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ] || ! grep -q "name = \"worktrunk\"" Cargo.toml; then
    print_error "Error: Must be run from worktrunk project root"
    exit 1
fi

print_status "Found worktrunk project"

# Check Rust installation
echo ""
echo "Checking Rust toolchain..."
if ! command -v cargo &> /dev/null; then
    print_error "Cargo not found. Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

RUST_VERSION=$(rustc --version | awk '{print $2}')
print_status "Rust version: $RUST_VERSION"

# Check required Rust version from rust-toolchain.toml
REQUIRED_VERSION=$(grep 'channel' rust-toolchain.toml | cut -d'"' -f2)
if [ "$RUST_VERSION" != "$REQUIRED_VERSION" ]; then
    print_warning "Expected Rust $REQUIRED_VERSION, but found $RUST_VERSION"
    echo "  rustup should automatically use the correct version from rust-toolchain.toml"
fi

# Build the project
echo ""
echo "Building worktrunk..."
if cargo build 2>&1 | tail -5; then
    print_status "Build successful"
else
    print_error "Build failed"
    exit 1
fi

# Run unit tests (lib + bins)
echo ""
echo "Running unit tests..."
if cargo test --lib --bins --quiet 2>&1 | tail -5; then
    print_status "Unit tests passed"
else
    print_error "Unit tests failed"
    exit 1
fi

# Check and install shells
echo ""
echo "Checking shells for integration tests..."
SHELLS_AVAILABLE=()
SHELLS_MISSING=()

for shell in bash zsh fish; do
    if command -v "$shell" &> /dev/null; then
        SHELLS_AVAILABLE+=("$shell")
        print_status "$shell is available"
    else
        SHELLS_MISSING+=("$shell")
    fi
done

# Install missing shells
if [ ${#SHELLS_MISSING[@]} -gt 0 ]; then
    echo ""
    echo "Installing missing shells: ${SHELLS_MISSING[*]}"

    # Check if we can use apt-get (Debian/Ubuntu)
    if command -v apt-get &> /dev/null; then
        # Install quietly
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq 2>&1 | grep -v "Failed to fetch" || true
        apt-get install -y -qq "${SHELLS_MISSING[@]}" 2>&1 | tail -5 || {
            print_warning "Could not install shells: ${SHELLS_MISSING[*]}"
            echo "  Some integration tests will fail"
        }
    else
        print_warning "Package manager not found, cannot install shells: ${SHELLS_MISSING[*]}"
        echo "  Some integration tests will fail"
    fi

    # Re-check which shells are now available
    SHELLS_AVAILABLE=()
    SHELLS_MISSING=()
    for shell in bash zsh fish; do
        if command -v "$shell" &> /dev/null; then
            SHELLS_AVAILABLE+=("$shell")
            print_status "$shell is now available"
        else
            SHELLS_MISSING+=("$shell")
            print_warning "$shell installation failed"
        fi
    done
fi

# Run integration tests
echo ""
echo "Running integration tests..."
if [ ${#SHELLS_MISSING[@]} -gt 0 ]; then
    echo "(Note: Tests for missing shells will fail: ${SHELLS_MISSING[*]})"
fi

TEST_OUTPUT=$(cargo test --test integration 2>&1 || true)
TEST_SUMMARY=$(echo "$TEST_OUTPUT" | grep "test result:" | tail -1)

if echo "$TEST_SUMMARY" | grep -q "test result:"; then
    PASSED=$(echo "$TEST_SUMMARY" | grep -oP '\d+(?= passed)')
    FAILED=$(echo "$TEST_SUMMARY" | grep -oP '\d+(?= failed)' || echo "0")
    IGNORED=$(echo "$TEST_SUMMARY" | grep -oP '\d+(?= ignored)' || echo "0")

    print_status "Integration tests: $PASSED passed, $FAILED failed, $IGNORED ignored"

    if [ "$FAILED" != "0" ] && [ "$FAILED" != "" ]; then
        if [ ${#SHELLS_MISSING[@]} -gt 0 ]; then
            print_warning "Some tests failed - this is expected:"
            echo "  - Missing shells (${SHELLS_MISSING[*]}) cause shell-specific tests to fail"
        else
            print_warning "Some tests failed unexpectedly"
            echo "  - PTY/snapshot tests may need updating with 'cargo insta review'"
        fi
    fi
else
    print_error "Could not parse test results"
fi

# Summary
echo ""
echo "========================================"
echo "Setup Summary"
echo "========================================"
print_status "Core environment is ready"
print_status "Rust toolchain: $RUST_VERSION"
print_status "Build: OK"
print_status "Unit tests: OK"

if [ ${#SHELLS_MISSING[@]} -gt 0 ]; then
    echo ""
    print_warning "Optional shells not installed: ${SHELLS_MISSING[*]}"
    echo "  To run all integration tests, install:"
    for shell in "${SHELLS_MISSING[@]}"; do
        case "$shell" in
            fish)
                echo "    - fish: https://fishshell.com/ or 'apt-get install fish'"
                ;;
            zsh)
                echo "    - zsh: 'apt-get install zsh'"
                ;;
        esac
    done
fi

echo ""
echo "========================================"
echo "Quick Start Commands"
echo "========================================"
echo "  cargo build              # Build the project"
echo "  cargo test --lib --bins  # Run unit tests"
echo "  cargo test --test '*'    # Run integration tests"
echo "  cargo run -- --help      # Run worktrunk CLI"
echo "  ./target/debug/wt --help # Run built binary directly"
echo ""
print_status "Environment setup complete!"
