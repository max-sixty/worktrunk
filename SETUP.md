# Worktrunk Development Environment - Claude Code Web

This document describes the development environment setup for working on worktrunk in Claude Code web.

## Quick Start

```bash
# Run the setup script to verify environment
./setup-environment.sh

# Build the project
cargo build

# Run unit tests (fast, comprehensive)
cargo test --lib --bins

# Run integration tests
cargo test --test integration
```

## Environment Status

### ✅ Working

- **Rust Toolchain**: Version 1.90.0 (as specified in `rust-toolchain.toml`)
- **Build System**: Cargo builds successfully with all dependencies
- **Unit Tests**: All 197 unit tests pass (143 in lib, 54 in bins)
- **Core Functionality**: Binary builds and runs correctly
- **Git Integration**: Git is available and working
- **Bash Shell**: Available for shell integration tests

### ⚠️ Partial / Limited

- **Integration Tests**: 333 of 360 tests pass
  - 25 tests fail due to missing shells (zsh, fish)
  - 2 tests are ignored

- **Shell Support**:
  - ✅ bash - fully available
  - ❌ zsh - not installed
  - ❌ fish - not installed

### Test Failure Breakdown

All 25 failing tests are due to missing shells (zsh, fish):

1. **Parameterized shell tests**: Tests using `rstest` to run across bash/zsh/fish
   - `case_2` failures: zsh not installed
   - `case_3` failures: fish not installed
   - Affected test suites: `shell_wrapper`, `e2e_shell`, `e2e_shell_post_start`

2. **Fish-specific tests**: 4 tests that only run on fish
   - `test_fish_*` - Fish shell integration tests

3. **Expected behavior**: These failures are normal for this environment
   - The codebase is designed to support multiple shells
   - All bash tests pass successfully
   - CI/CD runs these tests in environments with all shells installed

## Installing Additional Shells (Optional)

To run all integration tests, you can install the missing shells:

```bash
# Install zsh
apt-get update && apt-get install -y zsh

# Install fish
apt-get update && apt-get install -y fish
```

After installing shells, re-run tests:
```bash
cargo test --test integration
```

## Updating Snapshots

Some tests use `insta` for snapshot testing. If snapshots need updating:

```bash
# Review and update snapshots interactively
cargo insta review

# Or accept all changes (use with caution)
cargo insta accept
```

## Development Workflow

### Building

```bash
# Debug build (fast, includes debug symbols)
cargo build

# Release build (optimized, slower to build)
cargo build --release

# Build without syntax highlighting (avoids C compilation)
cargo build --no-default-features
```

### Testing

```bash
# Run all unit tests (fast)
cargo test --lib --bins

# Run specific integration test
cargo test --test integration integration_tests::list

# Run tests with output
cargo test --test integration -- --nocapture

# Skip long-running benchmarks
cargo test --test integration --skip bench_list_real_repo
```

### Running Worktrunk

```bash
# Via cargo (rebuilds if needed)
cargo run -- --help
cargo run -- list

# Direct binary (faster for repeated runs)
./target/debug/wt --help
./target/debug/wt list
```

## Project Structure

```
worktrunk/
├── src/
│   ├── lib.rs              # Library crate
│   ├── main.rs             # Binary entry point
│   ├── commands/           # CLI commands
│   ├── config/             # Configuration handling
│   ├── git/                # Git operations
│   ├── output/             # Output system (interactive & directive modes)
│   └── styling/            # Terminal styling & ANSI codes
├── tests/
│   ├── common/             # Test utilities
│   └── integration_tests/  # Integration test suites
├── benches/                # Performance benchmarks
└── templates/              # Jinja2 templates for LLM prompts
```

## Key Files

- **`CLAUDE.md`**: Comprehensive development guidelines (code quality, output formatting, etc.)
- **`Cargo.toml`**: Project manifest and dependencies
- **`rust-toolchain.toml`**: Rust version specification
- **`setup-environment.sh`**: Environment verification script (this automated setup)

## Common Issues

### Issue: Tests fail with "command not found: fish"

**Solution**: Install fish shell or run only bash tests:
```bash
cargo test --test integration -- --skip fish
```

### Issue: Snapshot test failures

**Solution**: Review and update snapshots:
```bash
cargo insta review
```

### Issue: Build fails with tree-sitter errors

**Solution**: Disable syntax highlighting feature:
```bash
cargo build --no-default-features
```

## Development Guidelines

See `CLAUDE.md` for comprehensive guidelines including:

- Output formatting standards
- CLI design principles
- Testing requirements
- Git commit practices
- Code quality standards

## Next Steps for Agents

1. **To work on features**: All core functionality works, proceed with normal development
2. **To run full test suite**: Install zsh and fish shells first
3. **To update snapshots**: Use `cargo insta review` after making output changes
4. **To understand codebase**: Read `CLAUDE.md` for project conventions

## Environment Verification

Run the setup script anytime to verify the environment:

```bash
./setup-environment.sh
```

Expected output:
- ✓ Core environment is ready
- ✓ Rust toolchain: 1.90.0
- ✓ Build: OK
- ✓ Unit tests: OK
- ⚠️ Integration tests: 333 passed, 25 failed, 2 ignored

---

**Last Updated**: 2025-11-20
**Environment**: Claude Code web
**Rust Version**: 1.90.0
