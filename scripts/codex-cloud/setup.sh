#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

export DEBIAN_FRONTEND=noninteractive
export MISE_HTTP_RETRIES=6

apt-get update -qq
apt-get install -y -qq --no-install-recommends software-properties-common
add-apt-repository -y ppa:git-core/ppa
apt-get update -qq
apt-get install -y -qq --no-install-recommends git zsh fish xz-utils lsof tini

mise use --global task@3.52.0
uv tool install pre-commit==4.6.1
rustup component add rust-docs

install -d /root/.local/bin
tools_tmp="$(mktemp -d)"

insta_version=1.48.0
insta_archive="cargo-insta-x86_64-unknown-linux-gnu.tar.xz"
insta_url="https://github.com/mitsuhiko/insta/releases/download/${insta_version}/${insta_archive}"
insta_sha256="1c05a480a5a7f755f0ea15b2d8e2f71ad51b9c3a270d38ac72005c57ac0a1487"
curl -fsSL --retry 6 --retry-all-errors --retry-delay 2 \
  "$insta_url" -o "$tools_tmp/$insta_archive"
printf '%s  %s\n' "$insta_sha256" "$tools_tmp/$insta_archive" | sha256sum -c -
tar -xJf "$tools_tmp/$insta_archive" -C "$tools_tmp"
install -m 0755 "$tools_tmp/cargo-insta-x86_64-unknown-linux-gnu/cargo-insta" /root/.local/bin/cargo-insta

nextest_version=0.9.143
nextest_archive="cargo-nextest-${nextest_version}-x86_64-unknown-linux-gnu.tar.gz"
nextest_url="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${nextest_version}/${nextest_archive}"
nextest_sha256="66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e"
curl -fsSL --retry 6 --retry-all-errors --retry-delay 2 \
  "$nextest_url" -o "$tools_tmp/$nextest_archive"
printf '%s  %s\n' "$nextest_sha256" "$tools_tmp/$nextest_archive" | sha256sum -c -
tar -xzf "$tools_tmp/$nextest_archive" -C "$tools_tmp"
install -m 0755 "$tools_tmp/cargo-nextest" /root/.local/bin/cargo-nextest

nu_version=0.114.1
nu_archive="nu-${nu_version}-x86_64-unknown-linux-gnu.tar.gz"
nu_url="https://github.com/nushell/nushell/releases/download/${nu_version}/${nu_archive}"
nu_sha256="8802b26edcdf1a64477567b5ce909fbae3d72d731c8f0847892ea16c6fa73c53"
curl -fsSL --retry 6 --retry-all-errors --retry-delay 2 \
  "$nu_url" -o "$tools_tmp/$nu_archive"
printf '%s  %s\n' "$nu_sha256" "$tools_tmp/$nu_archive" | sha256sum -c -
tar -xzf "$tools_tmp/$nu_archive" -C "$tools_tmp"
install -m 0755 "$tools_tmp/nu-${nu_version}-x86_64-unknown-linux-gnu/nu" /root/.local/bin/nu

pwsh_version=7.6.4
pwsh_archive="powershell-${pwsh_version}-linux-x64.tar.gz"
pwsh_url="https://github.com/PowerShell/PowerShell/releases/download/v${pwsh_version}/${pwsh_archive}"
pwsh_sha256="4471b5a36bfe86ec7af8525d36bb1cacba0128e7aac22d05cc064bc00e604721"
curl -fsSL --retry 6 --retry-all-errors --retry-delay 2 \
  "$pwsh_url" -o "$tools_tmp/$pwsh_archive"
printf '%s  %s\n' "$pwsh_sha256" "$tools_tmp/$pwsh_archive" | sha256sum -c -
install -d /opt/microsoft/powershell/7
tar -xzf "$tools_tmp/$pwsh_archive" -C /opt/microsoft/powershell/7
chmod 0755 /opt/microsoft/powershell/7/pwsh
ln -sf /opt/microsoft/powershell/7/pwsh /usr/local/bin/pwsh

rm -r -- "$tools_tmp"

git_version="$(git version | awk '{print $3}')"
dpkg --compare-versions "$git_version" ge 2.54.0

chmod 0711 /root
chown -R ubuntu:ubuntu /root/.rustup
install -d -m 0755 -o ubuntu -g ubuntu target
install -d -m 0755 -o ubuntu -g ubuntu \
  /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit
chown -hR ubuntu:ubuntu "$repo_root"
for cache_dir in /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit; do
  [ ! -e "$cache_dir" ] || chown -R ubuntu:ubuntu "$cache_dir"
done
git config --system --add safe.directory "$repo_root"

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
    PATH="$toolchain_bin:/root/.local/bin:/root/.local/share/mise/installs/task/3.52.0:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
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
/usr/sbin/runuser -u ubuntu -- \
  /usr/bin/env HOME=/home/ubuntu USER=ubuntu LOGNAME=ubuntu \
  CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup \
  PRE_COMMIT_HOME=/root/.cache/pre-commit PATH="$PATH" \
  pre-commit install-hooks
for cache_dir in /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit; do
  [ ! -e "$cache_dir" ] || chown -R ubuntu:ubuntu "$cache_dir"
done
test "$(command -v cargo)" = /root/.cargo/bin/cargo
test "$(CODEX_CARGO_IDENTITY_PROBE=1 cargo)" = 1000
cargo --version
cargo fetch --locked
