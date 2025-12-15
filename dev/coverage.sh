#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required. Install with: cargo install cargo-llvm-cov" >&2
  exit 1
fi

# Always use the repo's pinned toolchain so llvm-tools are found even if the
# default cargo is a Homebrew build.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TOOLCHAIN="$(awk -F '\"' '/^channel/ {print $2; exit}' "${SCRIPT_DIR}/../rust-toolchain.toml")"
if command -v rustup >/dev/null 2>&1; then
  CARGO_BIN=(rustup run "${TOOLCHAIN}" cargo)
else
  CARGO_BIN=(cargo)
fi

# Ensure LLVM tools are available for coverage instrumentation.
if ! rustup component list --installed | grep -Eq '^llvm-tools(-preview)?'; then
  rustup component add llvm-tools-preview
fi

COVERAGE_DIR="target/llvm-cov"
LCOV_PATH="${COVERAGE_DIR}/lcov.info"
mkdir -p "${COVERAGE_DIR}"

# Include shell-integration-tests feature for comprehensive coverage of TUI/PTY code.
# Set NEXTEST_NO_INPUT_HANDLER to prevent nextest terminal cleanup issues with PTY tests.
export NEXTEST_NO_INPUT_HANDLER=1

# Run tests once with instrumentation, without generating a report yet.
"${CARGO_BIN[@]}" llvm-cov --locked --workspace --no-report --features shell-integration-tests "$@"

# Generate HTML (optionally open) and LCOV reports from the recorded data without rerunning tests.
OPEN_FLAG=()
if [ "${COVERAGE_OPEN:-1}" != "0" ]; then
  OPEN_FLAG=(--open)
fi

"${CARGO_BIN[@]}" llvm-cov report --html --output-dir "${COVERAGE_DIR}/html" "${OPEN_FLAG[@]}"
"${CARGO_BIN[@]}" llvm-cov report --lcov --output-path "${LCOV_PATH}"

echo "HTML report: ${COVERAGE_DIR}/html/index.html"
echo "LCOV report: ${LCOV_PATH}"
