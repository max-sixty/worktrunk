#!/usr/bin/env bash
# Codex Cloud environment for worktrunk's test suite.
#
# The universal image is missing the shells, tools, and git version the suite
# needs, and it runs the agent as root while the suite assumes a non-root UID
# 1000 whose children are reaped by tini. `setup` installs the tools and
# replaces cargo with a wrapper that drops to `ubuntu`; `maintain` re-runs only
# the ownership and cache preparation that a cached environment still needs.
#
# Both commands are typed into the Codex Cloud environment settings, so they
# stay short and fixed across changes to this file. See README.md.

set -euo pipefail

TASK_VERSION=3.52.0
PRE_COMMIT_VERSION=4.6.2
INSTA_VERSION=1.48.0
NEXTEST_VERSION=0.9.143
NU_VERSION=0.115.0
PWSH_VERSION=7.6.5

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

require_root_on_universal_image() {
  if [ "$(id -u)" != 0 ] || [ "$(id -u ubuntu 2>/dev/null)" != 1000 ]; then
    echo "Codex Cloud $1 requires root and the universal image's ubuntu user" >&2
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
  apt-get install -y -qq --no-install-recommends git zsh fish xz-utils lsof tini

  uv tool install pre-commit=="$PRE_COMMIT_VERSION"
  install -d /root/.local/bin

  tools_tmp="$(mktemp -d)"

  install_binary \
    "https://github.com/go-task/task/releases/download/v$TASK_VERSION/task_linux_amd64.tar.gz" \
    02c679ffae53dca791804847d78b31731615894e292948397c971c87ac9e95bd \
    task.tar.gz task /root/.local/bin/task
  install_binary \
    "https://github.com/mitsuhiko/insta/releases/download/$INSTA_VERSION/cargo-insta-x86_64-unknown-linux-gnu.tar.xz" \
    1c05a480a5a7f755f0ea15b2d8e2f71ad51b9c3a270d38ac72005c57ac0a1487 \
    cargo-insta.tar.xz cargo-insta-x86_64-unknown-linux-gnu/cargo-insta \
    /root/.local/bin/cargo-insta
  install_binary \
    "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$NEXTEST_VERSION/cargo-nextest-$NEXTEST_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
    66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e \
    cargo-nextest.tar.gz cargo-nextest /root/.local/bin/cargo-nextest
  install_binary \
    "https://github.com/nushell/nushell/releases/download/$NU_VERSION/nu-$NU_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
    da83cfe482060d2c34b6b9af829975a313bce6b92e0398c3b2a59cb38630c7b2 \
    nu.tar.gz "nu-$NU_VERSION-x86_64-unknown-linux-gnu/nu" /root/.local/bin/nu

  pwsh_archive=powershell.tar.gz
  # The universal image supplies libicu; the version probe below fails if that changes.
  download \
    "https://github.com/PowerShell/PowerShell/releases/download/v$PWSH_VERSION/powershell-$PWSH_VERSION-linux-x64.tar.gz" \
    b34ab3b19acac1d3d4d0d3cfdb02acf62f457b0b6a962ff008132033f7566844 \
    "$pwsh_archive"
  install -d /opt/microsoft/powershell/7
  tar -xzf "$tools_tmp/$pwsh_archive" -C /opt/microsoft/powershell/7
  chmod 0755 /opt/microsoft/powershell/7/pwsh
  ln -sf /opt/microsoft/powershell/7/pwsh /usr/local/bin/pwsh
  rm -r -- "$tools_tmp"

  git_version="$(git version | awk '{print $3}')"
  dpkg --compare-versions "$git_version" ge 2.54.0
  chmod 0711 /root
  chown -hR ubuntu:ubuntu "$PWD"
  git config --system --add safe.directory "$PWD"

  rm /root/.cargo/bin/cargo
  install -m 0755 /dev/stdin /root/.cargo/bin/cargo <<'CARGO_WRAPPER'
#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    if [ "${CODEX_CARGO_IDENTITY_PROBE:-0}" = 1 ]; then
        exec /usr/bin/id -u
    fi
    cargo_path="$(/root/.cargo/bin/rustup which cargo)"
    toolchain_bin="${cargo_path%/cargo}"
    PATH="$toolchain_bin:/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    export PATH
    exec "$cargo_path" "$@"
fi

exec /usr/bin/tini -s -- /usr/sbin/runuser -u ubuntu -- \
    /usr/bin/env HOME=/home/ubuntu USER=ubuntu LOGNAME=ubuntu \
    CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup \
    PRE_COMMIT_HOME=/root/.cache/pre-commit PATH="$PATH" \
    CODEX_CARGO_IDENTITY_PROBE="${CODEX_CARGO_IDENTITY_PROBE:-0}" \
    /root/.cargo/bin/cargo "$@"
CARGO_WRAPPER

  for executable in task pre-commit cargo cargo-insta cargo-nextest git lsof pwsh tini zsh fish nu; do
    command -v "$executable" >/dev/null
  done
  pwsh -NoLogo -NoProfile -Command '$PSVersionTable.PSVersion.ToString()'
}

prepare() {
  chown -R ubuntu:ubuntu /root/.rustup
  install -d -m 0755 -o ubuntu -g ubuntu target \
    /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit
  find "$PWD" -path "$PWD/target" -prune -o -exec chown -h ubuntu:ubuntu {} +
  for cache_dir in /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit; do
    [ ! -e "$cache_dir" ] || chown -R ubuntu:ubuntu "$cache_dir"
  done
  /usr/sbin/runuser -u ubuntu -- \
    /usr/bin/env HOME=/home/ubuntu USER=ubuntu LOGNAME=ubuntu \
    RUSTUP_HOME=/root/.rustup \
    /root/.cargo/bin/rustup component add rust-docs
  /usr/sbin/runuser -u ubuntu -- \
    /usr/bin/env HOME=/home/ubuntu USER=ubuntu LOGNAME=ubuntu \
    CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup \
    PRE_COMMIT_HOME=/root/.cache/pre-commit PATH="$PATH" \
    pre-commit install-hooks
  test "$(command -v cargo)" = /root/.cargo/bin/cargo
  test "$(CODEX_CARGO_IDENTITY_PROBE=1 cargo)" = 1000
  cargo --version
  cargo fetch --locked
}

case "${1-}" in
  setup)
    require_root_on_universal_image setup
    setup
    prepare
    ;;
  maintain)
    require_root_on_universal_image maintenance
    prepare
    ;;
  *)
    echo "usage: ${BASH_SOURCE[0]} setup|maintain" >&2
    exit 1
    ;;
esac
