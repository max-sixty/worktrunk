//! Global output context with file-based directive passing
//!
//! Shell integration directives (cd, exec) travel from wt to the parent shell
//! through files named by environment variables. This module chooses the right
//! file for each directive and escalates a warning when a trusted directive is
//! refused. Regular output still uses `eprintln!`/`println!` directly (from
//! `worktrunk::styling` for color support).
//!
//! # Protocol
//!
//! Shell integration uses two separate files with different trust levels:
//!
//! - `WORKTRUNK_DIRECTIVE_CD_FILE` holds a single raw path. The wrapper runs
//!   `cd -- "$(< file)"` after wt exits. There is no shell parsing, no
//!   escaping, and no injection surface, so the env var is safe to pass
//!   through to alias/hook shell bodies — a body that appends to it can at
//!   worst redirect `cd`.
//!
//! - `WORKTRUNK_DIRECTIVE_EXEC_FILE` holds arbitrary shell that the wrapper
//!   sources after the `cd`. This is how `wt switch --execute <cmd>` runs its
//!   payload in the user's interactive shell, inheriting functions and env.
//!   Because the contents are sourced verbatim, wt scrubs this env var from
//!   project-alias and foreground-hook child processes so a body authored in
//!   shared config cannot inject shell into the parent session.
//!
//! # Conservative scrub
//!
//! User-source aliases pass the EXEC env var through (issue #2101): the body
//! lives in the user's own config, so `wt step my-alias` running
//! `wt switch main --execute claude` is no different from the user typing
//! the same command at the top level.
//!
//! Project aliases and foreground hooks keep the scrub. A nested `wt
//! --execute …` inside one of those bodies sees no EXEC file and refuses to
//! run the payload, emitting a warning and a link to
//! <https://github.com/max-sixty/worktrunk/issues/2101> so users can report
//! whether to relax the restriction further.
//!
//! The retired single-file protocol is never written. If an old wrapper still
//! sets only `WORKTRUNK_DIRECTIVE_FILE`, switch output diagnoses the outdated
//! wrapper and tells the user to reinstall shell integration.

use std::fs::OpenOptions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context as _;
use color_print::cformat;
use std::sync::{Mutex, OnceLock};

#[cfg(not(unix))]
use worktrunk::git::WorktrunkError;
#[cfg(not(unix))]
use worktrunk::shell_exec::Cmd;
#[cfg(unix)]
use worktrunk::shell_exec::ShellConfig;
// The std-`Command` scrub helper is only needed by the unix `exec` variant of
// `execute_command`; the non-unix variant scrubs via `Cmd`'s builder method.
#[cfg(unix)]
use worktrunk::shell_exec::scrub_git_discovery_env_vars;
use worktrunk::shell_exec::{
    DIRECTIVE_CD_FILE_ENV_VAR, DIRECTIVE_EXEC_FILE_ENV_VAR, RETIRED_DIRECTIVE_FILE_ENV_VAR,
};
use worktrunk::styling::{eprintln, hint_message, stderr, warning_message};

/// Issue tracking whether to relax the EXEC scrub further for project aliases
/// and hooks. (User aliases already pass EXEC through.) Emitted in the warning
/// so users can report use cases.
pub const EXEC_SCRUB_ISSUE_URL: &str = "https://github.com/max-sixty/worktrunk/issues/2101";

// Re-export set_verbosity from the library's styling module.
// This ensures the binary and library share the same global state.
// Library code (like expansion.rs) accesses verbosity() directly from styling.
pub use worktrunk::styling::set_verbosity;

/// Global output state, lazily initialized on first access.
///
/// Uses `OnceLock<Mutex<T>>` pattern:
/// - `OnceLock` provides one-time lazy initialization (via `get_or_init()`)
/// - `Mutex` allows mutation after initialization
/// - No unsafe code required
///
/// Lock poisoning (from `.expect()`) is theoretically possible but practically
/// unreachable - the lock is only held for trivial Option assignments that cannot panic.
static OUTPUT_STATE: OnceLock<Mutex<OutputState>> = OnceLock::new();

/// Ensures output shows the retired wrapper's reinstall action at most once.
static RETIRED_REPAIR_HINT_SHOWN: OnceLock<()> = OnceLock::new();

