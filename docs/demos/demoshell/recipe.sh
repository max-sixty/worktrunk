# Demoshell base image script for the Try it page (Alpine arm64, runs as root).
# It installs the latest wt release and prepares the demo repository the guide
# walks through; see README.md.

apk add bash curl git less xz

curl -fsSL https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-aarch64-unknown-linux-musl.tar.xz \
  | tar -xJ -C /tmp
install -m 755 /tmp/worktrunk-aarch64-unknown-linux-musl/wt /usr/local/bin/wt
rm -rf /tmp/worktrunk-aarch64-unknown-linux-musl

# Visitors get bash with shell integration, so `wt switch` and `wt merge`
# change directory the way they do on an installed machine.
sed -i 's#^root:\(.*\):/bin/sh$#root:\1:/bin/bash#' /etc/passwd
echo '[ -f ~/.bashrc ] && . ~/.bashrc' > "$HOME/.bash_profile"
echo 'PS1='"'"'\[\e[32m\]\W\[\e[0m\] $ '"'" > "$HOME/.bashrc"
wt config shell install bash --yes

git config --global user.name "Demo User"
git config --global user.email demo@example.com
git config --global init.defaultBranch main
mkdir -p "$HOME/.config/worktrunk"
echo 'skip-commit-generation-prompt = true' > "$HOME/.config/worktrunk/config.toml"

# A small repository with two worktrees already in flight, so the first
# `wt list` has something to show: `docs` is a commit ahead of main, and
# `experiment` has uncommitted changes.
mkdir "$HOME/acme"
cd "$HOME/acme"
git init -q
printf '# Acme\n\nPricing for the Acme app.\n' > README.md
printf 'Starter: $19/month\nTeam: $49/month\n' > pricing.txt
git add .
git commit -qm "Add pricing"
wt switch --create docs --no-cd --yes
printf '\nPrices are billed monthly.\n' >> ../acme.docs/README.md
git -C ../acme.docs commit -qam "Note monthly billing"
wt switch --create experiment --no-cd --yes
printf 'Enterprise: contact us\n' >> ../acme.experiment/pricing.txt
