# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
TASKFILE_SHA=f14dbc890ec1a08163efffc63bd1dd750beb820afe18a2941f846e2b1b2c7227; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml setup-codex
```

Maintenance command:

```bash
TASKFILE_SHA=f14dbc890ec1a08163efffc63bd1dd750beb820afe18a2941f846e2b1b2c7227; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml maintain-codex
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
