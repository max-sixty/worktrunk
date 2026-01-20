# Accessor Function Naming Conventions

Function prefixes signal return behavior and side effects.

## Prefix Semantics

| Prefix | Returns | Side Effects | Error Handling | Example |
|--------|---------|--------------|----------------|---------|
| `get_*` | `Option<T>` or `T` | None (may cache) | Returns None/default if absent | `get_config()`, `get_switch_previous()` |
| `require_*` | `Result<T>` | None | Errors if absent | `require_branch()`, `require_target_ref()` |
| `fetch_*` | `Result<T>` | Network I/O | Errors on failure | `fetch_pr_info()`, `fetch_mr_info()` |
| `load_*` | `Result<T>` | File I/O | Errors on failure | `load_project_config()`, `load_template()` |

## When to Use Each

**`get_*`** — Value may not exist and that's fine
```rust
// Returns Option - caller handles None
if let Some(prev) = repo.get_switch_previous() {
    // use prev
}

// Returns T with sensible default
let width = get_terminal_width(); // defaults to 80 if detection fails
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
- Don't use `get_*` if the function makes network calls (use `fetch_*`)
- Don't use `get_*` if absence is an error (use `require_*`)
- Don't use `load_*` for computed values (use `get_*`)

## Related Patterns

For caching behavior, see `caching-strategy.md`.
