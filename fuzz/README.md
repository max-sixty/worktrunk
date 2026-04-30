# Fuzz targets

Install `cargo-fuzz` once:

```bash
cargo install cargo-fuzz
```

Build every target:

```bash
cd fuzz
cargo +nightly fuzz build
```

Run a target briefly:

```bash
cargo +nightly fuzz run git_remote_url_parse -- -runs=1000
cargo +nightly fuzz run config_migrate_content -- -runs=1000
cargo +nightly fuzz run config_template_validation -- -runs=1000
cargo +nightly fuzz run sanitize_for_filename -- -runs=1000
```

Path-boundary fuzzing is pending until the copy-boundary helper lands as a
public API. This branch currently exposes `worktrunk::path::paths_match`, so
there is no stable target for that surface yet.
