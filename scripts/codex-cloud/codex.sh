#!/usr/bin/env bash
# Codex Cloud environment for worktrunk's test suite.
#
# The universal image is missing the shells, tools, and git version the suite
# needs. `setup` installs them; `maintain` re-warms the toolchain and caches for
# an environment restored from cache.
#
# Everything runs as root, which is how Codex Cloud runs the agent. The two
# permission tests skip there, as they already do under `task setup-web` — see
# the TODO in tests/integration_tests/approval_pty.rs.
#
# Both commands are typed into the Codex Cloud environment settings, so they
# stay short and fixed across changes to this file. See README.md.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."
source scripts/tool-pins.sh

require_root() {
  if [ "$(id -u)" != 0 ]; then
    echo "Codex Cloud $1 requires root" >&2
    exit 1
  fi
}

download() {
  curl -fsSL --retry 6 --retry-all-errors --retry-delay 2 \
    "$1" -o "$tools_tmp/$3"
  printf '%s  %s\n' "$2" "$tools_tmp/$3" | sha256sum -c -
}

install_binary() {
  download "$1" "$2" "$3"
  case "$3" in
    *.tar.xz) tar -xJf "$tools_tmp/$3" -C "$tools_tmp" ;;
    *.tar.gz) tar -xzf "$tools_tmp/$3" -C "$tools_tmp" ;;
  esac
  install -m 0755 "$tools_tmp/$4" "$5"
}

setup() {
  export DEBIAN_FRONTEND=noninteractive

  apt-get update -qq
  apt-get install -y -qq --no-install-recommends software-properties-common
  add-apt-repository -y ppa:git-core/ppa
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends git zsh fish xz-utils lsof

  uv tool install --force pre-commit=="$PRE_COMMIT_VERSION"
  install -d /root/.local/bin

  tools_tmp="$(mktemp -d)"

  install_binary \
    "https://github.com/go-task/task/releases/download/v$TASK_VERSION/task_linux_amd64.tar.gz" \
    "$TASK_SHA256" task.tar.gz task /root/.local/bin/task
  install_binary \
    "https://github.com/mitsuhiko/insta/releases/download/$INSTA_VERSION/cargo-insta-x86_64-unknown-linux-gnu.tar.xz" \
    "$INSTA_SHA256" cargo-insta.tar.xz \
    cargo-insta-x86_64-unknown-linux-gnu/cargo-insta /root/.local/bin/cargo-insta
  install_binary \
    "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$NEXTEST_VERSION/cargo-nextest-$NEXTEST_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
    "$NEXTEST_SHA256" cargo-nextest.tar.gz cargo-nextest /root/.local/bin/cargo-nextest
  install_binary \
    "https://github.com/nushell/nushell/releases/download/$NU_VERSION/nu-$NU_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
    "$NU_SHA256" nu.tar.gz "nu-$NU_VERSION-x86_64-unknown-linux-gnu/nu" /root/.local/bin/nu

  pwsh_archive=powershell.tar.gz
  # The universal image supplies libicu; the version probe below fails if that changes.
  download \
    "https://github.com/PowerShell/PowerShell/releases/download/v$PWSH_VERSION/powershell-$PWSH_VERSION-linux-x64.tar.gz" \
    "$PWSH_TARBALL_SHA256" "$pwsh_archive"
  install -d /opt/microsoft/powershell/7
  tar -xzf "$tools_tmp/$pwsh_archive" -C /opt/microsoft/powershell/7
  chmod 0755 /opt/microsoft/powershell/7/pwsh
  ln -sf /opt/microsoft/powershell/7/pwsh /usr/local/bin/pwsh
  rm -r -- "$tools_tmp"

  git_version="$(git version | awk '{print $3}')"
  dpkg --compare-versions "$git_version" ge 2.54.0
  git config --system --add safe.directory "$PWD"

  for executable in task pre-commit cargo cargo-insta cargo-nextest git lsof pwsh zsh fish nu; do
    command -v "$executable" >/dev/null
  done
  pwsh -NoLogo -NoProfile -Command '$PSVersionTable.PSVersion.ToString()'
}

maintain() {
  rustup component add rust-docs
  pre-commit install-hooks
  cargo fetch --locked
}

case "${1-}" in
  setup)
    require_root setup
    setup
    maintain
    ;;
  maintain)
    require_root maintenance
    maintain
    ;;
  *)
    echo "usage: ${BASH_SOURCE[0]} setup|maintain" >&2
    exit 1
    ;;
esac
