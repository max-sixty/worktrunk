#!/bin/bash
###############################################################################
# Update Homebrew Formula
###############################################################################
#
# Updates the homebrew-worktrunk formula with the current version from
# Cargo.toml. Downloads the release tarball and computes the SHA256 hash.
#
# Note: macOS only (uses BSD sed syntax).
#
# Prerequisites:
# - The version must already be released on GitHub (tag pushed)
# - The homebrew-worktrunk repo must be checked out as a sibling directory
#
# Usage:
#   ./dev/update-homebrew.sh
#
# Automatically commits and pushes to homebrew-worktrunk.
#
###############################################################################

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_status() { echo -e "${GREEN}✓${NC} $1"; }
print_warning() { echo -e "${YELLOW}⚠${NC} $1"; }
print_error() { echo -e "${RED}✗${NC} $1"; }

# Get script directory and project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOMEBREW_REPO="$(cd "$PROJECT_ROOT/../homebrew-worktrunk" 2>/dev/null && pwd)" || true

echo "========================================"
echo "Update Homebrew Formula"
echo "========================================"
echo ""

# Check homebrew repo exists
if [[ -z "$HOMEBREW_REPO" || ! -d "$HOMEBREW_REPO" ]]; then
    print_error "homebrew-worktrunk repo not found at ../homebrew-worktrunk"
    echo "Clone it first:"
    echo "  cd $(dirname "$PROJECT_ROOT")"
    echo "  git clone git@github.com:max-sixty/homebrew-worktrunk.git"
    exit 1
fi

FORMULA_FILE="$HOMEBREW_REPO/Formula/wt.rb"
if [[ ! -f "$FORMULA_FILE" ]]; then
    print_error "Formula file not found: $FORMULA_FILE"
    exit 1
fi

# Check homebrew repo is clean
if ! git -C "$HOMEBREW_REPO" diff --quiet || ! git -C "$HOMEBREW_REPO" diff --cached --quiet; then
    print_error "homebrew-worktrunk repo has uncommitted changes"
    echo "Commit or stash changes first"
    exit 1
fi

# Get version from Cargo.toml
VERSION=$(grep '^version = ' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
if [[ -z "$VERSION" ]]; then
    print_error "Could not extract version from Cargo.toml"
    exit 1
fi
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    print_error "Invalid version format: $VERSION (expected semver like 0.1.10)"
    exit 1
fi
print_status "Current version: $VERSION"

# Check if tag exists on GitHub
TARBALL_URL="https://github.com/max-sixty/worktrunk/archive/refs/tags/v${VERSION}.tar.gz"
echo "Checking release exists..."
if ! curl --output /dev/null --silent --head --fail "$TARBALL_URL"; then
    print_error "Release v${VERSION} not found on GitHub"
    echo "Make sure you've pushed the tag:"
    echo "  cargo release patch --execute  # or minor/major"
    exit 1
fi
print_status "Release v${VERSION} found on GitHub"

# Download and compute SHA256
echo "Computing SHA256..."
TMPFILE=$(mktemp)
trap "rm -f $TMPFILE" EXIT
curl -sL "$TARBALL_URL" -o "$TMPFILE"
SHA256=$(shasum -a 256 "$TMPFILE" | cut -d' ' -f1)
print_status "SHA256: $SHA256"

# Update formula
echo "Updating formula..."

# Update URL line (version pattern: digits and dots)
sed -i '' "s|url \"https://github.com/max-sixty/worktrunk/archive/refs/tags/v[0-9.]*\.tar\.gz\"|url \"$TARBALL_URL\"|" "$FORMULA_FILE"

# Update sha256 line (64 hex characters)
sed -i '' "s|sha256 \"[a-f0-9]\{64\}\"|sha256 \"$SHA256\"|" "$FORMULA_FILE"

# Verify updates succeeded
if ! grep -q "v${VERSION}" "$FORMULA_FILE"; then
    print_error "Failed to update URL in formula"
    exit 1
fi
if ! grep -q "$SHA256" "$FORMULA_FILE"; then
    print_error "Failed to update SHA256 in formula"
    exit 1
fi

print_status "Formula updated: $FORMULA_FILE"

# Commit and push
cd "$HOMEBREW_REPO"
git add Formula/wt.rb

if git diff --cached --quiet; then
    echo ""
    echo "========================================"
    print_status "Formula already up to date (v${VERSION})"
    echo "========================================"
else
    echo "Committing and pushing..."
    git commit -m "Update to v${VERSION}"
    git push
    echo ""
    echo "========================================"
    print_status "Done! homebrew-worktrunk updated to v${VERSION}"
    echo "Users can now run: brew upgrade wt"
    echo "========================================"
fi
