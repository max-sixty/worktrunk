#!/bin/sh
set -eu

# Hand-test: run docs/static/install.sh inside clean Docker containers and
# verify it produces a working `wt`. Not wired into CI because it hits the
# real network (GitHub releases) and needs Docker.
#
# Usage:
#   sh dev/install/test-containers.sh             # test all images
#   sh dev/install/test-containers.sh ubuntu      # test one image
#
# The script under test is the one checked into this repo (not fetched from
# worktrunk.dev) — we're testing the current source, not what's published.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/../../docs/static/install.sh"

if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required for the container test. Install Docker Desktop or"
    echo "equivalent, then re-run."
    exit 1
fi

# Each entry: image | setup. The upstream cargo-dist installer downloads a
# tar.xz release and extracts it, so `xz` must be on PATH alongside curl.
IMAGES="
ubuntu:24.04|apt-get update -qq && apt-get install -y -qq curl ca-certificates xz-utils
debian:12|apt-get update -qq && apt-get install -y -qq curl ca-certificates xz-utils
fedora:41|dnf install -y -q curl xz
alpine:3.20|apk add --no-cache curl ca-certificates xz
archlinux:latest|pacman -Sy --noconfirm curl ca-certificates xz
"

FILTER="${1:-}"
PASS=0
FAIL=0
FAILED=""

run_one() {
    image="$1"
    setup="$2"

    echo ""
    echo "=== $image ==="

    # Copy install.sh into the container, run setup + install, then verify
    # `wt --version` prints a worktrunk version. Use `sh -c` as entrypoint so
    # the script runs regardless of the image's default command. We capture
    # into a temp file rather than piping — a pipe would mask docker's exit
    # code behind sed's.
    log="$(mktemp)"
    set +e
    docker run --rm \
        -v "$INSTALL_SH:/tmp/install.sh:ro" \
        "$image" \
        sh -c "set -e; $setup >/dev/null; sh /tmp/install.sh; . \${CARGO_HOME:-\$HOME/.cargo}/env; wt --version" \
        >"$log" 2>&1
    rc=$?
    set -e
    sed 's/^/  /' "$log"
    rm -f "$log"

    if [ "$rc" -eq 0 ]; then
        echo "  PASS: $image"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $image (exit $rc)"
        FAIL=$((FAIL + 1))
        FAILED="$FAILED $image"
    fi
}

echo "Testing install.sh in containers..."

# Shell-splitting on newlines in POSIX sh: set IFS to newline, iterate.
old_ifs="$IFS"
IFS='
'
for entry in $IMAGES; do
    IFS='|'
    # shellcheck disable=SC2086
    set -- $entry
    IFS="$old_ifs"
    image="$1"
    setup="$2"
    if [ -n "$FILTER" ] && ! echo "$image" | grep -q "$FILTER"; then
        continue
    fi
    run_one "$image" "$setup"
    IFS='
'
done
IFS="$old_ifs"

echo ""
echo "=== Results ==="
echo "  $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    echo "  Failed:$FAILED"
    exit 1
fi
