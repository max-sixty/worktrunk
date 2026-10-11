use anyhow::Context;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::process::Stdio;
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
    /// Delayed cleanup of the removed current worktree's empty PWD placeholder.
    RemovePlaceholder,
    /// Background cleanup of staged worktrees and unregistered metadata
    TrashSweep,
}

/// Specification for a hook log file.
///
/// This is the single source of truth for hook log file paths.
/// Used by log creation in `spawn_detached` to place background hook output.
///
/// All hook output is centralized under the main worktree's `.git/wt/logs/`
/// directory (`Repository::wt_logs_dir()`); per-branch logs live in subtrees,
/// and rerunning the same operation on the same branch overwrites its previous
/// log. The complementary categorization rule — top-level *files* are shared
/// logs, top-level *directories* are per-branch trees — is the "Log layout
/// invariant" documented in `commands::config::state`.
///
/// # Log file layout
///
/// Hook commands produce logs at: `{branch}/{source}/{hook-type}/{name}.log`
/// (`source` is `user` or `project`)
/// - Example: `feature/user/post-start/server.log`
///
/// Per-branch internal operations produce logs at: `{branch}/internal/{op}.log`
/// - Example: `feature/internal/remove.log`
///
/// Repo-wide (branch-agnostic) internal operations produce top-level logs at
/// `internal-{op}.log`, alongside the other shared logs (`commands.jsonl`,
/// `trace.log`).
/// - Example: `internal-trash-sweep.log`
///
/// Branch and hook names are sanitized for filesystem safety via
/// `sanitize_for_filename`. Already-safe names pass through unchanged; names
/// containing invalid characters have them replaced and a short
/// collision-avoidance hash appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookLog {
    /// Hook command log: `{branch}/{source}/{hook-type}/{name}.log`
    Hook {
        source: HookSource,
        hook_type: HookType,
        name: String,
    },
    /// Per-branch internal operation log: `{branch}/internal/{op}.log`
    Internal(InternalOp),
    /// Repo-wide internal operation log: `internal-{op}.log` (no branch segment).
    Shared(InternalOp),
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

    /// Generate the full log path for a branch in the given log directory.
    ///
    /// Builds the nested path under `{log_dir}/{sanitized-branch}/...` for
    /// per-branch variants. The `Shared` variant ignores `branch` and writes
    /// directly under `{log_dir}` as `internal-{op}.log`.
    /// Parent directories must be created by the caller (see `create_detach_log`).
    pub fn path(&self, log_dir: &Path, branch: &str) -> PathBuf {
        match self {
            HookLog::Hook {
                source,
                hook_type,
                name,
            } => log_dir
                .join(sanitize_for_filename(branch))
                .join(source.to_string())
                .join(hook_type.to_string())
                .join(format!("{}.log", sanitize_for_filename(name))),
            HookLog::Internal(op) => log_dir
                .join(sanitize_for_filename(branch))
                .join("internal")
                .join(format!("{op}.log")),
            HookLog::Shared(op) => log_dir.join(format!("internal-{op}.log")),
        }
    }
}

/// Get the separator needed before closing brace in POSIX shell command grouping.
/// Returns empty string if command already ends with newline or semicolon.
///
/// Unix-only: the `{ …; } &` grouping it feeds is `spawn_detached_unix`'s
/// backgrounding wrapper, which the Windows spawn has no counterpart for.
#[cfg(unix)]
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