/// Selects which directive files wt writes to based on environment.
///
/// Computed once during `state()` initialization from the process environment.
#[derive(Debug, Clone, Default)]
enum DirectiveMode {
    /// Shell integration not active. `execute()` runs commands directly;
    /// `change_directory()` is a no-op beyond updating the buffered target
    /// dir used by `execute()`.
    #[default]
    Interactive,
    /// Split protocol. `cd_file` is always a real path; `exec_file` is
    /// `None` when the EXEC var was scrubbed from this process (we're
    /// running inside an alias/hook shell body). `--execute` in the scrubbed
    /// case warns and drops the command.
    Split {
        cd_file: PathBuf,
        exec_file: Option<PathBuf>,
    },
    /// A retired pre-split wrapper is active. This state is diagnostic-only:
    /// wt never reads or writes the retired directive file, and `--execute`
    /// is refused rather than run directly.
    Retired,
}

#[derive(Default)]
struct OutputState {
    /// Which directive files wt writes to.
    mode: DirectiveMode,
    /// Buffered target directory for execute() in interactive mode
    target_dir: Option<PathBuf>,
    /// Mapping from canonical path prefix to logical (symlink) prefix.
    /// Computed once at init from `$PWD` vs `std::env::current_dir()`.
    symlink_mapping: Option<SymlinkMapping>,
    /// Set when a command removes the CWD's worktree (e.g., `wt merge`).
    /// Used by the error handler to show a "directory was removed" hint.
    /// This explicit flag avoids unreliable CWD detection on Windows where
    /// deleted directories remain accessible to the process that held them.
    cwd_removed: bool,
}

/// Maps canonical path prefixes to logical (symlink-preserved) prefixes.
///
/// When a user navigates via symlink (e.g., `/workspace/project` -> `/mnt/wsl/workspace/project`),
/// `std::env::current_dir()` returns the canonical path while `$PWD` preserves the symlink.
/// This mapping allows translating canonical paths back to the user's logical path
/// for `cd` directives, so the user stays in their symlink tree.
#[derive(Debug, Clone)]
struct SymlinkMapping {
    canonical_prefix: PathBuf,
    logical_prefix: PathBuf,
}

impl SymlinkMapping {
    /// Compute a symlink mapping from `$PWD` (logical) and `current_dir()` (canonical).
    ///
    /// Returns `None` if:
    /// - `$PWD` is not set
    /// - `$PWD` equals `current_dir()` (no symlink)
    /// - `$PWD` doesn't canonicalize to `current_dir()` (stale `$PWD`)
    /// - No common suffix found (leaf-level symlink with different name)
    fn compute() -> Option<Self> {
        let logical_cwd = PathBuf::from(std::env::var("PWD").ok()?);
        let canonical_cwd = std::env::current_dir().ok()?;
        let canonical_of_pwd = dunce::canonicalize(&logical_cwd).ok();
        Self::from_paths(&logical_cwd, &canonical_cwd, canonical_of_pwd.as_deref())
    }

    /// Build a symlink mapping from logical and canonical working directories.
    ///
    /// `canonical_of_logical` is the result of canonicalizing the logical path,
    /// used to verify that `$PWD` is fresh (not stale from a previous `cd`).
    fn from_paths(
        logical_cwd: &Path,
        canonical_cwd: &Path,
        canonical_of_logical: Option<&Path>,
    ) -> Option<Self> {
        // No symlink: paths are identical
        if logical_cwd == canonical_cwd {
            return None;
        }

        // Verify $PWD is fresh — it must canonicalize to the same path as current_dir()
        if canonical_of_logical != Some(canonical_cwd) {
            return None;
        }

        // Find common suffix by matching components from the end
        let logical_components: Vec<_> = logical_cwd.components().collect();
        let canonical_components: Vec<_> = canonical_cwd.components().collect();

        let common_suffix_len = logical_components
            .iter()
            .rev()
            .zip(canonical_components.iter().rev())
            .take_while(|(l, c)| l == c)
            .count();

        // No common suffix means the leaf names differ — can't map
        if common_suffix_len == 0 {
            return None;
        }

        // Build prefixes from the non-matching leading components
        let logical_prefix: PathBuf = logical_components
            [..logical_components.len() - common_suffix_len]
            .iter()
            .collect();
        let canonical_prefix: PathBuf = canonical_components
            [..canonical_components.len() - common_suffix_len]
            .iter()
            .collect();

        Some(SymlinkMapping {
            canonical_prefix,
            logical_prefix,
        })
    }

