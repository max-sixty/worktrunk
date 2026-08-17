# Codex Cloud

The environment uses the `universal` image, caching, unrestricted internet,
and no variables or secrets.

Setup command:

```bash
bash scripts/codex-cloud/codex.sh setup
```

Maintenance command:

```bash
bash scripts/codex-cloud/codex.sh maintain
```

The agent remains root. Toolchain-sensitive Rustup and pre-commit steps run as
`ubuntu`; the Cargo wrapper also runs builds and tests as `ubuntu` under `tini`,
matching the suite's permission and child-reaping assumptions.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
