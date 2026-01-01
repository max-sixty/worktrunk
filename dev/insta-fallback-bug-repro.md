# cargo-insta config fallback bug

`test_runner_fallback: true` in config file is ignored; only CLI flag works.

## Repro

```bash
# Setup: ensure nextest is NOT installed
cargo uninstall cargo-nextest 2>/dev/null || true

# Create test project
cargo new /tmp/insta-repro && cd /tmp/insta-repro
cargo add insta --dev

# Add a snapshot test
cat > src/lib.rs << 'EOF'
#[test]
fn test_snapshot() {
    insta::assert_snapshot!("hello");
}
EOF

# Create config with fallback enabled
mkdir -p .config
cat > .config/insta.yaml << 'EOF'
test:
  runner: "nextest"
  test_runner_fallback: true
EOF

# Run: fallback does NOT work (bug)
cargo insta test
# Error: no such command: `nextest`

# CLI flag DOES work
cargo insta test --test-runner-fallback
# Runs successfully with cargo test
```

## Expected

Config `test_runner_fallback: true` should fall back to cargo test when nextest unavailable.

## Actual

Config setting ignored. Only `--test-runner-fallback` CLI flag works.

## Versions

- cargo-insta 1.45.1