    /// Translate a canonical path to its logical equivalent.
    ///
    /// Returns `None` if the path doesn't start with the canonical prefix.
    fn to_logical_path(&self, canonical_path: &Path) -> Option<PathBuf> {
        let remainder = canonical_path.strip_prefix(&self.canonical_prefix).ok()?;
        Some(self.logical_prefix.join(remainder))
    }
}

/// Translate a canonical path to the user's logical (symlink-preserved) path.
///
/// If the user navigated via symlink (e.g., `/workspace/project` -> `/mnt/wsl/workspace/project`),
/// this translates canonical paths back to the symlink tree. Returns the original path unchanged
/// if no symlink mapping exists or the translation doesn't round-trip correctly.
pub fn to_logical_path(path: &Path) -> PathBuf {
    let guard = state().lock().expect("OUTPUT_STATE lock poisoned");
    let Some(mapping) = &guard.symlink_mapping else {
        return path.to_path_buf();
    };
    mapping
        .to_logical_path(path)
        .filter(|translated| dunce::canonicalize(translated).ok() == dunce::canonicalize(path).ok())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Get or lazily initialize the global output state.
///
/// Reads directive file env vars from environment on first access and picks
/// a `DirectiveMode`. Empty or whitespace-only strings are treated as "not
/// set" to handle edge cases.
fn state() -> &'static Mutex<OutputState> {
    OUTPUT_STATE.get_or_init(|| {
        let mode = compute_directive_mode();
        let symlink_mapping = SymlinkMapping::compute();

        Mutex::new(OutputState {
            mode,
            target_dir: None,
            symlink_mapping,
            cwd_removed: false,
        })
    })
}

