# Changelog

## 0.8.3

### Improved

- **Hook execution path**: Shows the execution path when post-merge hooks run in a different directory than where the user invoked the command (e.g., with `--no-remove`).
- **TTY check for `wt select`**: Now fails gracefully when run in a non-interactive terminal instead of hanging.
- **Background hooks**: `post-start` and `post-switch` hooks spawn in background via stdin piping, matching their normal behavior during `wt switch`.
- **Occupied path error message**: When a worktree path is occupied by a different branch, the error now explains the situation clearly and suggests `git switch`.
- **Shell integration hint**: Shows a hint to restart the shell when shell integration is configured but not active.
- **Message style**: Removed 2nd person pronouns ("you/your") from user-facing messages following CLI guidelines.

### Fixed

- **`wt hook post-start` blocking**: Fixed bug where `wt hook post-start` ran in foreground blocking the command, instead of spawning in background like during normal `wt switch --create`.
- **Approval bypass with `project:` prefix**: Fixed security issue where using `project:` filter prefix (e.g., `wt hook pre-merge project:`) bypassed the approval check, allowing unapproved project commands to run.

### Documentation

- **License file**: Added combined MIT and Apache-2.0 license file.
- **Demo GIFs**: Added demo GIFs to command pages on the documentation site.
- **Install instructions**: Simplified to single-line commands.

### Internal

- **Pre-commit hooks**: Updated to immutable tags.
- **Lychee exclusions**: Cleaned up link checker configuration.

## 0.8.2

### Improved

- **Concurrent hook execution**: `wt hook post-start` and `wt hook post-switch` now run all commands concurrently (matching their normal background behavior) instead of sequentially with fail-fast. Multiple failures are collected and reported together.

### Documentation

- **Nested bare repo layout**: Added worktree-path template example for nested bare repo layout (`project/.git` pattern). Uses relative paths like `../{{ branch | sanitize }}` to create worktrees as siblings to the .git directory.

## 0.8.1

### Improved

- **Shell and PowerShell installers**: Added one-line install commands for Linux/macOS and Windows.
- **Consistent terminology**: CLI now uses "branch name" consistently instead of mixing "worktree" and "branch". The `wt remove` argument is renamed from `worktrees` to `branches` to reflect that worktrees are addressed by branch name.

### Fixed

- **Switch hints**: Removed incorrect `wt switch @` hint and improved error output spacing.

### Documentation

- **Dev server and database patterns**: Added practical examples for running per-worktree dev servers with subdomain routing and databases with unique ports.

## 0.8.0

### Improved

- **Separate `--yes` and `--force` flags**: `--force/-f` renamed to `--yes/-y` for skipping prompts (all commands). New `--force/-f` on `wt remove` forces removal of worktrees with untracked files (build artifacts, node_modules, etc.). (Breaking: `--force` no longer skips prompts; use `--yes`)
- **Clearer branch deletion output**: `wt remove` output now shows "worktree & branch" when the branch is deleted, or plain "worktree" with a hint when kept. Makes scanning output for branch fate easier.
- **`post-switch` hook on remove**: When `wt remove` switches to the main worktree, post-switch hooks now run in the destination.
- **Allow merge commits by default**: `wt step push` no longer rejects history with merge commits. Removed `--allow-merge-commits` flag. (Breaking: flag removed)

### Fixed

- **Orphan branches in `wt list`**: Branches with no common ancestor with the default branch no longer cause errors.
- **Remote branch filtering**: `wt list --remotes` now filters out branches that are tracked as upstreams, not just branches with worktrees.
- **Error message spacing**: Reduced double-newline spacing in error messages.

## 0.7.0

### Improved

- **Working tree conflict detection**: `wt list --full` now detects conflicts using uncommitted working tree changes, not just committed content. This catches conflicts earlier—before committing changes that would conflict with the target branch.
- **Dev server URL column**: New optional URL column in `wt list` configured via `[list] url` template in project config (`.config/wt.toml`). URLs show with health-check styling: normal if the port is listening, dimmed otherwise.
- **Shell integration simplification**: The shell wrapper is now self-contained with all directive handling inlined. Removes the separate helper function that could become unavailable if shell initialization order changed.
- **Performance**: Repository caching reduces git subprocess spawns; parallelized pre-skeleton operations for faster initial display.
- **Improved error hints**: When a worktree path already exists during creation, the error hint now correctly suggests `--create --clobber`.

