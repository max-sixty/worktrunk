# Scripts

This directory contains automation scripts for the worktrunk project.

## Claude Code Web Setup

**`setup-claude-code-web.sh`** - Environment setup for Claude Code web sessions

This script prepares a fresh Claude Code web environment with everything needed to develop and test worktrunk:

- Verifies Rust toolchain (1.90.0)
- Builds the project
- Runs unit tests
- Installs required shells (zsh, fish) on Debian/Ubuntu systems
- Runs full integration test suite
- Reports comprehensive environment status

### Usage

```bash
./scripts/setup-claude-code-web.sh
```

### Expected Output

When successful, you'll see:
- ✓ Rust toolchain: 1.90.0
- ✓ Build: OK
- ✓ Unit tests: OK (197 tests)
- ✓ All shells available: bash, zsh, fish
- ✓ Integration tests: 358 passed, 0 failed, 2 ignored

### Requirements

- Linux environment (tested on Debian/Ubuntu)
- Root access (for installing shells via apt-get)
- Internet connection (for downloading packages)

### What Gets Installed

The script will automatically install:
- **zsh** - Z shell (if not present)
- **fish** - Friendly interactive shell (if not present)

These shells are required for the full integration test suite which tests shell-specific functionality across bash, zsh, and fish.
