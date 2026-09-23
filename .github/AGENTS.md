# Worktrunk CI automation

The generic security model is in https://github.com/max-sixty/tend/blob/main/docs/security-model.md. This file records the Worktrunk-specific boundaries; verify current GitHub settings before changing them.

## Bot identity

worktrunk-bot is a PAT-backed GitHub user with write access. Only the repo owner can merge to main. The Merge access and Tag operations rulesets enforce those limits; required checks are test (linux), test (macos), test (windows), and fast-checks.

## Tokens and environments

- tend admits main and holds CLAUDE_CODE_OAUTH_TOKEN and TEND_BOT_TOKEN.
- release admits tags and holds AUR_SSH_PRIVATE_KEY and TEND_BOT_TOKEN.
- signing admits tags and holds SIGNPATH_API_TOKEN.
- github-pages admits main and uses OIDC without a stored secret.
- CODECOV_TOKEN is a repo secret allowlisted for coverage upload.
- crates.io uses Trusted Publishing through release, not a stored token.

A job naming an environment can read all its secrets. The branch and tag policies are therefore security gates: do not broaden them to make a workflow run. Tag environments admit every tag because the Tag operations ruleset controls creation and movement. tend and release each hold TEND_BOT_TOKEN because their admitted refs differ.

Generated tend workflows use environment {name: tend, deployment: false}. The deployment flag prevents pull_request_target runs from creating misleading deployment records on PRs. Edit the generator or its config, not generated tend-*.yaml files.

## Sandbox toolchain

Tend agents run in a copy-on-write view of the runner checkout and home. The `tend-setup` action installs the gate's tools; the following `setup` step checks their presence. Keep the probe when changing setup. A binary on PATH does not guarantee its service is reachable: Nix currently cannot connect to its daemon from the sandbox, so a flake.lock refresh needs a different execution path.

## Build environment

Swatinem/rust-cache hashes the CARGO* and RUST* variables visible at the cache step. Workflows sharing a cache must set the same variables before that step. A mismatch restores nothing without failing the job. Generated tend workflows currently build cold; measure restore cost before adding a separate cache scheme.