### Fixed

- **Docs syntax highlighting**: Fixed syntax highlighting colors being stripped by 1Password browser extension on the documentation site.

## 0.6.1

### Improved

- **`post-switch` hook**: New hook that runs in the background after every `wt switch` operation. Unlike `post-start` (which only runs on creation), `post-switch` runs on all switch results. Use cases include renaming terminal tabs, updating tmux window names, and IDE notifications.
- **Signal forwarding for hooks**: Hooks now receive SIGINT/SIGTERM when the parent process is interrupted, allowing proper cleanup. Previously, non-interactive shells continued executing after signals.
- **Faster `wt list` skeleton**: Time-to-skeleton reduced by caching default branch lookup, batching timestamp fetching, and deferring non-essential git operations. Skeleton shows `·` placeholder for gutter symbols until data loads.
- **Clearer `--clobber` hint**: Error message now says "to overwrite (with backup)" instead of "to retry with backup".

### Documentation

- **State side-effects**: Added section explaining how Worktrunk state operations may trigger git commands.
- **`wt merge` location**: Clarified that `wt merge` runs from the feature worktree.

## 0.6.0

### Improved

- **Single-width Unicode symbols**: Replaced emojis (🔄, ✅, ❌) with single-width Unicode symbols (◎, ✓, ✗, ▲, ↳, ○, ❯) for better terminal compatibility and consistent alignment.
- **Output system overhaul**: Clean separation of output channels (data→stdout, status→stderr, directives→file) means piping works with shell integration active. `wt list --format=json | jq` and `wt switch feature | tee log.txt` both work correctly. Background processes use `process_group(0)` instead of `nohup` for more reliable detachment.
- **Trailing arguments for `--execute`**: `wt switch --execute` now accepts arguments after `--`, enabling shell aliases like `alias wsc='wt switch --create -x claude'` then `wsc feature -- 'implement login'`.
- **`hash_port` template filter**: `{{ branch | hash_port }}` hashes the branch name to a deterministic port number (10000-19999), useful for running dev servers without port conflicts.
- **`sanitize` template filter**: `{{ branch | sanitize }}` explicitly replaces `/` and `\` with `-` for filesystem-safe paths. (Breaking: `{{ branch }}` now provides raw branch names. Update templates that use `{{ branch }}` in filesystem paths to use `{{ branch | sanitize }}` instead)
- **Log directory in state output**: `wt config state logs` and `wt config state get` now show the log directory path under a LOG FILES heading.
- **Actionable error hints**: Error messages now include hints about what command to run next.
- **Unified directory change output**: `wt remove` now shows "Switched to worktree for {branch} @ {path}" matching `wt switch` format.
- **Consistent "already up to date" formatting**: Standardized message wording and styling across commands.

### Fixed

- **`wt step rebase` with merge commits**: Fixed incorrect "Already up-to-date" when a branch has merge commits from merging target into itself.

### Documentation

- **Local CI workflow**: Added "Local CI" section to `wt merge --help` explaining how pre-merge hooks enable faster iteration.
- **Colored command reference**: Web docs now preserve ANSI colors in command reference output.
- **Clarified terminology**: Help text uses "default branch" instead of hardcoded "main".

## 0.5.2

### Improved

- **`--clobber` flag for `wt switch`**: When encountering a stale directory or file at the target worktree path, `--clobber` moves it to a timestamped `.bak` file instead of failing.
- **Relative paths in `wt list`**: Paths are now shown relative to the main worktree (`.`, `./subdir`, `../repo.feature`) instead of a computed common prefix that could degenerate to `/`.
- **Multiline error formatting**: Errors with context now show a header describing what worktrunk was trying to do, with the full error chain in a gutter block.
- **Semantic switch messaging**: Switching to an existing worktree now shows ⚪ (info) instead of ✅ (success), reflecting that nothing was created.

### Fixed

- **Symbol styling in removal messages**: Integration symbols (`_`, `⊂`) now render in their canonical dim appearance instead of inheriting the message's cyan color.
- **ConflictingChanges error formatting**: Fixed double newlines in the error message output.

## 0.5.1

### Improved

- **Integration status in removal messages**: Shows integration symbols (`_` for same commit, `⊂` for integrated) when removing worktrees, matching `wt list` display.
- **Concurrent command limiting**: Limits concurrent git processes to 32 (configurable via `WORKTRUNK_MAX_CONCURRENT_COMMANDS`), preventing resource exhaustion on repos with many branches.
- **Better error display for `wt list`**: Task errors are now collected and displayed as warnings after the table renders, instead of being silently swallowed.
- **Remove continues on partial failures**: `wt remove` continues removing other worktrees when some fail, reporting all errors at the end.
- **Bash syntax highlighting**: Shell commands in error gutters now have syntax highlighting.
- **Shell integration is command-aware**: Detection and removal works correctly when installed as `git-wt` or other names.
- **CI fetch error documentation**: Yellow warning symbol (⚠) in CI column is now documented in help text.

### Fixed

- **CI status with multiple workflows**: Fixed incorrect status when multiple workflows exist (e.g., `ci` and `publish-docs`). Now uses GitHub's check-runs API to aggregate all workflow statuses.
- **State storage unification**: Unified branch-keyed state under `worktrunk.state.<branch>.*`. Numeric branch names now work. (Existing CI cache and markers regenerate on first access)

### Internal

- **Environment variable prefix**: Standardized to `WORKTRUNK_` prefix (e.g., `WORKTRUNK_MAX_CONCURRENT_COMMANDS`).
- Automatic winget package publishing on releases.

## 0.5.0

### Improved

- **Path column hidden when redundant**: Path column is deprioritized when all paths match the naming template, showing only at wider terminal widths (~125+ columns).
- **Better error formatting**: Errors with context now show a header with the root cause in a gutter block, improving readability for git errors.
- **Clearer integration target**: Separated `default_branch` (for stats like ahead/behind) from `target` (for integration checks), catching branches merged remotely before pulling.

### Fixed

- **Untracked files block integration**: Untracked files now prevent a worktree from being flagged as integrated, avoiding accidental data loss on removal.
- **Dirty worktree count includes untracked**: Summary now correctly counts worktrees with untracked files as dirty.
- **Branch name disambiguation**: Fixed `refname:short` issues when a branch and remote have the same name.
- **JSON output uses kebab-case**: Enum values changed from snake_case to kebab-case (e.g., `same_commit` → `same-commit`). (Breaking: scripts parsing JSON output may need updates)
- **Legacy marker format removed**: Plain-text markers no longer parsed. (Breaking: re-set markers with `wt config state marker set`)

### Internal

- **Unified command execution**: All external commands now go through `shell_exec::run()` for consistent logging and tracing.

## 0.4.0

### Added

- **`--no-rebase` flag for `wt merge`**: Fails early with a clear error if the branch is not already rebased onto target, rather than auto-rebasing. Useful for workflows that handle rebasing separately. ([#194](https://github.com/max-sixty/worktrunk/pull/194))

### Changed

- **Branch-first argument resolution**: `wt switch` and `wt remove` now check if the branch has a worktree anywhere before checking the expected path. If you type `wt switch foo`, you get branch foo's worktree, not whatever happens to be at the expected path. ([#197](https://github.com/max-sixty/worktrunk/pull/197))

### Fixed

- **`--no-commit` incorrectly skipped rebasing**: `wt merge --no-commit` now correctly rebases before stopping (if needed), rather than skipping the rebase entirely. ([#194](https://github.com/max-sixty/worktrunk/pull/194))
- **Pager for `wt config show --full`**: The pager now works correctly with the `--full` flag, showing diagnostics properly. ([#198](https://github.com/max-sixty/worktrunk/pull/198))
- **Statusline stdin handling**: Fixed flaky behavior on Windows CI by using standard is_terminal() check instead of timeout-based approach. ([#210](https://github.com/max-sixty/worktrunk/pull/210))

### Improved

- **Path-occupied error messages**: When `wt switch` can't create a worktree because the path exists, error messages now show which branch occupies the path and provide actionable commands to fix the situation. ([#195](https://github.com/max-sixty/worktrunk/pull/195), [#206](https://github.com/max-sixty/worktrunk/pull/206), [#207](https://github.com/max-sixty/worktrunk/pull/207))
- **Switch mismatch detection**: Better error messages when path/branch mismatches occur, with hints showing the expected path. ([#195](https://github.com/max-sixty/worktrunk/pull/195))

## 0.3.1

### Fixed

- **Branch names with slashes**: Branch names like `fix/feature-name` no longer break git config markers. Slashes are now escaped for git config compatibility. ([#189](https://github.com/max-sixty/worktrunk/pull/189), thanks @kyleacmooney)
- **stdin inheritance for `--execute`**: Interactive programs (vim, python -i, claude) now work correctly with `--execute` on non-Unix platforms. ([#191](https://github.com/max-sixty/worktrunk/pull/191))
- **Filenames with spaces/newlines**: Git status parsing now handles filenames containing spaces and newlines correctly using NUL-separated output.
- **Concurrent approval race condition**: Multiple concurrent approval/revocation operations no longer overwrite each other. Approvals now reload from disk before saving.
- **Dirty worktrees incorrectly marked integrated**: Priority 5 integration check now requires clean working tree state, preventing worktrees with uncommitted changes from being flagged as safe to remove.
- **Type changes not detected as staged**: Index status check now recognizes file type changes (`T` status) as staged changes.
- **User hook failure strategy**: Hook failure strategy now correctly applies to user hooks instead of always using fail-fast.
- **Branch variable in detached HEAD**: `{{ branch }}` now correctly expands to "HEAD" in detached HEAD worktrees instead of "(detached)".

### Improved

- **Self-hosted GitLab support**: CI auth checks now detect the GitLab host from the remote URL, supporting self-hosted GitLab instances instead of always checking gitlab.com.
- **Platform-specific CI status**: `wt list --full` and `wt config show` now show only the relevant CI tool (GitHub Actions or GitLab CI) based on the repository's remote URL.
- **LLM error reproduction**: When LLM commands fail, error messages now show the full reproduction command (e.g., `wt step commit --show-prompt | llm`) for easier debugging.
- **Location format**: Messages now use `@` instead of `at` for location phrases (e.g., "Switched to feature @ /path").
- **Switch help text**: Clarified that `wt switch` creates worktrees automatically for existing branches, not just for new branches with `--create`.

## 0.3.0

### Added

- **`--show-prompt` flag for LLM commands**: `wt step commit --show-prompt` and `wt step squash --show-prompt` output the rendered LLM prompt without executing the command. Useful for debugging templates or manually piping to LLM tools. ([#187](https://github.com/max-sixty/worktrunk/pull/187))
- **Diff size limits and diffstat for LLM prompts**: Large diffs (>400K chars) are progressively filtered—first removing lock files, then truncating to 50 lines/file, max 50 files. New `git_diff_stat` template variable shows line change statistics. ([#186](https://github.com/max-sixty/worktrunk/pull/186))
- **`MainState::Empty` status**: New `_` symbol for clean same-commit branches (safe to delete), distinguished from `–` (en-dash) for same-commit branches with uncommitted changes. Previously, both showed `_`. Only Empty branches are dimmed and considered "potentially removable". ([#185](https://github.com/max-sixty/worktrunk/pull/185))

### Changed

- **State subcommands default to `get`**: Running `wt config state default-branch` now defaults to `get`, making the command shorter. Use explicit `get` subcommand to access options like `--refresh` or `--branch`. ([#184](https://github.com/max-sixty/worktrunk/pull/184))
- **Clearer integration reason messages**: Updated descriptions to be more precise—"same commit as" instead of "already in" for SameCommit, "ancestor of" for Ancestor, "no added changes" for NoAddedChanges, "tree matches" for TreesMatch.

## 0.2.1

### Changed

- **Unified state management**: `wt config var` and `wt config cache` replaced by `wt config state` with consistent get/set/clear semantics for all runtime state. New subcommands: `default-branch`, `ci-status`, `marker`, `logs`, `show`. ([#178](https://github.com/max-sixty/worktrunk/pull/178))
- **Comprehensive state overview**: `wt config state show` displays all state (default branch, switch history, markers, CI cache, logs) with `--format=json` support. ([#180](https://github.com/max-sixty/worktrunk/pull/180))

### Added

- **`git-wt` binary for Windows**: New `git-wt` binary avoids conflict with Windows Terminal's `wt` command. Build with `--features git-wt`. Shell init/install now accept `--cmd` to specify which binary name to use. ([#177](https://github.com/max-sixty/worktrunk/pull/177))
- **Diffstat in select preview**: The log preview (Tab 2) in `wt select` now shows line change statistics (+N -M) matching `wt list`'s HEAD± column format. ([#179](https://github.com/max-sixty/worktrunk/pull/179))

### Fixed

- **Windows compatibility**: Multiple test and runtime fixes for Windows including stdin timeout handling, path canonicalization, and cross-platform test behavior. ([#167](https://github.com/max-sixty/worktrunk/pull/167), [#168](https://github.com/max-sixty/worktrunk/pull/168), [#169](https://github.com/max-sixty/worktrunk/pull/169), [#170](https://github.com/max-sixty/worktrunk/pull/170), [#171](https://github.com/max-sixty/worktrunk/pull/171), [#174](https://github.com/max-sixty/worktrunk/pull/174), [#176](https://github.com/max-sixty/worktrunk/pull/176))

## 0.1.21

### Fixed

- **Windows path handling in shell templates**: Fixed path quoting in hook templates on Windows by using `cygpath` to convert native Windows paths to POSIX format for Git Bash compatibility. Template variables like `{{ worktree }}` and `{{ repo_root }}` now work correctly. ([#161](https://github.com/max-sixty/worktrunk/pull/161))
- **Hook errors show `--no-verify` hint**: When hooks fail during `wt merge`, `wt commit`, or `wt squash`, the error message now includes a hint about using `--no-verify` to skip hooks. ([4a89748](https://github.com/max-sixty/worktrunk/commit/4a89748f))

## 0.1.20

### Changed

- **`--doctor` renamed to `--full`**: The `wt list --doctor` flag is now `wt list --full`. The new name better reflects that it shows extended information (binaries status, full diff stats). ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))
- **CLI binaries status in `wt config show --full`**: Shows installation and authentication status of `gh` and `glab` CLI tools in a new BINARIES section. ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))
- **CI tool hints**: `wt list --full` shows a hint when CI status is unavailable, with specific guidance on which CLI tool to install or authenticate. ([171952e](https://github.com/max-sixty/worktrunk/commit/171952ec))

### Fixed

- **GitHub StatusContext checks**: CI status now includes StatusContext checks (used by some CI systems like Jenkins, CircleCI, and external status checks) in addition to CheckRuns. ([690da88](https://github.com/max-sixty/worktrunk/commit/690da889))
- **Windows Git Bash detection with WSL**: Fixed detection of Git Bash when WSL is installed. Previously, the WSL bash shim in PATH could be found instead of Git Bash, causing hook execution failures. ([b48b0ba](https://github.com/max-sixty/worktrunk/commit/b48b0ba7))

## 0.1.19

### Added

- **`wt step for-each` command**: Run commands across all worktrees sequentially. Supports template variables (`{{ branch }}`, `{{ worktree }}`, etc.) and JSON context on stdin. Example: `wt step for-each -- git pull --autostash`. ([#138](https://github.com/max-sixty/worktrunk/pull/138))

### Changed

- **Content integration detection always enabled**: The `⊂` (content integrated) symbol now appears without requiring `--full`. Squash-merged branches are detected automatically. ([f39c442](https://github.com/max-sixty/worktrunk/commit/f39c4428))
- **SIGINT forwarding**: Ctrl+C now properly terminates child processes in hooks, preventing orphaned background commands. ([#136](https://github.com/max-sixty/worktrunk/pull/136))

### Fixed

- **Windows path handling**: Fixed path canonicalization issues on Windows that caused worktree detection failures. Uses `dunce` to handle Windows verbatim paths (`\\?\`) that git cannot process. ([#125](https://github.com/max-sixty/worktrunk/pull/125))

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
