#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

rustup component add rust-docs
chown -R ubuntu:ubuntu /root/.rustup
install -d -m 0755 -o ubuntu -g ubuntu target
install -d -m 0755 -o ubuntu -g ubuntu \
  /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit
find "$repo_root" -path "$repo_root/target" -prune -o -exec chown -h ubuntu:ubuntu {} +
for cache_dir in /root/.cargo/registry /root/.cargo/git /root/.cache/pre-commit; do
  [ ! -e "$cache_dir" ] || chown -R ubuntu:ubuntu "$cache_dir"
done
/usr/sbin/runuser -u ubuntu -- \
  /usr/bin/env HOME=/home/ubuntu USER=ubuntu LOGNAME=ubuntu \
  CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup \
  PRE_COMMIT_HOME=/root/.cache/pre-commit PATH="$PATH" \
  pre-commit install-hooks
cargo fetch --locked
