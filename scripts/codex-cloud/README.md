# Codex Cloud environment

These scripts configure the `universal` Codex Cloud image for Worktrunk's full
development and test suite.

Create a Codex Cloud environment with the `universal` image, container caching
enabled, unrestricted agent internet access, and no environment variables or
secrets.

Use this setup command:

```bash
printf '%s  %s\n' '20415cbd8fa04364868f1cf54ef26e995a4f61b1a8df9a374c364dc41c4bbe23' scripts/codex-cloud/setup.sh | sha256sum -c - && bash scripts/codex-cloud/setup.sh
```

Use this maintenance command:

```bash
printf '%s  %s\n' '241f39eca59e60f5572d932c588803a139a28fb9641df5f1ef6442026966c63a' scripts/codex-cloud/maintenance.sh | sha256sum -c - && bash scripts/codex-cloud/maintenance.sh
```

The checksums are a security boundary: Codex checks out the task branch before
running environment commands, and the scripts run as root. A branch that
changes either script must fail verification instead of gaining root execution.
After an approved script change reaches the default branch, update its checksum
in the environment settings. That settings change also invalidates the cache.

The agent remains root, while the Cargo wrapper runs builds and tests as the
image's `ubuntu` user under `tini`. This matches the permission and process
reaping behavior expected by the test suite.

Validate the environment in a cloud task with:

```bash
cargo run -- hook pre-merge --yes
```
