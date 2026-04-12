use anyhow::Context;
use color_print::cformat;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use strum::IntoEnumIterator;
use worktrunk::git::{HookType, Repository};
use worktrunk::path::{format_path_for_display, sanitize_for_filename};
use worktrunk::utils::epoch_now;

use crate::commands::hook_filter::HookSource;

// ==================== Hook Log Specification ====================

/// Internal worktrunk operations that produce log files.
///
/// These are operations performed by worktrunk itself (not user-defined hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display, strum::EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum InternalOp {
    /// Background worktree removal (`wt remove` in background mode)
    Remove,
    /// Background cleanup of stale entries in `.git/wt/trash/`
    TrashSweep,
}

/// Specification for a hook log file.
///
/// This is the single source of truth for hook log file paths.
/// Used by both log creation (in `spawn_detached`) and log lookup (in `handle_logs_get`).
///
/// # Log file layout
///
/// Hook commands produce logs at: `{branch}/{source}/{hook-type}/{name}.log`
/// - Example: `feature/user/post-start/server.log`
///
/// Internal operations produce logs at: `{branch}/internal/{op}.log`
/// - Example: `feature/internal/remove.log`
///
/// Branch and hook names are sanitized for filesystem safety via
/// `sanitize_for_filename`, which replaces invalid characters and appends a
/// short collision-avoidance hash.
///
/// # CLI format for lookup
///
/// The first segment determines the log type:
/// - `user:hook-type:name` → User hook (e.g., `user:post-start:server`)
/// - `project:hook-type:name` → Project hook (e.g., `project:post-create:build`)
/// - `internal:op` → Internal operation (e.g., `internal:remove`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookLog {
    /// Hook command log: `{branch}/{source}/{hook-type}/{name}.log`
    Hook {
        source: HookSource,
        hook_type: HookType,
        name: String,
    },
    /// Internal operation log: `{branch}/internal/{op}.log`
    Internal(InternalOp),
}

impl HookLog {
    /// Create a hook command log specification.
    pub fn hook(source: HookSource, hook_type: HookType, name: impl Into<String>) -> Self {
        Self::Hook {
            source,
            hook_type,
            name: name.into(),
        }
    }

    /// Create an internal operation log specification.
    pub fn internal(op: InternalOp) -> Self {
        Self::Internal(op)
    }

    /// Generate the full log path for a branch in the given log directory.
    ///
    /// Builds the nested path under `{log_dir}/{sanitized-branch}/...`.
    /// Parent directories must be created by the caller (see `create_detach_log`).
    pub fn path(&self, log_dir: &Path, branch: &str) -> PathBuf {
        let branch_dir = log_dir.join(sanitize_for_filename(branch));
        match self {
            HookLog::Hook {
                source,
                hook_type,
                name,
            } => branch_dir
                .join(source.to_string())
                .join(hook_type.to_string())
                .join(format!("{}.log", sanitize_for_filename(name))),
            HookLog::Internal(op) => branch_dir.join("internal").join(format!("{op}.log")),
        }
    }

    /// Convert to CLI spec format (for error messages and roundtrip).
    ///
    /// Returns the format used by `parse()`: `source:hook-type:name` or `internal:op`.
    pub fn to_spec(&self) -> String {
        match self {
            HookLog::Hook {
                source,
                hook_type,
                name,
            } => format!("{}:{}:{}", source, hook_type, name),
            HookLog::Internal(op) => format!("internal:{}", op),
        }
    }

    /// Parse from CLI argument.
    ///
    /// # Formats
    ///
    /// The first segment determines the type:
    /// - `user:hook-type:name` → User hook log
    /// - `project:hook-type:name` → Project hook log
    /// - `internal:op` → Internal operation log
    ///
    /// # Errors
    ///
    /// Returns an error if the format is invalid or unrecognized.
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split(':').collect();

        match parts.as_slice() {
            // internal:op
            ["internal", op_str] => {
                let op = InternalOp::from_str(op_str).map_err(|_| {
                    let valid: Vec<_> = InternalOp::iter().map(|o| o.to_string()).collect();
                    cformat!(
                        "Unknown internal operation: <bold>{}</>. Valid: {}",
                        op_str,
                        valid.join(", ")
                    )
                })?;
                Ok(Self::Internal(op))
            }
            // source:hook-type:name
            [source_str, hook_type_str, name] if !name.is_empty() => {
                let source = HookSource::from_str(source_str).map_err(|_| {
                    cformat!(
                        "Unknown source: <bold>{}</>. Valid: user, project",
                        source_str
                    )
                })?;
                let hook_type = HookType::from_str(hook_type_str).map_err(|_| {
                    let valid: Vec<_> = HookType::iter().map(|h| h.to_string()).collect();
                    cformat!(
                        "Unknown hook type: <bold>{}</>. Valid: {}",
                        hook_type_str,
                        valid.join(", ")
                    )
                })?;
                Ok(Self::Hook {
                    source,
                    hook_type,
                    name: (*name).to_string(),
                })
            }
            _ => Err(cformat!(
                "Invalid log spec: <bold>{}</>. Format: source:hook-type:name or internal:op",
                s
            )),
        }
    }
}