/// Spawn a detached background process with output redirected to a log file.
///
/// The process will be fully detached from the parent:
/// - On Unix: uses `process_group(0)` to create a new process group (survives PTY closure)
/// - On Windows: uses `CREATE_NEW_PROCESS_GROUP` to detach from console
///
/// Internal ops (`HookLog::Internal` and `HookLog::Shared`) are run at lowered
/// priority via [`worktrunk::priority::command`] so their I/O and CPU don't
/// compete with user-visible work; user hooks run at normal priority.
///
/// Logs are centralized in the main worktree's `.git/wt/logs/` directory.
pub fn spawn_detached(
    repo: &Repository,
    worktree_path: &Path,
    command: &str,
    branch: &str,
    hook_log: &HookLog,
) -> anyhow::Result<std::path::PathBuf> {
    let (log_path, log_file) = create_detach_log(repo, branch, hook_log)?;

    tracing::debug!(
        command = %command,
        "$ {} (detached, logging to {})",
        command,
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    #[cfg(unix)]
    {
        let low_priority = matches!(hook_log, HookLog::Internal(_) | HookLog::Shared(_));
        spawn_detached_unix(worktree_path, command, log_file, low_priority)?;
    }

    #[cfg(windows)]
    {
        spawn_detached_windows(worktree_path, command, log_file)?;
    }

    Ok(log_path)
}

#[cfg(unix)]
fn spawn_detached_unix(
    worktree_path: &Path,
    command: &str,
    log_file: fs::File,
    low_priority: bool,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    // Wrap in braces so `&` backgrounds the entire compound command.
    // Without braces, `cmd1 && cmd2; cmd3 &` parses as two statements:
    // `cmd1 && cmd2` (foreground) then `cmd3 &` (background) — the semicolon
    // has lower precedence than `&`, so only the last segment is backgrounded.
    let shell_cmd = format!("{{ {}{} }} &", command, posix_command_separator(command));

    // Detachment via process_group(0): puts the spawned shell in its own process group.
    // When the controlling PTY closes, SIGHUP is sent to the foreground process group.
    // Since our process is in a different group, it doesn't receive the signal.
    //
    // For low-priority ops (internal cleanup), wrap the shell via
    // `worktrunk::priority::command`. The policy is inherited by the backgrounded
    // command and its grandchildren.
    let mut cmd = worktrunk::priority::command("sh", low_priority);
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
    let mut child =
        worktrunk::shell_exec::spawn(&mut cmd).context("Failed to spawn detached process")?;

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
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use worktrunk::shell_exec::ShellConfig;

    // CREATE_NEW_PROCESS_GROUP: Creates new process group (0x00000200)
    // CREATE_NO_WINDOW: Creates process without console window (0x08000000)
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let shell = ShellConfig::get()?;

    // Git Bash runs the command as written; no stdin wrapper is needed.
    let mut cmd = shell.command(command);

    cmd.current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    // Prevent hooks from writing to the directive file
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    worktrunk::shell_exec::spawn(&mut cmd).context("Failed to spawn detached process")?;

    // Windows: Process is fully detached with a hidden window via CREATE_NO_WINDOW flag,
    // no need to wait (unlike Unix which waits for the outer shell)

    Ok(())
}

/// Spawn a detached background process by executing a binary directly.
///
/// Unlike [`spawn_detached`] (which wraps a shell command in `sh -c`), this
/// spawns the executable without an intermediate shell. Complete input is
/// prepared by [`worktrunk::shell_exec::buffered_stdin`] before spawn.
///
/// Used for structured child processes like `wt hook run-pipeline` where the parent
/// passes data via stdin rather than through shell arguments.
///
/// When `hook_log` is [`HookLog::Hook`], the spawn is treated as a background
/// hook pipeline: the child (and every process it later spawns) receives
/// [`worktrunk::priority::FOREGROUND_ENV_VAR`] =
/// [`worktrunk::priority::BACKGROUND_HOOK_VALUE`] so nested `wt` invocations
/// can tell they're running inside a background hook.
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

    tracing::debug!(
        program = %program.display(),
        "$ {} {} (detached, logging to {})",
        program.display(),
        args.join(" "),
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    let is_background_hook = matches!(hook_log, HookLog::Hook { .. });

    #[cfg(unix)]
    {
        let low_priority = matches!(hook_log, HookLog::Internal(_) | HookLog::Shared(_));
        spawn_detached_exec_unix(
            worktree_path,
            program,
            args,
            log_file,
            stdin_bytes,
            low_priority,
            is_background_hook,
        )?;
    }

    #[cfg(windows)]
    {
        spawn_detached_exec_windows(
            worktree_path,
            program,
            args,
            log_file,
            stdin_bytes,
            is_background_hook,
        )?;
    }

    Ok(log_path)
}

/// Apply [`worktrunk::priority::FOREGROUND_ENV_VAR`] to a
/// [`std::process::Command`] when spawning a background hook pipeline.
fn set_background_hook_env(cmd: &mut std::process::Command, is_background_hook: bool) {
    if is_background_hook {
        cmd.env(
            worktrunk::priority::FOREGROUND_ENV_VAR,
            worktrunk::priority::BACKGROUND_HOOK_VALUE,
        );
    }
}

#[cfg(unix)]
fn spawn_detached_exec_unix(
    worktree_path: &Path,
    program: &Path,
    args: &[&str],
    log_file: fs::File,
    stdin_bytes: &[u8],
    low_priority: bool,
    is_background_hook: bool,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    // See [`worktrunk::priority`] for the priority-lowering rationale.
    let mut cmd = worktrunk::priority::command(program, low_priority);
    cmd.args(args)
        .current_dir(worktree_path)
        .stdin(
            worktrunk::shell_exec::buffered_stdin(stdin_bytes)
                .context("Failed to prepare detached input")?,
        )
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .process_group(0);
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    set_background_hook_env(&mut cmd, is_background_hook);
    worktrunk::shell_exec::spawn(&mut cmd).context("Failed to spawn detached process")?;

    Ok(())
}

#[cfg(windows)]
fn spawn_detached_exec_windows(
    worktree_path: &Path,
    program: &Path,
    args: &[&str],
    log_file: fs::File,
    stdin_bytes: &[u8],
    is_background_hook: bool,
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(worktree_path)
        .stdin(
            worktrunk::shell_exec::buffered_stdin(stdin_bytes)
                .context("Failed to prepare detached input")?,
        )
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    set_background_hook_env(&mut cmd, is_background_hook);
    worktrunk::shell_exec::spawn(&mut cmd).context("Failed to spawn detached process")?;

    Ok(())
}

/// Repo-wide internal cleanup op, run fire-and-forget after `wt remove`'s
/// primary user-visible output.
///
/// This is worktrunk's opportunistic janitor: it reclaims resources orphaned
/// by interrupted or out-of-band operations. It runs on the `wt remove`
/// cadence (not a timer, not a user command, not a read-only command), and
/// after primary output so it can never delay a user-visible message. Every
/// step is best-effort and additive — failures log at debug level and the
/// `wt remove` operation proceeds regardless.
///
/// It DOES delay process exit — the shell wrapper waits on it. Enumeration is
/// two spawns, one `pgrep` plus one `lsof` over the whole daemon PID set, flat
/// in the machine-wide daemon count. Any live daemon then costs a
/// `git worktree list`, and a classified orphan a `SIGTERM`→`SIGKILL` wait
/// bounded by `REAP_KILL_DEADLINE` — both flat in the daemon count too (see
/// the `internal-sweep` / `enumerate-fsmonitor-daemons` trace spans and
/// benches/AGENTS.md § Recording `wt remove` / `wt step prune` staging).
///
/// Steps:
///
/// 1. [`sweep_stale_trash`] — delete stale payload and metadata disposal
///    entries left by interrupted or incomplete cleanup.
/// 2. [`worktrunk::git::fsmonitor::reap_orphan_fsmonitor_daemons`] — terminate
///    `git fsmonitor--daemon` processes whose worktree no longer exists.
///    Defense-in-depth for daemons orphaned by paths that bypass `wt remove`
///    (plain `git worktree remove`, manual `rm -rf`, a crashed `wt`); the
///    `wt remove` source itself stops the daemon synchronously.
pub fn run_internal_sweep(repo: &Repository) {
    let _span = worktrunk::trace::Span::new("internal-sweep");
    sweep_stale_trash(repo);
    worktrunk::git::fsmonitor::reap_orphan_fsmonitor_daemons(repo);
}

/// How old a disposal entry must be before [`sweep_stale_trash`] deletes it.
pub const TRASH_STALE_THRESHOLD_SECS: u64 = 24 * 60 * 60;

/// Fire-and-forget cleanup of staged worktrees and unregistered metadata.
///
/// Worktree removal uses a fast path that renames the worktree into
/// `.git/wt/trash/<name>-<timestamp>/` and deletes it in a detached background
/// process. If that process is interrupted (SIGKILL, reboot, disk full), the
/// renamed directory is orphaned. Metadata unregister uses timestamped temporary
/// directories with [`Repository::UNREGISTERED_WORKTREE_PREFIX`] directly in
/// the Git common dir, so a blocked payload-trash path cannot prevent removal.
/// Those directories also become disposable after the atomic unregister move.
/// `wt remove` calls this function after its
/// primary user-visible output — so the sweep never delays the progress or
/// success message — to provide eventual cleanup: entries older than
/// [`TRASH_STALE_THRESHOLD_SECS`] are removed by a single detached `rm -rf`.
///
/// Best effort: directory read failures and spawn failures are logged at debug
/// level and otherwise ignored. The sweep is purely additive — the primary
/// `wt remove` operation proceeds regardless of outcome.
pub fn sweep_stale_trash(repo: &Repository) {
    let stale = collect_stale_trash_entries(repo, epoch_now(), TRASH_STALE_THRESHOLD_SECS);
    if stale.is_empty() {
        return;
    }

    let command = match build_trash_sweep_command(&stale) {
        Ok(command) => command,
        Err(error) => {
            tracing::debug!(%error, "Failed to build disposal sweep command");
            return;
        }
    };

    // The sweep is repo-wide (not branch-scoped), so it logs to a top-level
    // shared file alongside `commands.jsonl` and `trace.log`. The branch
    // argument is ignored for the `Shared` variant.
    if let Err(e) = spawn_detached(
        repo,
        &repo.wt_dir(),
        &command,
        "",
        &HookLog::Shared(InternalOp::TrashSweep),
    ) {
        tracing::debug!(error = %e, "Failed to spawn stale trash sweep: {e}");
    }
}

/// Build the `rm -rf -- …` command that [`sweep_stale_trash`] hands to
/// [`spawn_detached`]. All entries are joined into a single invocation so the
/// sweep spawns one background process regardless of how many stale entries
/// exist; each path is POSIX-escaped so directories with spaces or shell
/// metacharacters round-trip safely through the wrapping `sh -c`.
fn build_trash_sweep_command(paths: &[PathBuf]) -> anyhow::Result<String> {
    let escaped: Vec<_> = paths
        .iter()
        .map(path_shell_argument)
        .collect::<anyhow::Result<_>>()?;
    Ok(format!("rm -rf -- {}", escaped.join(" ")))
}

/// Shell command strings must preserve the chosen path exactly. Refuse native
/// paths that cannot be represented instead of targeting a lossy replacement.
fn path_shell_argument(path: impl AsRef<Path>) -> anyhow::Result<String> {
    let path = path
        .as_ref()
        .to_str()
        .context("Cannot build a removal command for a non-UTF-8 path")?;
    Ok(shell_escape::unix::escape(path.into()).into_owned())
}

/// The janitor applies its age policy to the same inventory state get/clear use.
/// A failed inventory leaves disposal alone; primary removal still succeeds.
fn collect_stale_trash_entries(repo: &Repository, now: u64, threshold_secs: u64) -> Vec<PathBuf> {
    let entries = match repo.disposal_entries() {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!(%error, "Failed to inspect worktree disposal");
            return Vec::new();
        }
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let timestamp = entry.staged_at?;
            (now.saturating_sub(timestamp) >= threshold_secs).then_some(entry.path)
        })
        .collect()
}

