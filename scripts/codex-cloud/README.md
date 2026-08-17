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

Everything runs as root, which is how Codex Cloud runs the agent.

Validation:

```bash
cargo run -- hook pre-merge --yes
```
