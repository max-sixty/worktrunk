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

# Platform-specific binaries
PLATFORMS=(
    "aarch64-apple-darwin"
    "x86_64-apple-darwin"
    "x86_64-unknown-linux-musl"
)

BASE_URL="https://github.com/max-sixty/worktrunk/releases/download/v${VERSION}"

# Check if release binaries exist
echo "Checking release binaries..."
FIRST_BINARY="${BASE_URL}/worktrunk-${PLATFORMS[0]}.tar.xz"
if ! curl --output /dev/null --silent --head --fail "$FIRST_BINARY"; then
    print_error "Release binaries not found for v${VERSION}"
    echo "Make sure the release CI has completed."
    echo "Check: https://github.com/max-sixty/worktrunk/releases/tag/v${VERSION}"
    exit 1
fi
print_status "Release v${VERSION} binaries found"

# Download and compute SHA256 for each platform
declare -A SHAS
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

for platform in "${PLATFORMS[@]}"; do
    echo "Computing SHA256 for $platform..."
    URL="${BASE_URL}/worktrunk-${platform}.tar.xz"
    TMPFILE="$TMPDIR/$platform.tar.xz"

    if ! curl -sL "$URL" -o "$TMPFILE"; then
        print_error "Failed to download $platform binary"
        exit 1
    fi

    SHA=$(shasum -a 256 "$TMPFILE" | cut -d' ' -f1)
    SHAS[$platform]=$SHA
    print_status "$platform: $SHA"
done

# Update formula
echo "Updating formula..."

# Update version line
sed -i '' "s|version \"[0-9.]*\"|version \"$VERSION\"|" "$FORMULA_FILE"

# Update each platform's URL and sha256
for platform in "${PLATFORMS[@]}"; do
    OLD_URL_PATTERN="worktrunk/releases/download/v[0-9.]*/worktrunk-${platform}.tar.xz"
    NEW_URL="worktrunk/releases/download/v${VERSION}/worktrunk-${platform}.tar.xz"

    # Update URL (preserve the full URL structure)
    sed -i '' "s|${OLD_URL_PATTERN}|${NEW_URL}|g" "$FORMULA_FILE"
done

# Update sha256 values - the formula has them in order: aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-musl
# We need to update them in sequence. Use a temp file approach.
TMPFORMULA="$TMPDIR/formula.rb"
cp "$FORMULA_FILE" "$TMPFORMULA"

# Replace sha256 values in order they appear
awk -v sha1="${SHAS[aarch64-apple-darwin]}" \
    -v sha2="${SHAS[x86_64-apple-darwin]}" \
    -v sha3="${SHAS[x86_64-unknown-linux-musl]}" '
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
