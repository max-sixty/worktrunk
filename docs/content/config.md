+++
title = "wt config"
weight = 15

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit src/cli.rs to update -->

Manages configuration, shell integration, and runtime settings. The command provides subcommands for setup, inspection, and cache management.

## Examples

Install shell integration (required for directory switching):

```bash
wt config shell install
```

Create user config file with documented examples:

```bash
wt config create
```

Show current configuration and file locations:

```bash
wt config show
```

## Shell integration

Shell integration allows Worktrunk to change the shell's working directory after `wt switch`. Without it, commands run in a subprocess and directory changes don't persist.

The `wt config shell install` command adds integration to the shell's config file. Manual installation:

```bash
# For bash: add to ~/.bashrc
eval "$(wt config shell init bash)"

# For zsh: add to ~/.zshrc
eval "$(wt config shell init zsh)"

# For fish: add to ~/.config/fish/config.fish
wt config shell init fish | source
```

## Configuration files

**User config** — `~/.config/worktrunk/config.toml` (or `$WORKTRUNK_CONFIG_PATH`):

Personal settings like LLM commit generation, path templates, and default behaviors. The `wt config create` command generates a file with documented examples.

**Project config** — `.config/wt.toml` in repository root:

Project-specific hooks: post-create, post-start, pre-commit, pre-merge, post-merge. See [Hooks](@/hooks.md) for details.

## LLM commit messages

Worktrunk can generate commit messages using an LLM. Enable in user config:

```toml
[commit-generation]
command = "llm"
```

See [LLM Commits](@/llm-commits.md) for installation, provider setup, and customization.

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit cli.rs to update -->

```
wt config - Manage configuration and shell integration
Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create user configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  var        Get or set runtime variables (stored in git config)
  approvals  Manage command approvals

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```