/// Get the separator needed before closing brace in POSIX shell command grouping.
/// Returns empty string if command already ends with newline or semicolon.
fn posix_command_separator(command: &str) -> &'static str {
    if command.ends_with('\n') || command.ends_with(';') {
        ""
    } else {
        ";"
    }
}

/// Create the log directory and file for a detached process.
///
/// Returns `(log_path, log_file)`. Shared by `spawn_detached` and
/// `spawn_detached_exec`.
fn create_detach_log(
    repo: &Repository,
    branch: &str,
    hook_log: &HookLog,
) -> anyhow::Result<(PathBuf, fs::File)> {
    let log_dir = repo.wt_logs_dir();
    let log_path = hook_log.path(&log_dir, branch);

    // Create the full ancestor chain (e.g., {log_dir}/{branch}/{source}/{hook-type}/).
    // log_path always has a parent here because HookLog::path() always appends at
    // least one segment beyond log_dir.
    let parent = log_path
        .parent()
        .expect("HookLog::path always includes a parent");
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create log directory {}",
            format_path_for_display(parent)
        )
    })?;

    let log_file = fs::File::create(&log_path).with_context(|| {
        format!(
            "Failed to create log file {}",
            format_path_for_display(&log_path)
        )
    })?;

    Ok((log_path, log_file))
}

/// Build a [`Command`] that runs `program` at nice 19 when `low_priority` is set.
///
/// Used by the detached-spawn paths so internal cleanup ops (`wt remove`'s background
/// `rm -rf`, trash sweep) don't compete with foreground work. The nice value is
/// inherited across `fork`/`exec`, so wrapping the outer shell or the target binary
/// is enough — any children inherit it too.
#[cfg(unix)]
fn low_priority_command(program: impl AsRef<std::ffi::OsStr>, low_priority: bool) -> Command {
    if low_priority {
        let mut cmd = Command::new("nice");
        cmd.arg("-n").arg("19").arg("--").arg(program);
        cmd
    } else {
        Command::new(program)
    }
}

