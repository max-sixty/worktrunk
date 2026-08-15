# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
TASKFILE_SHA=abce64dd8aea02ba4dc9d8e8a5c6837c3b8937c4fec4d213373593ec6b0eb05e; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml setup-codex
```

Maintenance command:

```bash
TASKFILE_SHA=abce64dd8aea02ba4dc9d8e8a5c6837c3b8937c4fec4d213373593ec6b0eb05e; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml maintain-codex
```

The hash prevents a task branch from changing code run as root. After a reviewed
Taskfile change reaches the default branch, update both environment commands;
the settings change invalidates the cache.

The agent remains root. Toolchain-sensitive Rustup and pre-commit steps run as
`ubuntu`; the Cargo wrapper also runs builds and tests as `ubuntu` under `tini`,
matching the suite's permission and child-reaping assumptions.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