/// Remove an already-staged worktree's trash independently of branch deletion.
///
/// The worktree has been renamed and its registry entry pruned. This command
/// can start immediately; it never touches the original worktree path.
pub fn build_remove_command_staged(staged_path: &Path) -> anyhow::Result<String> {
    Ok(format!("rm -rf -- {}", path_shell_argument(staged_path)?))
}

/// Remove the empty shell-PWD placeholder after synchronous removal finishes.
///
/// The one-second delay gives the shell wrapper time to consume its cd
/// directive. `rmdir` preserves any files written into the original path.
pub fn build_remove_placeholder_command(original_path: &Path) -> anyhow::Result<String> {
    Ok(format!(
        "sleep 1 && rmdir -- {} 2>/dev/null",
        path_shell_argument(original_path)?
    ))
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
) -> anyhow::Result<String> {
    use shell_escape::unix::escape;

    let worktree_escaped = path_shell_argument(worktree_path)?;

    let force_flag = if force_worktree { " --force" } else { "" };

    // The fsmonitor daemon is stopped (and force-killed if wedged) synchronously
    // in the Rust foreground by `stop_fsmonitor_daemon`, before this command is
    // built — see `spawn_background_removal`. The detached process only does the
    // `git worktree remove` / `rm -rf`.
    //
    // When removing the current worktree, delay so the shell wrapper can cd away
    // before the directory is removed. The primary fix for the "shell-init: error
    // retrieving current directory" race is in the fish wrapper (using builtins
    // instead of subprocesses to read the directive), but this provides defense in
    // depth for other shells and edge cases.
    let prefix = if changed_directory {
        "sleep 1 && ".to_string()
    } else {
        String::new()
    };

    Ok(match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{}git worktree remove{} {} && git branch -D -- {}",
                prefix, force_flag, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{}git worktree remove{} {}",
                prefix, force_flag, worktree_escaped
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use path_slash::PathExt as _;

    use super::*;

    #[test]
    fn test_sanitize_for_filename() {
        // Path separators, Windows-illegal characters, multiple special chars,
        // already-safe names, and reserved prefix names
        assert_snapshot!(
            [
                ("path separator /", sanitize_for_filename("feature/branch")),
                (r"path separator \", sanitize_for_filename(r"feature\branch")),
                ("colon", sanitize_for_filename("bug:123")),
                ("angle brackets", sanitize_for_filename("fix<angle>")),
                ("pipe", sanitize_for_filename("fix|pipe")),
                ("question mark", sanitize_for_filename("fix?question")),
                ("wildcard", sanitize_for_filename("fix*wildcard")),
                ("quotes", sanitize_for_filename(r#"fix"quotes""#)),
                ("multiple special", sanitize_for_filename(r#"a/b\c<d>e:f"g|h?i*j"#)),
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
        already safe: normal-branch
        underscore: branch_with_underscore
        reserved prefix CONSOLE: CONSOLE
        reserved prefix COM10: COM10
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
    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn test_removal_builders_refuse_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let temp = tempfile::tempdir().unwrap();
        let native = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"worktree-\xff".to_vec()));
        let replacement = temp.path().join("worktree-�");
        fs::create_dir(&replacement).unwrap();
        assert_eq!(native.to_string_lossy(), replacement.to_string_lossy());
        assert!(build_trash_sweep_command(std::slice::from_ref(&native)).is_err());
        assert!(build_remove_command_staged(&native).is_err());
        assert!(build_remove_placeholder_command(&native).is_err());
        assert!(build_remove_command(&native, Some("feature"), false, false).is_err());
        assert!(native.to_str().is_none());
        assert!(replacement.is_dir());
    }

    #[test]
    fn test_build_remove_command() {
        use std::path::PathBuf;

        let path = PathBuf::from("/tmp/test-worktree");

        // changed_directory=true: sleep before removal
        assert_snapshot!(build_remove_command(&path, None, false, true).unwrap(), @"sleep 1 && git worktree remove /tmp/test-worktree");
        assert_snapshot!(build_remove_command(&path, Some("feature-branch"), false, true).unwrap(), @"sleep 1 && git worktree remove /tmp/test-worktree && git branch -D -- feature-branch");

        // changed_directory=false: no sleep
        assert_snapshot!(build_remove_command(&path, None, false, false).unwrap(), @"git worktree remove /tmp/test-worktree");
        assert_snapshot!(build_remove_command(&path, Some("feature-branch"), false, false).unwrap(), @"git worktree remove /tmp/test-worktree && git branch -D -- feature-branch");

        // With force flag
        assert_snapshot!(build_remove_command(&path, None, true, true).unwrap(), @"sleep 1 && git worktree remove --force /tmp/test-worktree");

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        assert_snapshot!(build_remove_command(&special_path, Some("feature/branch"), false, true).unwrap(), @"sleep 1 && git worktree remove '/tmp/test worktree' && git branch -D -- feature/branch");
    }

    #[test]
    fn test_build_remove_command_staged() {
        let staged_path = PathBuf::from("/tmp/repo/.git/wt/trash/my-project.feature-1234567890");
        let original_path = PathBuf::from("/tmp/my-project.feature");
        assert_snapshot!(build_remove_command_staged(&staged_path).unwrap(), @"rm -rf -- /tmp/repo/.git/wt/trash/my-project.feature-1234567890");
        assert_snapshot!(build_remove_placeholder_command(&original_path).unwrap(), @"sleep 1 && rmdir -- /tmp/my-project.feature 2>/dev/null");

        let special_path = PathBuf::from("/tmp/repo/.git/wt/trash/test worktree-123");
        let special_original = PathBuf::from("/tmp/test worktree");
        assert_snapshot!(build_remove_command_staged(&special_path).unwrap(), @"rm -rf -- '/tmp/repo/.git/wt/trash/test worktree-123'");
        assert_snapshot!(build_remove_placeholder_command(&special_original).unwrap(), @"sleep 1 && rmdir -- '/tmp/test worktree' 2>/dev/null");
    }

    #[test]
    fn test_build_trash_sweep_command() {
        // Empty list still produces a well-formed command — the caller
        // (`sweep_stale_trash`) bails before we get here, but the helper itself
        // must not panic on an empty slice.
        assert_snapshot!(build_trash_sweep_command(&[]).unwrap(), @"rm -rf -- ");

        // Plain paths — joined with spaces, no quoting.
        let paths = [
            PathBuf::from("/tmp/repo/.git/wt/trash/feature-1700000000"),
            PathBuf::from("/tmp/repo/.git/wt/trash/bugfix-1700000100"),
        ];
        assert_snapshot!(
            build_trash_sweep_command(&paths).unwrap(),
            @"rm -rf -- /tmp/repo/.git/wt/trash/feature-1700000000 /tmp/repo/.git/wt/trash/bugfix-1700000100"
        );

        // Shell metacharacters — POSIX single-quote escaping isolates each path
        // so the wrapping `sh -c` reads them as literal arguments. The embedded
        // single quote uses the standard `'\''` idiom. This is the regression
        // guard for the platform/MSYSTEM-sensitive `shell_escape::escape` we
        // replaced: under cmd.exe quoting `$(echo pwned)` would still execute
        // when spliced into a POSIX shell.
        let nasty = [
            PathBuf::from("/tmp/trash/with space-1"),
            PathBuf::from("/tmp/trash/$(echo pwned)-2"),
            PathBuf::from("/tmp/trash/a'b-3"),
        ];
        assert_snapshot!(
            build_trash_sweep_command(&nasty).unwrap(),
            @"rm -rf -- '/tmp/trash/with space-1' '/tmp/trash/$(echo pwned)-2' '/tmp/trash/a'\\''b-3'"
        );
    }

    #[test]
    fn test_hook_log_path() {
        use worktrunk::git::HookType;

        let log_dir = Path::new("/repo/.git/wt/logs");

        // Hook path: {log_dir}/{sanitized-branch}/{source}/{hook-type}/{sanitized-name}.log
        let log = HookLog::hook(HookSource::User, HookType::PostCreate, "server");
        assert_snapshot!(
            log.path(log_dir, "main").to_slash_lossy(),
            @"/repo/.git/wt/logs/main/user/post-start/server.log"
        );

        // Slash in branch name gets sanitized (feature/auth → feature-auth-{hash})
        assert_snapshot!(
            log.path(log_dir, "feature/auth").to_slash_lossy(),
            @"/repo/.git/wt/logs/feature-auth-j34/user/post-start/server.log"
        );

        // Project source
        let log = HookLog::hook(HookSource::Project, HookType::PreCreate, "build");
        assert_snapshot!(
            log.path(log_dir, "main").to_slash_lossy(),
            @"/repo/.git/wt/logs/main/project/pre-start/build.log"
        );

        // Per-branch internal operation path: {log_dir}/{sanitized-branch}/internal/{op}.log
        assert_snapshot!(
            HookLog::Internal(InternalOp::Remove)
                .path(log_dir, "main")
                .to_slash_lossy(),
            @"/repo/.git/wt/logs/main/internal/remove.log"
        );

        // Repo-wide (branch-agnostic) internal operation path:
        // {log_dir}/internal-{op}.log — the branch argument is ignored.
        assert_snapshot!(
            HookLog::Shared(InternalOp::TrashSweep)
                .path(log_dir, "anything")
                .to_slash_lossy(),
            @"/repo/.git/wt/logs/internal-trash-sweep.log"
        );
    }

    #[test]
    fn test_collect_stale_trash_entries() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let trash = repo.wt_trash_dir();
        fs::create_dir_all(&trash).unwrap();
        let now: u64 = 1_700_000_000;
        let day = TRASH_STALE_THRESHOLD_SECS;

        // Stale: 2 days old
        let stale = trash.join(format!("feature-old-{}", now - 2 * day));
        fs::create_dir(&stale).unwrap();
        // Fresh: 1 hour old
        let fresh = trash.join(format!("feature-new-{}", now - 3600));
        fs::create_dir(&fresh).unwrap();
        // Exactly at threshold: 1 day old (inclusive)
        let boundary = trash.join(format!("feature-edge-{}", now - day));
        fs::create_dir(&boundary).unwrap();
        // Unparsable: no timestamp suffix — sweep ignores it
        let foreign = trash.join("random-folder");
        fs::create_dir(&foreign).unwrap();

        let mut collected = collect_stale_trash_entries(&repo, now, day);
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

    /// The common directory is an external namespace: delete only the owned
    /// metadata prefix, and never follow a prefixed symlink into other data.
    #[cfg(unix)]
    #[test]
    fn test_metadata_sweep_preserves_other_git_entries_and_symlink_targets() {
        use worktrunk::shell_exec::Cmd;
        let mut test = worktrunk::testing::TestRepo::with_initial_commit();
        let live = test.add_worktree("live-disposal-control");
        let repo = Repository::at(test.root_path()).unwrap();
        let common = repo.git_common_dir();
        let outside = tempfile::tempdir().unwrap();
        let now = 1_700_000_000;
        let old = now - 2 * TRASH_STALE_THRESHOLD_SECS;
        let prefix = Repository::UNREGISTERED_WORKTREE_PREFIX;
        let disposable = common.join(format!("{prefix}random-{old}"));
        fs::create_dir(&disposable).unwrap();
        fs::write(disposable.join("index"), "committed disposal").unwrap();
        let foreign = common.join(format!("foreign-{old}"));
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("index"), "unrelated data").unwrap();
        let live_index = repo.worktree_at(&live).git_dir().unwrap().join("index");
        let index_contents = fs::read(&live_index).unwrap();
        fs::write(outside.path().join("payload"), "outside data").unwrap();
        let link = common.join(format!("{prefix}symlink-{old}"));
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let stale = collect_stale_trash_entries(&repo, now, TRASH_STALE_THRESHOLD_SECS);
        assert_eq!(stale.len(), 2);
        let output = Cmd::new("sh")
            .args(["-c".to_string(), build_trash_sweep_command(&stale).unwrap()])
            .run()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!disposable.exists());
        assert_eq!(
            fs::symlink_metadata(link).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("payload")).unwrap(),
            "outside data"
        );
        assert_eq!(
            fs::read_to_string(foreign.join("index")).unwrap(),
            "unrelated data"
        );
        assert_eq!(fs::read(live_index).unwrap(), index_contents);
    }

    #[test]
    fn test_collect_stale_trash_entries_missing_dir() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        assert!(
            collect_stale_trash_entries(&repo, 1_700_000_000, TRASH_STALE_THRESHOLD_SECS)
                .is_empty()
        );
    }
}
