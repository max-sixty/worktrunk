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

# Install shells for integration tests
echo ""
echo "Installing shells for integration tests..."
if command -v apt-get &> /dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq 2>&1 | grep -v "Failed to fetch" || true
    apt-get install -y -qq zsh fish 2>&1 | tail -3 || true
fi
for shell in bash zsh fish; do
    command -v "$shell" &> /dev/null && print_status "$shell available" || print_warning "$shell missing"
done

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
cargo install cargo-insta cargo-nextest --quiet
cargo install --path . --quiet
print_status "Installed cargo-insta, cargo-nextest, worktrunk"

echo ""
print_status "Setup complete! Run 'wt --help' to get started."