fn read_env_path(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

/// Whether this invocation came through a retired pre-split shell wrapper.
///
/// A split-protocol CD file always wins over a leftover single-file variable.
/// The old variable is recognized only to produce a targeted repair message;
/// wt never writes to it.
pub(crate) fn retired_shell_wrapper_active() -> bool {
    matches!(
        state().lock().expect("OUTPUT_STATE lock poisoned").mode,
        DirectiveMode::Retired
    )
}

fn compute_directive_mode() -> DirectiveMode {
    let cd = read_env_path(DIRECTIVE_CD_FILE_ENV_VAR);
    let exec = read_env_path(DIRECTIVE_EXEC_FILE_ENV_VAR);
    let retired = read_env_path(RETIRED_DIRECTIVE_FILE_ENV_VAR);

    match cd {
        Some(cd_file) => DirectiveMode::Split {
            cd_file,
            exec_file: exec,
        },
        None if retired.is_some() => DirectiveMode::Retired,
        None => DirectiveMode::Interactive,
    }
}

/// Print the canonical repair action for the retired pre-split wrapper once.
pub(crate) fn print_outdated_shell_wrapper_hint_once() {
    if RETIRED_REPAIR_HINT_SHOWN.set(()).is_ok() {
        eprintln!(
            "{}",
            hint_message(cformat!(
                "To update the shell wrapper, run <underline>wt config shell install</>"
            ))
        );
    }
}

/// Refuse `--execute` from a retired wrapper. Fires at most once per process,
/// and supplies the reinstall action when no earlier directory warning did.
fn warn_retired_exec_once(command: &str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_err() {
        return;
    }
    eprintln!(
        "{}",
        warning_message(cformat!(
            "<bold>--execute</> disabled because the shell wrapper is out of date; skipping <bold>{command}</>"
        ))
    );
    print_outdated_shell_wrapper_hint_once();
}

/// Warn that `--execute` was refused because we're running inside an alias or
/// hook body with the EXEC file scrubbed. Fires at most once per process so
/// repeated refusals don't spam the terminal.
fn warn_exec_scrubbed_once(command: &str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_err() {
        return;
    }
    eprintln!(
        "{}",
        warning_message(cformat!(
            "<bold>--execute</> disabled inside project alias / hook bodies for safety; skipping <bold>{command}</>"
        ))
    );
    eprintln!(
        "{}",
        hint_message(cformat!(
            "User-source aliases allow it. Comment at <underline>{EXEC_SCRUB_ISSUE_URL}</> if you need it for project aliases or hooks"
        ))
    );
}

/// Append a line to a directive file.
///
/// `what` names the payload for the failure message ("the command", "the cd
/// command"). The raw `io::Error` names neither the file nor what didn't get
/// written, and this write runs *last* — after the operation the user asked for
/// has already landed — so `✗ No such file or directory (os error 2)` on its own
/// reads as if the command or the worktree were the thing that's missing. These
/// files belong to the shell wrapper, so the message has to point there.
fn append_line(path: &Path, line: &str, what: &str) -> anyhow::Result<()> {
    append_line_io(path, line).with_context(|| {
        format!(
            "Failed to write {what} to the directive file {}",
            path.display()
        )
    })
}

fn append_line_io(path: &Path, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    writeln!(file, "{}", line)?;
    file.flush()
}

/// Truncate-write the given path to the CD directive file. The file holds one
/// line: the absolute path the shell wrapper should `cd` to. Truncate-then-
/// write semantics mean the last writer wins, which matches how overlapping
/// `change_directory()` calls should resolve (hook emits a cd after switch
/// emits its own → hook wins).
///
/// A failure names the file, for the reason [`append_line`] does.
fn write_cd_path(file: &Path, path: &Path) -> anyhow::Result<()> {
    write_cd_path_io(file, path)
        .with_context(|| format!("Failed to write the cd directive file {}", file.display()))
}

fn write_cd_path_io(file: &Path, path: &Path) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(file)?;
    // Lossy for non-UTF-8 paths (extremely rare in practice; worktrunk-
    // managed paths are always valid UTF-8).
    f.write_all(path.as_os_str().to_string_lossy().as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()
}

/// Request directory change (for shell integration).
///
/// Writes the target path to the CD directive file. In interactive mode (no
/// current wrapper), just buffers the target so that a later `execute()` can
/// use it as the child's working directory.
///
/// A write failure names the file it couldn't write — see [`append_line`].
pub fn change_directory(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    let mode = {
        let mut guard = state().lock().expect("OUTPUT_STATE lock poisoned");
        guard.target_dir = Some(path.to_path_buf());
        guard.mode.clone()
    };

    match mode {
        DirectiveMode::Interactive | DirectiveMode::Retired => Ok(()),
        DirectiveMode::Split { cd_file, .. } => write_cd_path(&cd_file, &to_logical_path(path)),
    }
}

/// Mark that the current working directory's worktree has been removed.
///
/// Called by the removal handler (e.g., during `wt merge`) when it knows the
/// process CWD was part of the worktree being removed. The error handler in
/// `main.rs` checks this to show a "directory was removed" hint.
pub fn mark_cwd_removed() {
    state()
        .lock()
        .expect("OUTPUT_STATE lock poisoned")
        .cwd_removed = true;
}

/// Check whether the CWD worktree was removed during this command.
pub fn was_cwd_removed() -> bool {
    state()
        .lock()
        .expect("OUTPUT_STATE lock poisoned")
        .cwd_removed
}

/// Request command execution.
///
/// Dispatches by directive mode:
/// - Interactive: runs the command directly (replacing this process on Unix).
/// - Split protocol with EXEC file: appends the command to the EXEC file; the
///   wrapper sources it after wt exits, so it runs in the user's interactive
///   shell.
/// - Split protocol without EXEC file: refuses the command with a warning. We
///   land here when running inside an alias or hook body, where `Cmd` scrubbed
///   the EXEC var to keep arbitrary shell from reaching the parent session.
/// - Retired wrapper: refuses the command and points to the reinstall action.
///
/// A failed append names the file it couldn't write — see [`append_line`].
pub fn execute(command: impl Into<String>) -> anyhow::Result<()> {
    let command = command.into();

    let (mode, target_dir) = {
        let guard = state().lock().expect("OUTPUT_STATE lock poisoned");
        (guard.mode.clone(), guard.target_dir.clone())
    };

    match mode {
        DirectiveMode::Interactive => execute_command(command, target_dir.as_deref()),
        DirectiveMode::Split {
            exec_file: Some(file),
            ..
        } => append_line(&file, &command, "the command"),
        DirectiveMode::Split {
            exec_file: None, ..
        } => {
            warn_exec_scrubbed_once(&command);
            Ok(())
        }
        DirectiveMode::Retired => {
            warn_retired_exec_once(&command);
            Ok(())
        }
    }
}

/// Whether a call to `execute()` with a non-empty command would be refused
/// by the conservative scrub or a retired wrapper. Callers can use this to
/// suppress pre-exec output (e.g. "Executing (--execute):" headers) so the
/// warning stands alone.
pub fn exec_would_be_refused() -> bool {
    let guard = state().lock().expect("OUTPUT_STATE lock poisoned");
    matches!(
        guard.mode,
        DirectiveMode::Split {
            exec_file: None,
            ..
        } | DirectiveMode::Retired
    )
}

/// Execute a command in the given directory (Unix: exec, non-Unix: spawn)
#[cfg(unix)]
fn execute_command(command: String, target_dir: Option<&Path>) -> anyhow::Result<()> {
    let exec_dir = target_dir.unwrap_or_else(|| Path::new("."));
    let shell = ShellConfig::get()?;

    // Use exec() to replace wt process with the command.
    // This gives the command full TTY access (stdin, stdout, stderr all inherited),
    // enabling interactive programs like `claude` to work properly.
    let mut cmd = shell.command(&command);
    cmd.current_dir(exec_dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // A buffered target means wt relocated the payload into a worktree it
    // selected, so the payload's `git` calls must discover that worktree from
    // the cwd, not an inherited `GIT_DIR` (issue #3373; see
    // `scrub_git_discovery_env_vars`). With no target the payload runs where
    // the user invoked wt, and keeps their context.
    if target_dir.is_some() {
        scrub_git_discovery_env_vars(&mut cmd);
    }
    let err = cmd.exec();

    // exec() only returns on error
    Err(anyhow::anyhow!(cformat!(
        "Failed to exec <bold>{}</> with {}: {}",
        command,
        shell.name,
        err
    )))
}

/// Execute a command in the given directory (non-Unix: spawn and wait)
#[cfg(not(unix))]
fn execute_command(command: String, target_dir: Option<&Path>) -> anyhow::Result<()> {
    let mut cmd = Cmd::shell(&command).stdin(Stdio::inherit());
    if let Some(dir) = target_dir {
        // wt relocated the payload into a worktree it selected; its `git`
        // calls must discover that worktree from the cwd, not an inherited
        // `GIT_DIR` (issue #3373; see `scrub_git_discovery_env_vars`).
        cmd = cmd.current_dir(dir).scrub_git_discovery_env();
    }

    if let Err(err) = cmd.stream() {
        // If the command failed with an exit code, just exit with that code.
        // This matches Unix behavior where exec() replaces the process and
        // the shell's exit code becomes the process exit code (no error message).
        if let Some(WorktrunkError::ChildProcessExited { code, .. }) =
            err.downcast_ref::<WorktrunkError>()
        {
            std::process::exit(*code);
        }
        return Err(err);
    }
    Ok(())
}

/// Terminate command output
///
/// Resets ANSI state on stderr when shell integration is active.
/// In interactive mode (no shell wrapper), message formatting functions
/// already reset their own styles, so no global reset is needed.
pub fn terminate_output() -> io::Result<()> {
    if !is_shell_integration_active() {
        return Ok(());
    }

    // Reset ANSI state before returning to shell. With color off anstream
    // strips the reset, which is correct — nothing emitted color.
    let mut stderr = stderr();
    write!(stderr, "{}", anstyle::Reset)?;
    stderr.flush()
}

/// Check whether the split directive-file protocol is active.
pub fn is_shell_integration_active() -> bool {
    matches!(
        state().lock().expect("OUTPUT_STATE lock poisoned").mode,
        DirectiveMode::Split { .. }
    )
}

/// Compute whether to show "@ path" in hook announcements.
///
/// Returns `Some(hooks_run_at)` when the user's shell is (or will be) somewhere
/// else, so the path annotation helps them understand where hooks executed.
/// Returns `None` when no annotation is needed because the user is (or will be)
/// at the hook location.
///
/// # Arguments
///
/// * `hooks_run_at` - The directory where hooks will execute
/// * `user_location` - Where the user's shell is (or will be) when they see the output
///
/// # Higher-level helpers
///
/// For most cases, use the convenience functions instead of computing `user_location` manually:
/// - [`pre_hook_display_path`] - for pre-hooks and manual `wt hook` invocations
/// - [`post_hook_display_path`] - for post-hooks (handles shell integration internally)
pub fn compute_hooks_display_path<'a>(
    hooks_run_at: &'a std::path::Path,
    user_location: &std::path::Path,
) -> Option<&'a std::path::Path> {
    // Canonicalize both paths for comparison to handle relative vs absolute paths
    // (e.g., "." vs "/absolute/path/to/cwd"). Fall back to direct comparison if
    // canonicalization fails (e.g., path doesn't exist).
    let same_location = match (
        dunce::canonicalize(hooks_run_at),
        dunce::canonicalize(user_location),
    ) {
        (Ok(h), Ok(u)) => h == u,
        _ => hooks_run_at == user_location,
    };

    if same_location {
        None
    } else {
        Some(hooks_run_at)
    }
}

