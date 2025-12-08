# Changelog

## Unreleased

## 0.1.15

### Added

- **`wt hook` command**: New command for running lifecycle hooks directly. Moved hook execution from `wt step` to `wt hook` for cleaner semantic separation. ([#113](https://github.com/max-sixty/worktrunk/pull/113))
- **Named hook execution**: Run specific named commands with `wt hook <type> <name>` (e.g., `wt hook pre-merge test`). Includes shell completion for hook names from project config. ([#114](https://github.com/max-sixty/worktrunk/pull/114))

### Fixed

- **Zsh completion syntax**: Fixed `_describe` syntax in zsh shell completions. ([6ae9d0f](https://github.com/max-sixty/worktrunk/commit/6ae9d0f9))
- **Fish shell wrapper**: Fixed stderr redirection in fish shell wrapper. ([0301d4b](https://github.com/max-sixty/worktrunk/commit/0301d4bf))
- **CI status for local branches**: Only check CI for branches with upstream tracking configured. ([6273ccd](https://github.com/max-sixty/worktrunk/commit/6273ccdb))
- **Git error messages**: Include executed git command in error messages for easier debugging. ([200eea4](https://github.com/max-sixty/worktrunk/commit/200eea43))

## 0.1.14

### Added

- **Pre-remove hook**: New `pre-remove` hook runs before worktree removal, enabling cleanup tasks like stopping devcontainers. Thanks to [@pwntester](https://github.com/pwntester) in [#101](https://github.com/max-sixty/worktrunk/issues/101). ([#107](https://github.com/max-sixty/worktrunk/pull/107))
- **JSON context on stdin**: Hooks now receive worktree context as JSON on stdin, enabling hooks in any language (Python, Node, Ruby, etc.) to access repo information. ([#109](https://github.com/max-sixty/worktrunk/pull/109))
- **`wt config create --project`**: New flag to generate `.config/wt.toml` project config files directly. ([#110](https://github.com/max-sixty/worktrunk/pull/110))

### Fixed

- **Shell completion bypass**: Fixed lazy shell completion to use `command` builtin, bypassing the shell function that was causing `_clap_dynamic_completer_wt` errors. Thanks to [@jimmycuadra](https://github.com/jimmycuadra) in [#102](https://github.com/max-sixty/worktrunk/issues/102). ([#105](https://github.com/max-sixty/worktrunk/pull/105))
- **Remote-only branch completions**: `wt remove` completions now exclude remote-only branches (which can't be removed) and show a helpful error with hint to use `wt switch`. ([#108](https://github.com/max-sixty/worktrunk/pull/108))
- **Detached HEAD hooks**: Pre-remove hooks now work correctly on detached HEAD worktrees. ([#111](https://github.com/max-sixty/worktrunk/pull/111))
- **Hook `{{ target }}` variable**: Fixed template variable expansion in standalone hook execution. ([#106](https://github.com/max-sixty/worktrunk/pull/106))
