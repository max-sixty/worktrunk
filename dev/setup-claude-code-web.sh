#!/bin/bash
###############################################################################
# Claude Code Web - Environment Setup Script
###############################################################################
#
# This script prepares a fresh Claude Code web environment for worktrunk
# development. It installs required dependencies but does NOT run tests.
#
# What it does:
# - Verifies Rust toolchain (1.90.0)
# - Installs required shells (zsh, fish) on Debian/Ubuntu
# - Installs GitHub CLI (gh) for working with PRs/issues
# - Builds the project
# - Installs dev tools: cargo-insta, cargo-nextest, worktrunk
#
# After running this script, run tests with:
#   cargo test --lib --bins           # Unit tests
#   cargo test --test integration     # Integration tests
#   cargo run -- beta run-hook pre-merge  # All tests (via pre-merge hook)
#
# Usage:
#   ./dev/setup-claude-code-web.sh
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

# Install GitHub CLI (gh)
echo ""
echo "Installing GitHub CLI..."
if command -v gh &> /dev/null; then
    print_status "gh already installed"
else
    GH_VERSION="2.63.2"
    ARCH="linux_amd64"
    URL="https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_${ARCH}.tar.gz"

    mkdir -p ~/bin
    TEMP=$(mktemp -d)
    curl -fsSL "$URL" | tar -xz -C "$TEMP"
    mv "$TEMP/gh_${GH_VERSION}_${ARCH}/bin/gh" ~/bin/gh
    chmod +x ~/bin/gh
    rm -rf "$TEMP"

    export PATH="$HOME/bin:$PATH"
    print_status "gh installed to ~/bin/gh"
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

# Install development tools
echo ""
echo "Installing development tools..."

# Install cargo-insta (snapshot testing)
if cargo insta --version &> /dev/null; then
    print_status "cargo-insta already installed"
else
    echo "  Installing cargo-insta..."
    cargo install cargo-insta --quiet
    print_status "cargo-insta installed"
fi

# Install cargo-nextest (faster test runner)
if cargo nextest --version &> /dev/null; then
    print_status "cargo-nextest already installed"
else
    echo "  Installing cargo-nextest..."
    cargo install cargo-nextest --quiet
    print_status "cargo-nextest installed"
fi

# Install worktrunk itself
echo "  Installing worktrunk from source..."
cargo install --path . --quiet
print_status "worktrunk installed"

# Summary
echo ""
echo "========================================"
echo "Setup Summary"
echo "========================================"
print_status "Environment is ready for development"
print_status "Rust toolchain: $RUST_VERSION"
print_status "Build: OK"
print_status "Shells available: ${SHELLS_AVAILABLE[*]}"
if command -v gh &> /dev/null; then
    print_status "GitHub CLI (gh) installed"
fi
print_status "Dev tools: cargo-insta, cargo-nextest, worktrunk"

if [ ${#SHELLS_MISSING[@]} -gt 0 ]; then
    echo ""
    print_warning "Some shells not installed: ${SHELLS_MISSING[*]}"
    echo "  (Tests for these shells will fail)"
fi

echo ""
echo "========================================"
echo "Next Steps"
echo "========================================"
echo "Run tests:"
echo "  cargo test --lib --bins                # Unit tests"
echo "  cargo test --test integration          # Integration tests"
echo "  cargo run -- beta run-hook pre-merge   # All tests (via pre-merge hook)"
echo ""
echo "Or start developing:"
echo "  cargo run -- --help                    # Run worktrunk CLI"
echo "  ./target/debug/wt list                 # Try a command"
echo ""
print_status "Setup complete!"