/// Display path for pre-hooks and manual `wt hook` invocations.
///
/// Pre-hooks run while the user is at cwd, and no cd will happen after.
/// Manual `wt hook` commands also run at cwd with no cd.
///
/// Shows the path if hooks run somewhere other than cwd.
///
/// # Examples
///
/// ```ignore
/// // `run_hooks_foreground` computes this internally for all foreground
/// // hooks (including manual `wt hook` runs of post-* hook types);
/// // plan-backed pre-hooks pass it explicitly:
/// execute_planned_hook(..., pre_hook_display_path(ctx.worktree_path))?;
/// ```
pub fn pre_hook_display_path(hooks_run_at: &std::path::Path) -> Option<&std::path::Path> {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => {
            // Can't determine cwd (directory deleted/renamed) - show the path
            // since we can't know if user is there or not
            return Some(hooks_run_at);
        }
    };
    compute_hooks_display_path(hooks_run_at, &cwd)
}

/// Display path for post-hooks.
///
/// Post-hooks run after the operation completes. If shell integration is active,
/// the user will be cd'd to the destination, so no path needs to be shown.
/// Without shell integration, shows the path if user is elsewhere.
///
/// # Examples
///
/// ```ignore
/// // Register hooks with display path:
/// announcer.register(&ctx, HookType::PostCreate, &extra_vars, post_hook_display_path(&destination))?;
/// ```
pub fn post_hook_display_path(destination: &std::path::Path) -> Option<&std::path::Path> {
    post_hook_display_path_with(destination, is_shell_integration_active())
}

