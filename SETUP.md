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

### ✅ Fully Working

- **Rust Toolchain**: Version 1.90.0 (as specified in `rust-toolchain.toml`)
- **Build System**: Cargo builds successfully with all dependencies
- **Unit Tests**: All 197 unit tests pass (143 in lib, 54 in bins)
- **Integration Tests**: All 358 tests pass (2 tests ignored)
- **Core Functionality**: Binary builds and runs correctly
- **Git Integration**: Git 2.43.0 available and working
- **Shell Support**: All shells available
  - ✅ bash 5.2.15
  - ✅ zsh 5.9
  - ✅ fish 3.7.0

### Test Results

- **Total Tests**: 360
- **Passing**: 358 (99.4%)
- **Ignored**: 2 (0.6%)
- **Failing**: 0

All test suites pass successfully:
- ✅ Unit tests (lib + bins)
- ✅ Integration tests (all shells)
- ✅ Shell wrapper tests (bash, zsh, fish)
- ✅ PTY tests
- ✅ Snapshot tests

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

### Issue: Snapshot test failures

If output format changes:
```bash
cargo insta review
```

### Issue: Build fails with tree-sitter errors

Disable syntax highlighting feature (optional):
```bash
cargo build --no-default-features
```

### Issue: Missing shells

The setup script automatically installs zsh and fish on Debian/Ubuntu systems. If you're on a different system or if auto-install fails:

```bash
# macOS
brew install zsh fish

# Debian/Ubuntu
apt-get install -y zsh fish

# Fedora/RHEL
dnf install -y zsh fish
```

## Development Guidelines

See `CLAUDE.md` for comprehensive guidelines including:

- Output formatting standards
- CLI design principles
- Testing requirements
- Git commit practices
- Code quality standards

## Next Steps for Agents

1. **Environment is fully ready**: All tests pass, all shells installed
2. **Start developing**: Proceed with any features or bug fixes
3. **Update snapshots**: Use `cargo insta review` after making output changes
4. **Follow conventions**: Read `CLAUDE.md` for project-specific guidelines

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
- ✓ All shells available: bash, zsh, fish
- ✓ Integration tests: 358 passed, 0 failed, 2 ignored

---

**Last Updated**: 2025-11-20
**Environment**: Claude Code web
**Rust Version**: 1.90.0
