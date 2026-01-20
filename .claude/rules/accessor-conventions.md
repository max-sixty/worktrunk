# Accessor Function Naming Conventions

Function prefixes signal return behavior and side effects.

## Prefix Semantics

| Prefix | Returns | Side Effects | Error Handling | Example |
|--------|---------|--------------|----------------|---------|
| (bare noun) | `Option<T>` or `T` | None (may cache) | Returns None/default if absent | `config()`, `switch_previous()` |
| `set_*` | `Result<()>` | Writes state | Errors on failure | `set_switch_previous()`, `set_config()` |
| `require_*` | `Result<T>` | None | Errors if absent | `require_branch()`, `require_target_ref()` |
| `fetch_*` | `Result<T>` | Network I/O | Errors on failure | `fetch_pr_info()`, `fetch_mr_info()` |
| `load_*` | `Result<T>` | File I/O | Errors on failure | `load_project_config()`, `load_template()` |

## When to Use Each

**Bare nouns** — Value may not exist and that's fine (Rust stdlib convention)
```rust
// Returns Option - caller handles None
if let Some(prev) = repo.switch_previous() {
    // use prev
}

// Returns Option - read from git config
if let Some(marker) = repo.branch_marker("feature") {
    println!("Branch marker: {marker}");
}
```

**`set_*`** — Write state to storage
```rust
// Set the previous branch for wt switch -
repo.set_switch_previous(Some(&branch))?;
```

**`require_*`** — Value must exist for operation to proceed
```rust
// Error propagates if branch is missing
let branch = env.require_branch("squash")?;
```

**`fetch_*`** — Retrieve from external service (network)
```rust
// May fail due to network, auth, rate limits
let pr = fetch_pr_info(123, &repo_root)?;
```

**`load_*`** — Read from filesystem
```rust
// May fail due to missing file, parse errors
let config = repo.load_project_config()?;
```

## Anti-patterns

Avoid mixing semantics:
- Don't use bare nouns if the function makes network calls (use `fetch_*`)
- Don't use bare nouns if absence is an error (use `require_*`)
- Don't use `load_*` for computed values (use bare nouns)
- Don't use `get_*` prefix — use bare nouns instead (Rust convention)

## Related Patterns

For caching behavior, see `caching-strategy.md`.