/// Spawn a detached background process with output redirected to a log file.
///
/// The process will be fully detached from the parent:
/// - On Unix: uses `process_group(0)` to create a new process group (survives PTY closure)
/// - On Windows: uses `CREATE_NEW_PROCESS_GROUP` to detach from console
///
/// Internal ops (`HookLog::Internal`) are run at nice 19 so their I/O and CPU don't
/// compete with user-visible work; user hooks run at normal priority.
///
/// Logs are centralized in the main worktree's `.git/wt/logs/` directory.
pub fn spawn_detached(
    repo: &Repository,
    worktree_path: &Path,
    command: &str,
    branch: &str,
    hook_log: &HookLog,
    context_json: Option<&str>,
) -> anyhow::Result<std::path::PathBuf> {
    let (log_path, log_file) = create_detach_log(repo, branch, hook_log)?;

    log::debug!(
        "$ {} (detached, logging to {})",
        command,
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    #[cfg(unix)]
    {
        let low_priority = matches!(hook_log, HookLog::Internal(_));
        spawn_detached_unix(worktree_path, command, log_file, context_json, low_priority)?;
    }

    #[cfg(windows)]
    {
        spawn_detached_windows(worktree_path, command, log_file, context_json)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_unix(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    context_json: Option<&str>,
    low_priority: bool,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    // Build the command, optionally piping JSON context to stdin
    let full_command = match context_json {
        Some(json) => {
            // Use printf to pipe JSON to the command's stdin
            // printf is more portable than echo for arbitrary content
            // Wrap command in braces to ensure proper grouping with &&, ||, etc.
            format!(
                "printf '%s' {} | {{ {}{} }}",
                shell_escape::escape(json.into()),
                command,
                posix_command_separator(command)
            )
        }
        None => command.to_string(),
    };

    // Wrap in braces so `&` backgrounds the entire compound command.
    // Without braces, `cmd1 && cmd2; cmd3 &` parses as two statements:
    // `cmd1 && cmd2` (foreground) then `cmd3 &` (background) — the semicolon
    // has lower precedence than `&`, so only the last segment is backgrounded.
    let shell_cmd = format!(
        "{{ {}{} }} &",
        full_command,
        posix_command_separator(&full_command)
    );

    // Detachment via process_group(0): puts the spawned shell in its own process group.
    // When the controlling PTY closes, SIGHUP is sent to the foreground process group.
    // Since our process is in a different group, it doesn't receive the signal.
    //
    // For low-priority ops (internal cleanup), wrap the shell in `nice -n 19` so the
    // backgrounded command inherits nice 19. `nice` is POSIX and shipped in base
    // macOS/Linux; a missing binary would fail the spawn (tolerable — these are the
    // same environments where `renice` is already assumed present elsewhere).
    let mut cmd = low_priority_command("sh", low_priority);
    cmd.arg("-c")
        .arg(&shell_cmd)
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .process_group(0); // New process group, not in PTY's foreground group
    // Prevent hooks from writing to the directive file
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    let mut child = cmd.spawn().context("Failed to spawn detached process")?;

    // Wait for sh to exit (immediate, doesn't block on background command)
    child
        .wait()
        .context("Failed to wait for detachment shell")?;

    Ok(())
}

#[cfg(windows)]
fn spawn_detached_windows(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    context_json: Option<&str>,
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use worktrunk::shell_exec::ShellConfig;

    // CREATE_NEW_PROCESS_GROUP: Creates new process group (0x00000200)
    // DETACHED_PROCESS: Creates process without console (0x00000008)
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    let shell = ShellConfig::get()?;

    // Build the command based on shell type
    let mut cmd = if shell.is_posix() {
        // Git Bash available - use same syntax as Unix
        let full_command = match context_json {
            Some(json) => {
                // Use printf to pipe JSON to the command's stdin (same as Unix)
                format!(
                    "printf '%s' {} | {{ {}{} }}",
                    shell_escape::escape(json.into()),
                    command,
                    posix_command_separator(command)
                )
            }
            None => command.to_string(),
        };
        shell.command(&full_command)
    } else {
        // PowerShell fallback
        let full_command = match context_json {
            Some(json) => {
                // PowerShell single-quote escaping:
                // - Single quotes prevent variable expansion ($) and are literal
                // - Backticks are literal in single quotes (NOT escape characters)
                // - Only single quotes need doubling (`'` → `''`)
                // See: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_quoting_rules
                let escaped_json = json.replace('\'', "''");
                // Pipe JSON to the command via PowerShell script block
                format!("'{}' | & {{ {} }}", escaped_json, command)
            }
            None => command.to_string(),
        };
        shell.command(&full_command)
    };

    cmd.current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    // Prevent hooks from writing to the directive file
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    cmd.spawn().context("Failed to spawn detached process")?;

    // Windows: Process is fully detached via DETACHED_PROCESS flag,
    // no need to wait (unlike Unix which waits for the outer shell)

    Ok(())
}

/// Spawn a detached background process by executing a binary directly.
///
/// Unlike [`spawn_detached`] (which wraps a shell command in `sh -c`), this
/// spawns the executable without an intermediate shell. Stdin bytes are written
/// to the child's stdin pipe and then the pipe is closed.
///
/// Used for structured child processes like `wt hook run-pipeline` where the parent
/// passes data via stdin rather than through a temp file or shell arguments.
pub fn spawn_detached_exec(
    repo: &Repository,
    worktree_path: &Path,
    program: &Path,
    args: &[&str],
    branch: &str,
    hook_log: &HookLog,
    stdin_bytes: &[u8],
) -> anyhow::Result<std::path::PathBuf> {
    let (log_path, log_file) = create_detach_log(repo, branch, hook_log)?;

    log::debug!(
        "$ {} {} (detached, logging to {})",
        program.display(),
        args.join(" "),
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    #[cfg(unix)]
    {
        let low_priority = matches!(hook_log, HookLog::Internal(_));
        spawn_detached_exec_unix(
            worktree_path,
            program,
            args,
            log_file,
            stdin_bytes,
            low_priority,
        )?;
    }

    #[cfg(windows)]
    {
        spawn_detached_exec_windows(worktree_path, program, args, log_file, stdin_bytes)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_exec_unix(
    worktree_path: &Path,
    program: &Path,
    args: &[&str],
    log_file: fs::File,
    stdin_bytes: &[u8],
    low_priority: bool,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::process::CommandExt;

    // See `spawn_detached_unix` for rationale on the `nice -n 19` wrapper.
    let mut cmd = low_priority_command(program, low_priority);
    cmd.args(args)
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .process_group(0);
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    let mut child = cmd.spawn().context("Failed to spawn detached process")?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe — child may exit before reading all input.
        let _ = stdin.write_all(stdin_bytes);
    }

    Ok(())
}

#[cfg(windows)]
fn spawn_detached_exec_windows(
    worktree_path: &Path,
    program: &Path,
    args: &[&str],
    log_file: fs::File,
    stdin_bytes: &[u8],
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    let mut child = cmd.spawn().context("Failed to spawn detached process")?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_bytes);
    }

    Ok(())
}

/// Generate a staging path for worktree removal.
///
/// Places the staging directory inside `.git/wt/trash/` so it is hidden from the
/// user's workspace. For the main worktree, `.git/` is on the same filesystem,
/// so `rename()` is an instant metadata operation. Linked worktrees on different
/// mount points will get EXDEV and fall back to legacy removal.
///
/// Format: `<wt/trash>/<name>-<timestamp>`
pub fn generate_removing_path(trash_dir: &Path, worktree_path: &Path) -> PathBuf {
    let timestamp = epoch_now();
    let name = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    trash_dir.join(format!("{}-{}", name, timestamp))
}

/// How old a `.git/wt/trash/` entry must be before [`sweep_stale_trash`] deletes it.
pub const TRASH_STALE_THRESHOLD_SECS: u64 = 24 * 60 * 60;

/// Fire-and-forget cleanup of stale entries in `.git/wt/trash/`.
///
/// Worktree removal uses a fast path that renames the worktree into
/// `.git/wt/trash/<name>-<timestamp>/` and deletes it in a detached background
/// process. If that process is interrupted (SIGKILL, reboot, disk full), the
/// renamed directory is orphaned. `wt remove` calls this function after its
/// primary user-visible output — so the sweep never delays the progress or
/// success message — to provide eventual cleanup: entries older than
/// [`TRASH_STALE_THRESHOLD_SECS`] are removed by a single detached `rm -rf`.
///
/// Best effort: directory read failures and spawn failures are logged at debug
/// level and otherwise ignored. The sweep is purely additive — the primary
/// `wt remove` operation proceeds regardless of outcome.
pub fn sweep_stale_trash(repo: &Repository) {
    let trash_dir = repo.wt_trash_dir();
    let stale = collect_stale_trash_entries(&trash_dir, epoch_now(), TRASH_STALE_THRESHOLD_SECS);
    if stale.is_empty() {
        return;
    }

    // Join all paths into a single `rm -rf` invocation so we spawn one
    // background process regardless of how many entries are stale.
    let escaped: Vec<String> = stale
        .iter()
        .map(|p| shell_escape::escape(p.to_string_lossy().as_ref().into()).into_owned())
        .collect();
    let command = format!("rm -rf -- {}", escaped.join(" "));

    if let Err(e) = spawn_detached(
        repo,
        &repo.wt_dir(),
        &command,
        "wt",
        &HookLog::internal(InternalOp::TrashSweep),
        None,
    ) {
        log::debug!("Failed to spawn stale trash sweep: {e}");
    }
}

/// Collect paths in `trash_dir` whose embedded timestamp is older than
/// `threshold_secs` relative to `now`.
///
/// Entries whose filename can't be parsed as `<name>-<timestamp>` are skipped —
/// the sweep only touches directories worktrunk created via
/// [`generate_removing_path`].
fn collect_stale_trash_entries(trash_dir: &Path, now: u64, threshold_secs: u64) -> Vec<PathBuf> {
    let Ok(read_dir) = fs::read_dir(trash_dir) else {
        return Vec::new();
    };

    read_dir
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let timestamp = parse_trash_entry_timestamp(name.to_str()?)?;
            let age = now.saturating_sub(timestamp);
            (age >= threshold_secs).then(|| entry.path())
        })
        .collect()
}

/// Extract the Unix timestamp suffix from a trash entry filename.
///
/// Filenames produced by [`generate_removing_path`] have the form
/// `<name>-<timestamp>`, where timestamp is a bare unsigned integer in Unix
/// epoch seconds. The worktree name may contain hyphens, so splitting from the
/// right and parsing the tail is unambiguous.
fn parse_trash_entry_timestamp(name: &str) -> Option<u64> {
    let (_, suffix) = name.rsplit_once('-')?;
    suffix.parse::<u64>().ok()
}

/// Build shell command for background removal of a staged (renamed) worktree.
///
/// This is used after the worktree has been renamed to a staging path,
/// git metadata has been pruned, and the branch has been deleted synchronously.
///
/// When `changed_directory` is true — the shell is cd-ing away from the removed
/// worktree — a placeholder directory is created at `original_path` so the shell's
/// working directory remains valid until the wrapper has processed the `cd`
/// directive. Without this, shells that validate `$env.PWD` (notably Nushell)
/// emit errors between binary exit and the `cd`. The background command then
/// waits for the shell wrapper before cleaning up the placeholder.
///
/// When `changed_directory` is false, no placeholder exists, so the background
/// command just removes the staged directory directly.
pub fn build_remove_command_staged(
    staged_path: &std::path::Path,
    original_path: &std::path::Path,
    changed_directory: bool,
) -> String {
    use shell_escape::escape;

    let staged_path_str = staged_path.to_string_lossy();
    let staged_escaped = escape(staged_path_str.as_ref().into());

    if changed_directory {
        let original_path_str = original_path.to_string_lossy();
        let original_escaped = escape(original_path_str.as_ref().into());

        // sleep 1: give the shell wrapper time to cd away before removing the placeholder.
        // rmdir: remove the empty placeholder (safe — only removes empty directories).
        // rm -rf: remove the staged worktree contents.
        // Use -- to prevent option parsing for paths starting with -
        format!(
            "sleep 1 && rmdir -- {} 2>/dev/null; rm -rf -- {}",
            original_escaped, staged_escaped
        )
    } else {
        // No placeholder to clean up — just remove the staged directory.
        format!("rm -rf -- {}", staged_escaped)
    }
}

/// Build shell command for background worktree removal (legacy path).
///
/// This is the fallback for when rename-based removal fails (e.g., cross-filesystem)
/// or for foreground mode where `git worktree remove` provides better error messages.
///
/// `branch_to_delete` is the branch to delete after removing the worktree.
/// Pass `None` for detached HEAD or when branch should be retained.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
///
/// `force_worktree` adds `--force` to `git worktree remove`, allowing removal
/// even when the worktree contains untracked files (like build artifacts).
///
/// When `changed_directory` is true, a 1-second delay runs first so the shell
/// wrapper can cd away before the directory is removed. When false (removing a
/// non-current worktree), the removal runs immediately.
pub fn build_remove_command(
    worktree_path: &std::path::Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
    changed_directory: bool,
) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "{{ git -C {} fsmonitor--daemon stop 2>/dev/null || true; }}",
        worktree_escaped
    );

    let force_flag = if force_worktree { " --force" } else { "" };

    // When removing the current worktree, delay so the shell wrapper can cd away
    // before the directory is removed. The primary fix for the "shell-init: error
    // retrieving current directory" race is in the fish wrapper (using builtins
    // instead of subprocesses to read the directive), but this provides defense in
    // depth for other shells and edge cases.
    let prefix = if changed_directory {
        format!("sleep 1 && {} && ", stop_fsmonitor)
    } else {
        format!("{} && ", stop_fsmonitor)
    };

    match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{}git worktree remove{} {} && git branch -D {}",
                prefix, force_flag, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{}git worktree remove{} {}",
                prefix, force_flag, worktree_escaped
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use path_slash::PathExt as _;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_low_priority_command() {
        use std::ffi::OsStr;

        let cmd = low_priority_command("sh", true);
        assert_eq!(cmd.get_program(), "nice");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(
            args,
            &[
                OsStr::new("-n"),
                OsStr::new("19"),
                OsStr::new("--"),
                OsStr::new("sh"),
            ]
        );

        let cmd = low_priority_command("sh", false);
        assert_eq!(cmd.get_program(), "sh");
        assert_eq!(cmd.get_args().count(), 0);
    }

    #[test]
    fn test_sanitize_for_filename() {
        // Path separators, Windows-illegal characters, multiple special chars,
        // already-safe names, and reserved prefix names
        assert_snapshot!(
            [
                ("path separator /", sanitize_for_filename("feature/branch")),
                ("path separator \\", sanitize_for_filename("feature\\branch")),
                ("colon", sanitize_for_filename("bug:123")),
                ("angle brackets", sanitize_for_filename("fix<angle>")),
                ("pipe", sanitize_for_filename("fix|pipe")),
                ("question mark", sanitize_for_filename("fix?question")),
                ("wildcard", sanitize_for_filename("fix*wildcard")),
                ("quotes", sanitize_for_filename("fix\"quotes\"")),
                ("multiple special", sanitize_for_filename("a/b\\c<d>e:f\"g|h?i*j")),
                ("already safe", sanitize_for_filename("normal-branch")),
                ("underscore", sanitize_for_filename("branch_with_underscore")),
                ("reserved prefix CONSOLE", sanitize_for_filename("CONSOLE")),
                ("reserved prefix COM10", sanitize_for_filename("COM10")),
            ]
            .into_iter()
            .map(|(label, val)| format!("{label}: {val}"))
            .collect::<Vec<_>>()
            .join("\n"),
            @r"
        path separator /: feature-branch-30k
        path separator \: feature-branch-k37
        colon: bug-123-4xh
        angle brackets: fix-angle-q9m
        pipe: fix-pipe-68k
        question mark: fix-question-ab6
        wildcard: fix-wildcard-38y
        quotes: fix-quotes-2xu
        multiple special: a-b-c-d-e-f-g-h-i-j-obi
        already safe: normal-branch-83y
        underscore: branch_with_underscore-b65
        reserved prefix CONSOLE: CONSOLE-8fv
        reserved prefix COM10: COM10-1s2
        "
        );

        // Windows reserved device names are handled (produce valid filenames)
        // The sanitize-filename crate replaces these rather than prefixing
        // Note: crate matches COM0-9/LPT0-9, stricter than Windows (which only reserves 1-9)
        for name in [
            "CON", "con", "PRN", "AUX", "NUL", "COM0", "COM1", "com9", "LPT0", "LPT1", "lpt9",
        ] {
            let result = sanitize_for_filename(name);
            assert!(!result.is_empty() && result.len() > 3, "{name} -> {result}");
        }

        // Collision avoidance: different inputs produce different outputs
        let a = sanitize_for_filename("feature/x");
        let b = sanitize_for_filename("feature-x");
        assert_ne!(a, b, "should not collide: {a} vs {b}");
    }

    #[test]
    fn test_posix_command_separator() {
        // Commands ending with newline don't need separator
        assert_eq!(posix_command_separator("echo hello\n"), "");

        // Commands ending with semicolon don't need separator
        assert_eq!(posix_command_separator("echo hello;"), "");

        // Commands without trailing newline/semicolon need separator
        assert_eq!(posix_command_separator("echo hello"), ";");

        // Empty command needs separator
        assert_eq!(posix_command_separator(""), ";");

        // Commands with internal newlines but not trailing
        assert_eq!(posix_command_separator("echo\nhello"), ";");

        // Commands with internal semicolons but not trailing
        assert_eq!(posix_command_separator("echo; hello"), ";");
    }

    #[test]
    fn test_build_remove_command() {
        use std::path::PathBuf;

        let path = PathBuf::from("/tmp/test-worktree");

        // changed_directory=true: sleep before removal
        assert_snapshot!(build_remove_command(&path, None, false, true), @"sleep 1 && { git -C /tmp/test-worktree fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove /tmp/test-worktree");
        assert_snapshot!(build_remove_command(&path, Some("feature-branch"), false, true), @"sleep 1 && { git -C /tmp/test-worktree fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove /tmp/test-worktree && git branch -D feature-branch");

        // changed_directory=false: no sleep
        assert_snapshot!(build_remove_command(&path, None, false, false), @"{ git -C /tmp/test-worktree fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove /tmp/test-worktree");
        assert_snapshot!(build_remove_command(&path, Some("feature-branch"), false, false), @"{ git -C /tmp/test-worktree fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove /tmp/test-worktree && git branch -D feature-branch");

        // With force flag
        assert_snapshot!(build_remove_command(&path, None, true, true), @"sleep 1 && { git -C /tmp/test-worktree fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove --force /tmp/test-worktree");

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        assert_snapshot!(build_remove_command(&special_path, Some("feature/branch"), false, true), @"sleep 1 && { git -C '/tmp/test worktree' fsmonitor--daemon stop 2>/dev/null || true; } && git worktree remove '/tmp/test worktree' && git branch -D feature/branch");
    }

    #[test]
    fn test_generate_removing_path() {
        let trash_dir = PathBuf::from("/tmp/repo/.git/wt/trash");
        let path = PathBuf::from("/tmp/my-project.feature");
        let removing_path = generate_removing_path(&trash_dir, &path);

        // Should be inside the trash directory
        assert_eq!(removing_path.parent(), Some(trash_dir.as_path()));

        // Should have the expected prefix
        let name = removing_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("my-project.feature-"), "got: {}", name);

        // Should have a timestamp suffix (digits only after the prefix)
        let timestamp_part = name.trim_start_matches("my-project.feature-");
        assert!(
            timestamp_part.chars().all(|c| c.is_ascii_digit()),
            "timestamp part should be numeric: {}",
            timestamp_part
        );
    }

    #[test]
    fn test_build_remove_command_staged() {
        let staged_path = PathBuf::from("/tmp/repo/.git/wt/trash/my-project.feature-1234567890");
        let original_path = PathBuf::from("/tmp/my-project.feature");

        // changed_directory=true: placeholder cleanup before rm -rf
        assert_snapshot!(build_remove_command_staged(&staged_path, &original_path, true), @"sleep 1 && rmdir -- /tmp/my-project.feature 2>/dev/null; rm -rf -- /tmp/repo/.git/wt/trash/my-project.feature-1234567890");

        // changed_directory=false: just rm -rf, no placeholder
        assert_snapshot!(build_remove_command_staged(&staged_path, &original_path, false), @"rm -rf -- /tmp/repo/.git/wt/trash/my-project.feature-1234567890");

        // Shell escaping for special characters (space in path)
        let special_path = PathBuf::from("/tmp/repo/.git/wt/trash/test worktree-123");
        let special_original = PathBuf::from("/tmp/test worktree");
        assert_snapshot!(build_remove_command_staged(&special_path, &special_original, true), @"sleep 1 && rmdir -- '/tmp/test worktree' 2>/dev/null; rm -rf -- '/tmp/repo/.git/wt/trash/test worktree-123'");
    }

    #[test]
    fn test_hook_log_path() {
        use worktrunk::git::HookType;

        let log_dir = Path::new("/repo/.git/wt/logs");

        // Hook path: {log_dir}/{sanitized-branch}/{source}/{hook-type}/{sanitized-name}.log
        let log = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        assert_snapshot!(
            log.path(log_dir, "main").to_slash_lossy(),
            @"/repo/.git/wt/logs/main-vfz/user/post-start/server-f4t.log"
        );

        // Slash in branch name gets sanitized (feature/auth → feature-auth-{hash})
        assert_snapshot!(
            log.path(log_dir, "feature/auth").to_slash_lossy(),
            @"/repo/.git/wt/logs/feature-auth-j34/user/post-start/server-f4t.log"
        );

        // Project source
        let log = HookLog::hook(HookSource::Project, HookType::PreStart, "build");
        assert_snapshot!(
            log.path(log_dir, "main").to_slash_lossy(),
            @"/repo/.git/wt/logs/main-vfz/project/pre-start/build-seq.log"
        );

        // Constructor and parse produce identical paths
        assert_eq!(
            HookLog::hook(HookSource::User, HookType::PostStart, "server").path(log_dir, "main"),
            HookLog::parse("user:post-start:server")
                .unwrap()
                .path(log_dir, "main"),
        );

        // Internal operation path: {log_dir}/{sanitized-branch}/internal/{op}.log
        assert_snapshot!(
            HookLog::internal(InternalOp::Remove).path(log_dir, "main").to_slash_lossy(),
            @"/repo/.git/wt/logs/main-vfz/internal/remove.log"
        );

        // Non-branch-scoped internal ops (like TrashSweep) use a pseudo-branch
        // at the top level — `wt remove` calls this with branch = "wt".
        assert_snapshot!(
            HookLog::internal(InternalOp::TrashSweep).path(log_dir, "wt").to_slash_lossy(),
            @"/repo/.git/wt/logs/wt-boj/internal/trash-sweep.log"
        );
    }

    #[test]
    fn test_hook_log_parse_internal() {
        let log = HookLog::parse("internal:remove").unwrap();
        assert_eq!(log, HookLog::Internal(InternalOp::Remove));
    }

    #[test]
    fn test_hook_log_parse_errors() {
        // Unknown source
        assert_snapshot!(HookLog::parse("invalid:post-start:server").unwrap_err(), @"Unknown source: [1minvalid[22m. Valid: user, project");

        // Unknown hook type
        assert_snapshot!(HookLog::parse("user:invalid-hook:server").unwrap_err(), @"Unknown hook type: [1minvalid-hook[22m. Valid: pre-switch, post-switch, pre-start, post-start, pre-commit, post-commit, pre-merge, post-merge, pre-remove, post-remove");

        // Unknown internal operation
        assert_snapshot!(HookLog::parse("internal:unknown").unwrap_err(), @"Unknown internal operation: [1munknown[22m. Valid: remove, trash-sweep");

        // Invalid formats: single word, two non-internal parts, missing segment
        assert_snapshot!(HookLog::parse("remove").unwrap_err(), @"Invalid log spec: [1mremove[22m. Format: source:hook-type:name or internal:op");
        assert_snapshot!(HookLog::parse("foo:bar").unwrap_err(), @"Invalid log spec: [1mfoo:bar[22m. Format: source:hook-type:name or internal:op");
        assert_snapshot!(HookLog::parse("user:").unwrap_err(), @"Invalid log spec: [1muser:[22m. Format: source:hook-type:name or internal:op");

        // Colons in hook names (ambiguous parsing)
        assert_snapshot!(HookLog::parse("user:post-start:my:server").unwrap_err(), @"Invalid log spec: [1muser:post-start:my:server[22m. Format: source:hook-type:name or internal:op");

        // Empty hook name
        assert_snapshot!(HookLog::parse("user:post-start:").unwrap_err(), @"Invalid log spec: [1muser:post-start:[22m. Format: source:hook-type:name or internal:op");
    }

    #[test]
    fn test_hook_log_roundtrip() {
        // What gets created should match what gets looked up
        use worktrunk::git::HookType;

        let log_dir = Path::new("/repo/.git/wt/logs");

        // Hook: create the same way hooks.rs does, parse the same way state.rs does
        let created = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        let parsed = HookLog::parse("user:post-start:server").unwrap();
        assert_eq!(created.path(log_dir, "main"), parsed.path(log_dir, "main"));

        // Internal: create the same way handlers.rs does, parse from CLI
        let created = HookLog::internal(InternalOp::Remove);
        let parsed = HookLog::parse("internal:remove").unwrap();
        assert_eq!(created.path(log_dir, "main"), parsed.path(log_dir, "main"));
    }

    #[test]
    fn test_hook_log_to_spec_roundtrip() {
        use worktrunk::git::HookType;

        // Hook roundtrip: to_spec -> parse -> equals original
        let original = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        let spec = original.to_spec();
        assert_eq!(spec, "user:post-start:server");
        let parsed = HookLog::parse(&spec).unwrap();
        assert_eq!(original, parsed);

        // Project hook
        let original = HookLog::hook(HookSource::Project, HookType::PreMerge, "lint");
        let spec = original.to_spec();
        assert_eq!(spec, "project:pre-merge:lint");
        let parsed = HookLog::parse(&spec).unwrap();
        assert_eq!(original, parsed);

        // Internal roundtrip
        let original = HookLog::internal(InternalOp::Remove);
        let spec = original.to_spec();
        assert_eq!(spec, "internal:remove");
        let parsed = HookLog::parse(&spec).unwrap();
        assert_eq!(original, parsed);

        // Trash sweep roundtrip
        let original = HookLog::internal(InternalOp::TrashSweep);
        let spec = original.to_spec();
        assert_eq!(spec, "internal:trash-sweep");
        let parsed = HookLog::parse(&spec).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_parse_trash_entry_timestamp() {
        // Simple name with trailing timestamp
        assert_eq!(
            parse_trash_entry_timestamp("feature-1700000000"),
            Some(1700000000)
        );
        // Worktree name containing hyphens — split from the right
        assert_eq!(
            parse_trash_entry_timestamp("my-project.feature-branch-1700000000"),
            Some(1700000000)
        );
        // Missing separator or non-numeric suffix → None (sweep leaves it alone)
        assert_eq!(parse_trash_entry_timestamp("no-timestamp"), None);
        assert_eq!(parse_trash_entry_timestamp("notimestamp"), None);
        assert_eq!(parse_trash_entry_timestamp(""), None);
    }

    #[test]
    fn test_collect_stale_trash_entries() {
        let trash = tempfile::tempdir().unwrap();
        let now: u64 = 1_700_000_000;
        let day = TRASH_STALE_THRESHOLD_SECS;

        // Stale: 2 days old
        let stale = trash.path().join(format!("feature-old-{}", now - 2 * day));
        fs::create_dir(&stale).unwrap();
        // Fresh: 1 hour old
        let fresh = trash.path().join(format!("feature-new-{}", now - 3600));
        fs::create_dir(&fresh).unwrap();
        // Exactly at threshold: 1 day old (inclusive)
        let boundary = trash.path().join(format!("feature-edge-{}", now - day));
        fs::create_dir(&boundary).unwrap();
        // Unparsable: no timestamp suffix — sweep ignores it
        let foreign = trash.path().join("random-folder");
        fs::create_dir(&foreign).unwrap();

        let mut collected = collect_stale_trash_entries(trash.path(), now, day);
        collected.sort();
        let mut expected = vec![stale, boundary];
        expected.sort();
        assert_eq!(collected, expected);
        assert!(
            fresh.exists(),
            "fresh entries must not appear in stale list"
        );
        assert!(foreign.exists(), "unparsable entries must be left alone");
    }

    #[test]
    fn test_collect_stale_trash_entries_missing_dir() {
        let missing = std::path::PathBuf::from("/nonexistent/wt/trash/path");
        assert!(
            collect_stale_trash_entries(&missing, 1_700_000_000, TRASH_STALE_THRESHOLD_SECS)
                .is_empty()
        );
    }
}
