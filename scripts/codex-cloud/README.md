# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
TASKFILE_SHA=342b9134ce86621c9ee22bac3518ace25dba356f872d99521037255dbe3cc5bd; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml setup-codex
```

Maintenance command:

```bash
TASKFILE_SHA=342b9134ce86621c9ee22bac3518ace25dba356f872d99521037255dbe3cc5bd; printf '%s  %s\n' "$TASKFILE_SHA" scripts/codex-cloud/Taskfile.yaml | sha256sum -c - && MISE_NO_CONFIG=1 MISE_HTTP_RETRIES=6 mise x task@3.52.0 -- task -t scripts/codex-cloud/Taskfile.yaml maintain-codex
```

The hash prevents a task branch from changing code run as root. After a reviewed
Taskfile change reaches the default branch, update both environment commands;
the settings change invalidates the cache.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
