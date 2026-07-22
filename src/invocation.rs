//! Utilities for understanding how the binary was invoked.
//!
//! These functions examine `argv[0]` and environment variables to determine:
//! - What name the binary was invoked as (`binary_name`)
//! - Whether we're running as a git subcommand (`is_git_subcommand`)
//! - Whether shell integration can work (`was_invoked_with_explicit_path`)

/// Get the binary name from `argv[0]`, falling back to "wt".
///
/// Used as the default for `--cmd` in shell integration commands.
/// When invoked as `git-wt`, returns "git-wt"; when invoked as `wt`, returns "wt".
/// On Windows, strips `.exe` extension — users should use `wt` not `wt.exe` in aliases.
pub fn binary_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_stem()
                .and_then(|name| name.to_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "wt".to_string())
}

/// Check if we're running as a git subcommand (e.g., `git wt` instead of `git-wt`).
///
/// When git runs a subcommand like `git wt`, it sets `GIT_EXEC_PATH` in the environment.
/// This is NOT set when running `git-wt` directly or via a shell function.
///
/// This distinction matters for shell integration: `git wt` runs as a subprocess of git,
/// so even with shell integration configured, the `cd` directive cannot propagate to
/// the parent shell. Users must use `git-wt` directly (via shell function) for automatic cd.
pub fn is_git_subcommand() -> bool {
    std::env::var_os("GIT_EXEC_PATH").is_some()
}

/// Get the `argv[0]` value (how we were invoked), with forward-slash separators.
///
/// Used in error messages to show what command was actually run.
/// Returns the full invocation path (e.g., `target/debug/wt`, `./wt`, `wt`).
/// Backslashes are normalized to forward slashes on Windows for consistent display.
pub fn invocation_path() -> String {
    std::env::args_os()
        .next()
        .map(|arg0| normalize_invocation_path(&arg0))
        .unwrap_or_else(|| "wt".to_string())
}

/// Render an `argv[0]` value as a display string with forward-slash separators.
///
/// Uses [`path_slash`] rather than an unconditional `\` → `/` replacement so the
/// conversion is platform-aware: on Windows (where `\` is a path separator) the
/// separators become `/`, while a literal `\` in a Unix path — a valid filename
/// character there, not a separator — is left untouched.
fn normalize_invocation_path(arg0: &std::ffi::OsStr) -> String {
    use path_slash::PathExt as _;
    std::path::Path::new(arg0).to_slash_lossy().into_owned()
}

/// Check if we were invoked via an explicit path rather than PATH lookup.
///
/// # Purpose
///
/// When shell integration is configured (e.g., `eval "$(wt config shell init)"`),
/// the shell wrapper function intercepts calls to `wt` and handles directory
/// changes. However, this only works when the shell finds `wt` via PATH lookup.
///
/// If the user runs a specific binary path (like `cargo run` or `./target/debug/wt`),
/// the shell wrapper won't intercept it, and shell integration won't work.
///
/// # Heuristic
///
/// Returns `true` if argv\[0\] contains a path separator (`/` or `\`).
///
/// - PATH lookup: shell sets argv\[0\] to just the command name (`wt`)
/// - Explicit path: argv\[0\] contains the path (`./wt`, `target/debug/wt`, `/usr/bin/wt`)
///
/// # Examples
///
/// | Invocation                  | argv\[0\]            | Returns | Reason                    |
/// |-----------------------------|----------------------|---------|---------------------------|
/// | `wt switch foo`             | `wt`                 | `false` | PATH lookup, wrapper works|
/// | `cargo run -- switch foo`   | `target/debug/wt`    | `true`  | Explicit path, no wrapper |
/// | `./target/debug/wt switch`  | `./target/debug/wt`  | `true`  | Explicit path, no wrapper |
/// | `/usr/local/bin/wt switch`  | `/usr/local/bin/wt`  | `true`  | Explicit path, no wrapper |
///
/// # Edge Cases
///
/// - **False positive**: User types full path to installed binary (`/usr/local/bin/wt`).
///   Harmless — if they're typing the full path, shell wrapper wouldn't intercept anyway.
///
/// - **Aliases**: `alias wt='...'` — shell expands alias before setting argv\[0\], so:
///   - `alias wt='wt'` → argv\[0\] = `wt` → `false` (correct)
///   - `alias wt='./target/debug/wt'` → argv\[0\] = `./target/debug/wt` → `true` (correct)
///
/// - **Symlinks**: If `~/bin/wt` is a symlink to `target/debug/wt`, argv\[0\] = `~/bin/wt`
///   (contains `/`) → `true`. This is correct — the shell wrapper wraps PATH's `wt`,
///   not the symlink.
///
/// - **`git wt` subcommand**: When invoked as `git wt`, git dispatches to `git-wt` binary
///   and sets argv\[0\] = `git-wt` (no path separator) → returns `false`. However, shell
///   integration configured for `wt` won't intercept `git wt` — they're different commands.
///   This is handled separately by `Shell::is_shell_configured()` which checks for the
///   actual binary name (`git-wt`), not `wt`.
///
/// # Why Not Other Approaches?
///
/// - **`current_exe()` + check for `/target/debug/`**: Only catches cargo builds,
///   misses other "ran specific path" scenarios.
///
/// - **Compare with `which wt`**: More accurate but requires subprocess overhead
///   and `which` behavior varies across shells.
///
/// - **Check if `current_exe()` is in PATH**: Complex PATH parsing, platform differences.
///
/// The argv\[0\] heuristic is simple, fast, and catches all cases where shell
/// integration won't work because the shell wrapper wasn't invoked.
pub fn was_invoked_with_explicit_path() -> bool {
    std::env::args_os()
        .next()
        .map(|arg0| {
            let arg0 = arg0.to_string_lossy();
            arg0.contains('/') || arg0.contains('\\')
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn normalize_invocation_path_passes_through_forward_slash_paths() {
        // Real `argv[0]` values on Unix carry no backslashes, so normalization
        // is a pass-through and must not alter them.
        assert_eq!(normalize_invocation_path(OsStr::new("wt")), "wt");
        assert_eq!(normalize_invocation_path(OsStr::new("./wt")), "./wt");
        assert_eq!(
            normalize_invocation_path(OsStr::new("target/debug/wt")),
            "target/debug/wt"
        );
        assert_eq!(
            normalize_invocation_path(OsStr::new("/usr/local/bin/wt")),
            "/usr/local/bin/wt"
        );
    }

    #[cfg(windows)]
    #[test]
    fn normalize_invocation_path_normalizes_backslashes_on_windows() {
        // On Windows `\` is a path separator, so it renders as `/`.
        assert_eq!(
            normalize_invocation_path(OsStr::new(r"target\debug\wt.exe")),
            "target/debug/wt.exe"
        );
        assert_eq!(
            normalize_invocation_path(OsStr::new(r".\wt.exe")),
            "./wt.exe"
        );
    }
}
