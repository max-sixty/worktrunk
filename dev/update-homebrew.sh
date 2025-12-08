#!/bin/bash
###############################################################################
# Update Homebrew Formula
###############################################################################
#
# Updates the homebrew-worktrunk formula with the current version from
# Cargo.toml. Downloads release binaries and computes SHA256 hashes.
#
# Note: macOS only (uses BSD sed syntax).
#
# Prerequisites:
# - The release CI must have completed (binaries published to GitHub)
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

BASE_URL="https://github.com/max-sixty/worktrunk/releases/download/v${VERSION}"

# Check if release binaries exist
echo "Checking release binaries..."
FIRST_BINARY="${BASE_URL}/worktrunk-aarch64-apple-darwin.tar.xz"
if ! curl --output /dev/null --silent --head --fail "$FIRST_BINARY"; then
    print_error "Release binaries not found for v${VERSION}"
    echo "Make sure the release CI has completed."
    echo "Check: https://github.com/max-sixty/worktrunk/releases/tag/v${VERSION}"
    exit 1
fi
print_status "Release v${VERSION} binaries found"

# Download and compute SHA256 for each platform
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

compute_sha() {
    local platform=$1
    local url="${BASE_URL}/worktrunk-${platform}.tar.xz"
    local tmpfile="$TMPDIR/${platform}.tar.xz"

    echo "Computing SHA256 for $platform..."
    if ! curl -sL "$url" -o "$tmpfile"; then
        print_error "Failed to download $platform binary"
        exit 1
    fi

    local sha=$(shasum -a 256 "$tmpfile" | cut -d' ' -f1)
    print_status "$platform: $sha"
    echo "$sha"
}

SHA_AARCH64_DARWIN=$(compute_sha "aarch64-apple-darwin" | tail -1)
SHA_X86_64_DARWIN=$(compute_sha "x86_64-apple-darwin" | tail -1)
SHA_X86_64_LINUX=$(compute_sha "x86_64-unknown-linux-musl" | tail -1)

# Update formula
echo "Updating formula..."

# Update version line
sed -i '' "s|version \"[0-9.]*\"|version \"$VERSION\"|" "$FORMULA_FILE"

# Update each platform's URL
sed -i '' "s|worktrunk/releases/download/v[0-9.]*/worktrunk-aarch64-apple-darwin.tar.xz|worktrunk/releases/download/v${VERSION}/worktrunk-aarch64-apple-darwin.tar.xz|g" "$FORMULA_FILE"
sed -i '' "s|worktrunk/releases/download/v[0-9.]*/worktrunk-x86_64-apple-darwin.tar.xz|worktrunk/releases/download/v${VERSION}/worktrunk-x86_64-apple-darwin.tar.xz|g" "$FORMULA_FILE"
sed -i '' "s|worktrunk/releases/download/v[0-9.]*/worktrunk-x86_64-unknown-linux-musl.tar.xz|worktrunk/releases/download/v${VERSION}/worktrunk-x86_64-unknown-linux-musl.tar.xz|g" "$FORMULA_FILE"

# Update sha256 values in order they appear (aarch64-darwin, x86_64-darwin, x86_64-linux)
TMPFORMULA="$TMPDIR/formula.rb"
cp "$FORMULA_FILE" "$TMPFORMULA"

awk -v sha1="$SHA_AARCH64_DARWIN" \
    -v sha2="$SHA_X86_64_DARWIN" \
    -v sha3="$SHA_X86_64_LINUX" '
BEGIN { count = 0 }
/sha256 "[a-f0-9]{64}"/ {
    count++
    if (count == 1) gsub(/sha256 "[a-f0-9]{64}"/, "sha256 \"" sha1 "\"")
    else if (count == 2) gsub(/sha256 "[a-f0-9]{64}"/, "sha256 \"" sha2 "\"")
    else if (count == 3) gsub(/sha256 "[a-f0-9]{64}"/, "sha256 \"" sha3 "\"")
}
{ print }
' "$TMPFORMULA" > "$FORMULA_FILE"

# Verify updates succeeded
if ! grep -q "version \"${VERSION}\"" "$FORMULA_FILE"; then
    print_error "Failed to update version in formula"
    exit 1
fi
if ! grep -q "v${VERSION}" "$FORMULA_FILE"; then
    print_error "Failed to update URLs in formula"
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
