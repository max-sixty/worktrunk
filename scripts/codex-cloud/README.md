# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
mise x task -- task -t scripts/codex-cloud/Taskfile.yaml setup-codex
```

Maintenance command:

```bash
mise x task -- task -t scripts/codex-cloud/Taskfile.yaml maintain-codex
```

Mise only bootstraps Task for these commands. Setup installs Task globally for
later use.

The agent remains root. Toolchain-sensitive Rustup and pre-commit steps run as
`ubuntu`; the Cargo wrapper also runs builds and tests as `ubuntu` under `tini`,
matching the suite's permission and child-reaping assumptions.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
