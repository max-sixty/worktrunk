#!/usr/bin/env bash
# Codex Cloud environment for worktrunk's test suite.
#
# The environment uses the `universal` image, caching, unrestricted internet,
# and no variables or secrets. Its two settings fields hold these commands, so
# they stay short and fixed across changes to this file:
#
#     bash dev/codex.sh setup
#     bash dev/codex.sh maintain
#
# Validate a built environment with `cargo run -- hook pre-merge --yes`.
#
# The universal image is missing the shells, tools, and git version the suite
# needs. `setup` installs them; `maintain` re-warms the toolchain and caches for
# an environment restored from cache. Both run as root, which is how Codex Cloud
# runs the agent — so the tests that need an unprivileged uid skip, as they
# already do under `task setup-web`. The standing TODO is in
# tests/integration_tests/approval_pty.rs.
#
# The gate runs `--all-features`, so nu and pwsh drive PTY snapshots their own
# versions can move. cargo-insta, cargo-nextest, and nu are kept level with the
# pins in .github/actions/test-setup/action.yaml. CI has no pwsh pin — it takes
# whatever the runner image ships — so that version answers to nothing but this
# file.

set -euo pipefail

PRE_COMMIT_VERSION=4.6.2
INSTA_VERSION=1.48.0
NEXTEST_VERSION=0.9.143
NU_VERSION=0.115.0
PWSH_VERSION=7.6.5

cd "$(dirname "${BASH_SOURCE[0]}")/.."

setup() {
  export DEBIAN_FRONTEND=noninteractive

  apt-get update -qq
  apt-get install -y -qq --no-install-recommends software-properties-common
  add-apt-repository -y ppa:git-core/ppa
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends git zsh fish xz-utils lsof

  uv tool install --force pre-commit=="$PRE_COMMIT_VERSION"

  tmp="$(mktemp -d)"
  curl -fsSL "https://github.com/mitsuhiko/insta/releases/download/$INSTA_VERSION/cargo-insta-x86_64-unknown-linux-gnu.tar.xz" | tar -xJ -C "$tmp"
  install -D -m 0755 "$tmp/cargo-insta-x86_64-unknown-linux-gnu/cargo-insta" /root/.local/bin/cargo-insta
  curl -fsSL "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$NEXTEST_VERSION/cargo-nextest-$NEXTEST_VERSION-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C "$tmp"
  install -D -m 0755 "$tmp/cargo-nextest" /root/.local/bin/cargo-nextest
  curl -fsSL "https://github.com/nushell/nushell/releases/download/$NU_VERSION/nu-$NU_VERSION-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C "$tmp"
  install -D -m 0755 "$tmp/nu-$NU_VERSION-x86_64-unknown-linux-gnu/nu" /root/.local/bin/nu
  rm -r -- "$tmp"

  install -d /opt/microsoft/powershell/7
  curl -fsSL "https://github.com/PowerShell/PowerShell/releases/download/v$PWSH_VERSION/powershell-$PWSH_VERSION-linux-x64.tar.gz" | tar -xz -C /opt/microsoft/powershell/7
  chmod 0755 /opt/microsoft/powershell/7/pwsh
  ln -sf /opt/microsoft/powershell/7/pwsh /usr/local/bin/pwsh
  # pwsh needs the image's libicu, and without it lands on PATH but aborts at
  # startup — so run it rather than looking for it.
  pwsh -NoLogo -NoProfile -Command '$PSVersionTable.PSVersion.ToString()'

  # A stale git fails the suite obscurely; fail here instead.
  dpkg --compare-versions "$(git version | awk '{print $3}')" ge 2.54.0
  git config --system --add safe.directory "$PWD"
}

maintain() {
  rustup component add rust-docs
  pre-commit install-hooks
  cargo fetch --locked
}

case "${1-}" in
  setup) setup; maintain ;;
  maintain) maintain ;;
  *) echo "usage: ${BASH_SOURCE[0]} setup|maintain" >&2; exit 1 ;;
esac
