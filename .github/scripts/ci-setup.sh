#!/usr/bin/env bash
# Project-specific CI setup for Claude workflows.
# Passed as setup_command to continuous reusable workflows.
#
# Equivalent to .github/actions/claude-setup but as a shell script
# (reusable workflows can't invoke composite actions from the caller's repo).
set -euo pipefail

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
  export PATH="$HOME/.local/bin:$PATH"
fi
if ! command -v pre-commit &>/dev/null; then
  uv tool install pre-commit
fi

# Install shells for integration tests
sudo apt-get update -qq
sudo apt-get install -y -qq zsh fish

# Wait for background cargo installs
wait
