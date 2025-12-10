# Changelog

## 0.1.18

### Added

- **Windows support**: Git Bash with PowerShell fallback enables worktrunk on Windows. Git Bash is preferred (same bash hook syntax across platforms); PowerShell works for basic commands with limitations. ([#122](https://github.com/max-sixty/worktrunk/pull/122))
- **Winget publishing**: Release workflow now publishes to Windows Package Manager. ([079c9df](https://github.com/max-sixty/worktrunk/commit/079c9df3))

### Changed

- **Approvals command moved**: `wt config approvals` is now `wt hook approvals` since approvals manage hook commands. ([b7b1b9e](https://github.com/max-sixty/worktrunk/commit/b7b1b9e3))
- **Approval prompts show templates**: Approval prompts now display command templates (what gets saved) rather than expanded values. ([2315d26](https://github.com/max-sixty/worktrunk/commit/2315d268))
- **Preview mode renamed**: The `history` preview mode is now `log` for clarity. ([0461152](https://github.com/max-sixty/worktrunk/commit/04611524))

### Fixed

- **PR/MR source filtering**: Filter PRs by source repository instead of author, fixing false matches when multiple users have PRs with the same branch name. ([e9ccdf7](https://github.com/max-sixty/worktrunk/commit/e9ccdf77))

## 0.1.17

### Added

- **User-level hooks**: Define hooks in `~/.config/wt.toml` that run for all repositories. New `wt hook show` command displays configured hooks and their sources. ([#118](https://github.com/max-sixty/worktrunk/pull/118))
- **SSH URL support**: Git SSH URLs (e.g., `git@github.com:user/repo.git`) now work correctly for remote operations and branch name escaping. ([92c2cef](https://github.com/max-sixty/worktrunk/commit/92c2cef8))
- **Help text wrapping**: CLI help text now wraps to terminal width for better readability. ([fe981c2](https://github.com/max-sixty/worktrunk/commit/fe981c2e))

### Changed

- **JSON output redesign**: `wt list --format=json` now outputs a query-friendly format. This is a breaking change for existing JSON consumers. ([236eae8](https://github.com/max-sixty/worktrunk/commit/236eae81))
- **Status symbols**: Reorganized status column symbols for better scannability. Same-commit now distinguished from ancestor in integration detection. ([5053af8](https://github.com/max-sixty/worktrunk/commit/5053af88), [a087962](https://github.com/max-sixty/worktrunk/commit/a0879623))

### Fixed

- **ANSI state reset**: Reset terminal ANSI state before returning to shell, preventing color bleeding into subsequent commands. ([334f6d9](https://github.com/max-sixty/worktrunk/commit/334f6d99))
- **Empty staging error**: Fail early with a clear error when trying to generate a commit message with nothing staged. ([b9522bc](https://github.com/max-sixty/worktrunk/commit/b9522bc6))

## 0.1.16

### Added

- **Squash-merge integration detection**: Improved branch cleanup detection with four ordered checks to identify when branch content is already in the target branch. This enables accurate removal of squash-merged branches even after target advances. New status symbols: `·` for same commit, `⊂` for content integrated via different history. ([6325be2](https://github.com/max-sixty/worktrunk/commit/6325be28))
- **CI absence caching**: Cache "no CI found" results to avoid repeated API calls for branches without CI configured. Reduces unnecessary rate limit consumption. ([8db3928](https://github.com/max-sixty/worktrunk/commit/8db39285))
- **Shell completion tests**: Black-box snapshot tests for zsh, bash, and fish completions that verify actual completion output. ([#117](https://github.com/max-sixty/worktrunk/pull/117))

### Changed

- **Merge conflict indicator**: Changed from `⊘` to `⚔` (crossed swords) for better visual distinction from the rebase symbol. ([f3b96a8](https://github.com/max-sixty/worktrunk/commit/f3b96a83))

### Documentation

- **Hook JSON context**: Document all JSON fields available to hooks on stdin with examples for Python and other languages. ([af80589](https://github.com/max-sixty/worktrunk/commit/af805898))
- **CI caching**: Document that CI results are cached for 30-60 seconds and how to use `wt config cache` to manage the cache. ([4804913](https://github.com/max-sixty/worktrunk/commit/48049132))
- **Status column clarifications**: Clarify that the Status column contains multiple subcolumns with priority ordering. ([1f9bb38](https://github.com/max-sixty/worktrunk/commit/1f9bb38f))

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

- **Shell completion bypass**: Fixed lazy shell completion to use `command` builtin, bypassing the shell function that was causing `_clap_dynamic_completer_wt` errors. Thanks to [@cquiroz](https://github.com/cquiroz) in [#102](https://github.com/max-sixty/worktrunk/issues/102). ([#105](https://github.com/max-sixty/worktrunk/pull/105))
- **Remote-only branch completions**: `wt remove` completions now exclude remote-only branches (which can't be removed) and show a helpful error with hint to use `wt switch`. ([#108](https://github.com/max-sixty/worktrunk/pull/108))
- **Detached HEAD hooks**: Pre-remove hooks now work correctly on detached HEAD worktrees. ([#111](https://github.com/max-sixty/worktrunk/pull/111))
- **Hook `{{ target }}` variable**: Fixed template variable expansion in standalone hook execution. ([#106](https://github.com/max-sixty/worktrunk/pull/106))
