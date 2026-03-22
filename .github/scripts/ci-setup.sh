#!/usr/bin/env bash
# Project-specific CI setup for Claude workflows.
# Passed as setup_command to continuous reusable workflows.
#
# Equivalent to .github/actions/claude-setup but as a shell script
# (reusable workflows can't invoke composite actions from the caller's repo).
set -euo pipefail

# Set build environment variables (persist across workflow steps via GITHUB_ENV)
echo "CARGO_TERM_COLOR=always" >> "$GITHUB_ENV"
echo "RUSTFLAGS=-C debuginfo=0" >> "$GITHUB_ENV"
echo "RUSTDOCFLAGS=-Dwarnings" >> "$GITHUB_ENV"

# Install cargo tools (skip if already available)
if ! command -v cargo-insta &>/dev/null; then
  cargo install cargo-insta --version '=1.46.3' --locked &
fi
if ! command -v cargo-nextest &>/dev/null; then
  cargo install cargo-nextest --version '=0.9.130' --locked &
fi

# Install uv and pre-commit
if ! command -v uv &>/dev/null; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
  echo "$HOME/.local/bin" >> "$GITHUB_PATH"
  export PATH="$HOME/.local/bin:$PATH"  # for this script's remaining commands
fi
if ! command -v pre-commit &>/dev/null; then
  uv tool install pre-commit
fi

# Install shells for integration tests
sudo apt-get update -qq
sudo apt-get install -y -qq zsh fish

# Wait for background cargo installs
wait