fn post_hook_display_path_with(
    destination: &std::path::Path,
    shell_integration_active: bool,
) -> Option<&std::path::Path> {
    if shell_integration_active {
        None // Shell will cd user to destination
    } else {
        pre_hook_display_path(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hooks_display_path_same_location() {
        let path = PathBuf::from("/repo/worktree");
        let result = compute_hooks_display_path(&path, &path);
        assert!(result.is_none(), "Should return None when paths match");
    }

    #[test]
    fn test_compute_hooks_display_path_different_location() {
        let hooks_run_at = PathBuf::from("/repo/feature");
        let user_location = PathBuf::from("/repo/main");
        let result = compute_hooks_display_path(&hooks_run_at, &user_location);
        assert_eq!(result, Some(hooks_run_at.as_path()));
    }

    #[test]
    fn test_pre_hook_display_path_at_cwd() {
        // When hooks run at cwd, no path annotation needed
        let cwd = std::env::current_dir().unwrap();
        let result = pre_hook_display_path(&cwd);
        assert!(result.is_none(), "Should return None when hooks run at cwd");
    }

    #[test]
    fn test_pre_hook_display_path_elsewhere() {
        // When hooks run elsewhere, show the path
        let elsewhere = PathBuf::from("/some/other/path");
        let result = pre_hook_display_path(&elsewhere);
        assert_eq!(
            result,
            Some(elsewhere.as_path()),
            "Should return path when hooks run elsewhere"
        );
    }

    #[test]
    fn test_post_hook_display_path_no_shell_integration() {
        // Without shell integration, post_hook_display_path behaves like pre_hook_display_path.
        // Use the explicit-arg variant so the test is independent of process-wide
        // OUTPUT_STATE, which may be pre-initialized to shell-integration-active when tests
        // are spawned under `wt` (which inherits WORKTRUNK_DIRECTIVE_* env vars).
        let elsewhere = PathBuf::from("/some/destination");
        let result = post_hook_display_path_with(&elsewhere, false);
        let cwd = std::env::current_dir().unwrap();
        if cwd == elsewhere {
            assert!(result.is_none());
        } else {
            assert_eq!(result, Some(elsewhere.as_path()));
        }
    }

    #[test]
    fn test_post_hook_display_path_at_cwd_no_shell_integration() {
        // Without shell integration, if destination == cwd, no path needed.
        let cwd = std::env::current_dir().unwrap();
        let result = post_hook_display_path_with(&cwd, false);
        assert!(
            result.is_none(),
            "Should return None when destination is cwd (no shell integration)"
        );
    }

    #[test]
    fn test_post_hook_display_path_with_shell_integration() {
        // With shell integration active, the shell cds the user to destination,
        // so no annotation is needed.
        let elsewhere = PathBuf::from("/some/destination");
        let result = post_hook_display_path_with(&elsewhere, true);
        assert!(result.is_none());
    }

    #[test]
    fn test_lazy_init_does_not_panic() {
        // Verify lazy initialization doesn't panic.
        // State is lazily initialized on first access.
        let _ = is_shell_integration_active();
    }

    #[test]
    fn test_cwd_removed_flag() {
        // was_cwd_removed() returns the flag set by mark_cwd_removed().
        // Note: global state persists across tests, so we only test mark + read,
        // not the default (which another test may have already changed).
        mark_cwd_removed();
        assert!(was_cwd_removed());
    }

    #[test]
    fn test_spawned_thread_uses_correct_state() {
        use std::sync::mpsc;

        // Spawn a thread and verify it can access output without panicking.
        // State is lazily initialized and shared across threads.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Access output system in spawned thread
            let _ = is_shell_integration_active();
            tx.send(()).unwrap();
        })
        .join()
        .unwrap();

        rx.recv().unwrap();
    }

    /// Test that anstyle formatting is preserved
    #[test]
    fn test_success_preserves_anstyle() {
        use anstyle::{AnsiColor, Color, Style};

        let bold = Style::new().bold();
        let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

        // Create a styled message
        let styled = format!("{cyan}Styled{cyan:#} {bold}message{bold:#}");

        // The styled message should contain ANSI escape codes
        assert!(
            styled.contains('\x1b'),
            "Styled message should contain ANSI escape codes"
        );
    }

    // ========================================================================
    // Symlink Mapping Tests
    // ========================================================================

    #[test]
    fn test_symlink_mapping_to_logical_path() {
        let mapping = SymlinkMapping {
            canonical_prefix: PathBuf::from("/mnt/wsl"),
            logical_prefix: PathBuf::from("/"),
        };

        // Target under canonical prefix should be translated
        let result = mapping.to_logical_path(Path::new("/mnt/wsl/workspace/project.feature"));
        assert_eq!(result, Some(PathBuf::from("/workspace/project.feature")));
    }

    #[test]
    fn test_symlink_mapping_preserves_deep_paths() {
        let mapping = SymlinkMapping {
            canonical_prefix: PathBuf::from("/mnt/wsl"),
            logical_prefix: PathBuf::from("/"),
        };

        let result = mapping.to_logical_path(Path::new("/mnt/wsl/a/b/c/d"));
        assert_eq!(result, Some(PathBuf::from("/a/b/c/d")));
    }

    #[test]
    fn test_symlink_mapping_no_match() {
        let mapping = SymlinkMapping {
            canonical_prefix: PathBuf::from("/mnt/wsl"),
            logical_prefix: PathBuf::from("/"),
        };

        // Path outside canonical prefix returns None
        let result = mapping.to_logical_path(Path::new("/other/path"));
        assert_eq!(result, None);
    }

    #[test]
    fn test_symlink_mapping_macos_private_var() {
        // macOS: /var -> /private/var
        let mapping = SymlinkMapping {
            canonical_prefix: PathBuf::from("/private"),
            logical_prefix: PathBuf::from("/"),
        };

        let result = mapping.to_logical_path(Path::new("/private/var/folders/project.feature"));
        assert_eq!(result, Some(PathBuf::from("/var/folders/project.feature")));
    }

    #[test]
    fn test_symlink_mapping_equal_length_prefixes() {
        // When logical and canonical prefixes have the same depth
        let mapping = SymlinkMapping {
            canonical_prefix: PathBuf::from("/real/path"),
            logical_prefix: PathBuf::from("/link/path"),
        };

        let result = mapping.to_logical_path(Path::new("/real/path/workspace/project"));
        assert_eq!(result, Some(PathBuf::from("/link/path/workspace/project")));
    }

    // ========================================================================
    // SymlinkMapping::from_paths Tests
    // ========================================================================

    #[test]
    fn test_from_paths_no_symlink() {
        // When logical == canonical, no mapping needed
        let result = SymlinkMapping::from_paths(
            Path::new("/workspace/project"),
            Path::new("/workspace/project"),
            Some(Path::new("/workspace/project")),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_from_paths_stale_pwd() {
        // When canonical_of_logical doesn't match canonical_cwd, PWD is stale
        let result = SymlinkMapping::from_paths(
            Path::new("/old/link/project"),
            Path::new("/real/project"),
            Some(Path::new("/different/project")),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_from_paths_canonicalize_failed() {
        // When canonicalize returns None (path doesn't exist)
        let result = SymlinkMapping::from_paths(
            Path::new("/link/project"),
            Path::new("/real/project"),
            None,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_from_paths_no_common_suffix() {
        // When leaf names differ entirely — can't determine prefix mapping
        let result = SymlinkMapping::from_paths(
            Path::new("/link/alpha"),
            Path::new("/real/beta"),
            Some(Path::new("/real/beta")),
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_from_paths_wsl_style_symlink() {
        // WSL: /workspace/project -> /mnt/wsl/workspace/project
        let result = SymlinkMapping::from_paths(
            Path::new("/workspace/project"),
            Path::new("/mnt/wsl/workspace/project"),
            Some(Path::new("/mnt/wsl/workspace/project")),
        );
        let mapping = result.expect("should produce mapping");
        assert_eq!(mapping.logical_prefix, PathBuf::from("/"));
        assert_eq!(mapping.canonical_prefix, PathBuf::from("/mnt/wsl"));
    }

    #[test]
    fn test_from_paths_macos_private_var() {
        // macOS: /var/folders/xx/tmp -> /private/var/folders/xx/tmp
        let result = SymlinkMapping::from_paths(
            Path::new("/var/folders/xx/tmp"),
            Path::new("/private/var/folders/xx/tmp"),
            Some(Path::new("/private/var/folders/xx/tmp")),
        );
        let mapping = result.expect("should produce mapping");
        assert_eq!(mapping.logical_prefix, PathBuf::from("/"));
        assert_eq!(mapping.canonical_prefix, PathBuf::from("/private"));
    }

    #[test]
    fn test_from_paths_equal_depth_prefixes() {
        // Symlink at the same depth: /link/path/project -> /real/path/project
        let result = SymlinkMapping::from_paths(
            Path::new("/link/path/project"),
            Path::new("/real/path/project"),
            Some(Path::new("/real/path/project")),
        );
        let mapping = result.expect("should produce mapping");
        assert_eq!(mapping.logical_prefix, PathBuf::from("/link"));
        assert_eq!(mapping.canonical_prefix, PathBuf::from("/real"));
        // path/project is the common suffix
    }
}
