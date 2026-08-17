# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
TASKFILE_SHA=d4d8493ba44567d0aa41532e10f463e5e02a02e14d33de8bb85e9422c2b3bd61; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml setup-codex
```

Maintenance command:

```bash
TASKFILE_SHA=d4d8493ba44567d0aa41532e10f463e5e02a02e14d33de8bb85e9422c2b3bd61; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml maintain-codex
```

The hash prevents a task branch from changing code run as root. After a reviewed
Taskfile change reaches the default branch, update both environment commands;
the settings change invalidates the cache.

Setup leaves Codex Cloud's standard user and toolchain behavior intact. It
installs missing system and development dependencies, then runs Rustup,
pre-commit, Cargo, builds, and tests directly rather than interposing a
Codex-specific Cargo wrapper.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
