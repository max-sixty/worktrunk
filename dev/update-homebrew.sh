#!/bin/bash
# Update homebrew-worktrunk formula with current version from Cargo.toml.
# Fetches pre-computed SHA256 hashes from GitHub release assets.
# macOS only (BSD sed syntax). Requires sibling ../homebrew-worktrunk checkout.
set -e

GREEN='\033[0;32m'; RED='\033[0;31m'; NC='\033[0m'
ok() { echo -e "${GREEN}✓${NC} $1"; }
err() { echo -e "${RED}✗${NC} $1"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOMEBREW_REPO="$(cd "$PROJECT_ROOT/../homebrew-worktrunk" 2>/dev/null && pwd)" || true
FORMULA="$HOMEBREW_REPO/Formula/wt.rb"

[[ -f "$FORMULA" ]] || err "Formula not found. Clone homebrew-worktrunk as sibling directory."
git -C "$HOMEBREW_REPO" diff --quiet && git -C "$HOMEBREW_REPO" diff --cached --quiet || err "homebrew-worktrunk has uncommitted changes"

VERSION=$(grep '^version = ' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || err "Invalid version: $VERSION"
ok "Version: $VERSION"

BASE="https://github.com/max-sixty/worktrunk/releases/download/v${VERSION}"

# Fetch pre-computed SHA256 from release assets (cargo-dist publishes .sha256 files)
fetch_sha() {
    local sha=$(curl -sfL "$BASE/worktrunk-$1.tar.xz.sha256" | cut -d' ' -f1)
    [[ ${#sha} -eq 64 ]] || err "Failed to fetch SHA256 for $1"
    echo "$sha"
}

SHA1=$(fetch_sha "aarch64-apple-darwin")
SHA2=$(fetch_sha "x86_64-apple-darwin")
SHA3=$(fetch_sha "x86_64-unknown-linux-musl")
ok "Fetched SHA256 hashes"

# Update formula: version, URLs, and SHA256 values (in order they appear)
sed -i '' "s|version \"[0-9.]*\"|version \"$VERSION\"|" "$FORMULA"
sed -i '' "s|/v[0-9.]*/worktrunk-|/v${VERSION}/worktrunk-|g" "$FORMULA"

TMPFILE=$(mktemp)
awk -v s1="$SHA1" -v s2="$SHA2" -v s3="$SHA3" '
/sha256 "[a-f0-9]{64}"/ { n++; gsub(/sha256 "[a-f0-9]{64}"/, "sha256 \"" (n==1?s1:n==2?s2:s3) "\"") }
{ print }' "$FORMULA" > "$TMPFILE" && mv "$TMPFILE" "$FORMULA"

grep -q "version \"${VERSION}\"" "$FORMULA" || err "Failed to update version"
ok "Formula updated"

cd "$HOMEBREW_REPO"
git add Formula/wt.rb
if git diff --cached --quiet; then
    ok "Already up to date (v${VERSION})"
else
    git commit -m "Update to v${VERSION}"
    git push
    ok "Pushed v${VERSION} to homebrew-worktrunk"
fi
