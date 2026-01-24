use anyhow::Context;
use color_print::cformat;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use strum::IntoEnumIterator;
use worktrunk::git::{HookType, Repository};
use worktrunk::path::{format_path_for_display, sanitize_for_filename};

use crate::commands::hook_filter::HookSource;

// ==================== Hook Log Specification ====================

/// Internal worktrunk operations that produce log files.
///
/// These are operations performed by worktrunk itself (not user-defined hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub enum InternalOp {
    /// Background worktree removal (`wt remove` in background mode)
    Remove,
}

/// Specification for a hook log file.
///
/// This is the single source of truth for hook log file naming.
/// Used by both log creation (in `spawn_detached`) and log lookup (in `handle_logs_get`).
///
/// # Log file naming
///
/// Hook commands produce logs named: `{branch}-{source}-{hook_type}-{name}.log`
/// - Example: `feature-user-post-start-server.log`
///
/// Internal operations produce logs named: `{branch}-{op}.log`
/// - Example: `feature-remove.log`
///
/// # CLI format for lookup
///
/// The first segment determines the log type:
/// - `user:hook-type:name` → User hook (e.g., `user:post-start:server`)
/// - `project:hook-type:name` → Project hook (e.g., `project:post-create:build`)
/// - `internal:op` → Internal operation (e.g., `internal:remove`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookLog {
    /// Hook command log: `{branch}-{source}-{hook_type}-{name}.log`
    Hook {
        source: HookSource,
        hook_type: HookType,
        name: String,
    },
    /// Internal operation log: `{branch}-{op}.log`
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

    /// Generate the suffix (without branch) for the log filename.
    ///
    /// This is what gets appended after `{branch}-` in the log filename.
    pub fn suffix(&self) -> String {
        match self {
            HookLog::Hook {
                source,
                hook_type,
                name,
            } => {
                // HookSource uses #[strum(serialize_all = "kebab-case")] which produces lowercase
                format!("{}-{}-{}", source, hook_type, sanitize_for_filename(name))
            }
            HookLog::Internal(op) => op.to_string(),
        }
    }

    /// Generate full log filename for a branch.
    pub fn filename(&self, branch: &str) -> String {
        let safe_branch = sanitize_for_filename(branch);
        format!("{}-{}.log", safe_branch, self.suffix())
    }

    /// Generate full log path for a branch in the given log directory.
    pub fn path(&self, log_dir: &Path, branch: &str) -> PathBuf {
        log_dir.join(self.filename(branch))
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
                    cformat!(
                        "Unknown internal operation: <bold>{}</>. Valid: remove",
                        op_str
                    )
                })?;
                Ok(Self::Internal(op))
            }
            // source:hook-type:name
            [source_str, hook_type_str, name] => {
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

/// Spawn a detached background process with output redirected to a log file
///
/// The process will be fully detached from the parent:
/// - On Unix: uses process_group(0) to create a new process group (survives PTY closure)
/// - On Windows: uses CREATE_NEW_PROCESS_GROUP to detach from console
///
/// Logs are centralized in the main worktree's `.git/wt-logs/` directory.
///
/// # Arguments
/// * `repo` - Repository instance for accessing git common directory
/// * `worktree_path` - Working directory for the command
/// * `command` - Shell command to execute
/// * `branch` - Branch name for log organization
/// * `hook_log` - Log specification (determines the log filename)
/// * `context_json` - Optional JSON context to pipe to command's stdin
///
/// # Returns
/// Path to the log file where output is being written
pub fn spawn_detached(
    repo: &Repository,
    worktree_path: &Path,
    command: &str,
    branch: &str,
    hook_log: &HookLog,
    context_json: Option<&str>,
) -> anyhow::Result<std::path::PathBuf> {
    // Create log directory in the common git directory
    let log_dir = repo.wt_logs_dir();
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "Failed to create log directory {}",
            format_path_for_display(&log_dir)
        )
    })?;

    // Generate log path using the HookLog specification
    let log_path = hook_log.path(&log_dir, branch);

    // Create log file
    let log_file = fs::File::create(&log_path).with_context(|| {
        format!(
            "Failed to create log file {}",
            format_path_for_display(&log_path)
        )
    })?;

    log::debug!(
        "$ {} (detached, logging to {})",
        command,
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    #[cfg(unix)]
    {
        spawn_detached_unix(worktree_path, command, log_file, context_json)?;
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

    let shell_cmd = format!("{} &", full_command);

    // Detachment via process_group(0): puts the spawned shell in its own process group.
    // When the controlling PTY closes, SIGHUP is sent to the foreground process group.
    // Since our process is in a different group, it doesn't receive the signal.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&shell_cmd)
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .context("Failed to clone log file handle")?,
        ))
        .stderr(Stdio::from(log_file))
        // Prevent hooks from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .process_group(0) // New process group, not in PTY's foreground group
        .spawn()
        .context("Failed to spawn detached process")?;

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

    let shell = ShellConfig::get();

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
        // Prevent hooks from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .context("Failed to spawn detached process")?;

    // Windows: Process is fully detached via DETACHED_PROCESS flag,
    // no need to wait (unlike Unix which waits for the outer shell)

    Ok(())
}

/// Build shell command for background worktree removal
///
/// `branch_to_delete` is the branch to delete after removing the worktree.
/// Pass `None` for detached HEAD or when branch should be retained.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
///
/// `force_worktree` adds `--force` to `git worktree remove`, allowing removal
/// even when the worktree contains untracked files (like build artifacts).
pub fn build_remove_command(
    worktree_path: &std::path::Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());

    // TODO: This delay is a timing-based workaround, not a principled fix.
    // The race: after wt exits, the shell wrapper reads the directive file and
    // runs `cd`. But fish (and other shells) may call getcwd() before the cd
    // completes (e.g., for prompt updates), and if the background removal has
    // already deleted the directory, we get "shell-init: error retrieving current
    // directory". A 1s delay is very conservative (shell cd takes ~1-5ms), but
    // deterministic solutions (shell-spawned background, marker file sync) add
    // significant complexity for marginal benefit.
    let delay = "sleep 1";

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "git -C {} fsmonitor--daemon stop 2>/dev/null || true",
        worktree_escaped
    );

    let force_flag = if force_worktree { " --force" } else { "" };

    match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{} && {} && git worktree remove{} {} && git branch -D {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{} && {} && git worktree remove{} {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_for_filename() {
        // Path separators (hash suffix appended)
        assert!(sanitize_for_filename("feature/branch").starts_with("feature-branch-"));
        assert!(sanitize_for_filename("feature\\branch").starts_with("feature-branch-"));

        // Windows-illegal characters
        assert!(sanitize_for_filename("bug:123").starts_with("bug-123-"));
        assert!(sanitize_for_filename("fix<angle>").starts_with("fix-angle-"));
        assert!(sanitize_for_filename("fix|pipe").starts_with("fix-pipe-"));
        assert!(sanitize_for_filename("fix?question").starts_with("fix-question-"));
        assert!(sanitize_for_filename("fix*wildcard").starts_with("fix-wildcard-"));
        assert!(sanitize_for_filename("fix\"quotes\"").starts_with("fix-quotes-"));

        // Multiple special characters
        assert!(sanitize_for_filename("a/b\\c<d>e:f\"g|h?i*j").starts_with("a-b-c-d-e-f-g-h-i-j-"));

        // Already safe (still gets hash suffix)
        assert!(sanitize_for_filename("normal-branch").starts_with("normal-branch-"));
        assert!(
            sanitize_for_filename("branch_with_underscore").starts_with("branch_with_underscore-")
        );

        // Windows reserved device names (must be prefixed to avoid conflicts)
        assert!(sanitize_for_filename("CON").starts_with("_CON-"));
        assert!(sanitize_for_filename("con").starts_with("_con-"));
        assert!(sanitize_for_filename("PRN").starts_with("_PRN-"));
        assert!(sanitize_for_filename("AUX").starts_with("_AUX-"));
        assert!(sanitize_for_filename("NUL").starts_with("_NUL-"));
        assert!(sanitize_for_filename("COM1").starts_with("_COM1-"));
        assert!(sanitize_for_filename("com9").starts_with("_com9-"));
        assert!(sanitize_for_filename("LPT1").starts_with("_LPT1-"));
        assert!(sanitize_for_filename("lpt9").starts_with("_lpt9-"));

        // COM0/LPT0 are NOT reserved (only 1-9 are)
        assert!(sanitize_for_filename("COM0").starts_with("COM0-"));
        assert!(sanitize_for_filename("LPT0").starts_with("LPT0-"));

        // Longer names are fine
        assert!(sanitize_for_filename("CONSOLE").starts_with("CONSOLE-"));
        assert!(sanitize_for_filename("COM10").starts_with("COM10-"));

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

        // Without branch deletion, without force
        let cmd = build_remove_command(&path, None, false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("/tmp/test-worktree"));
        assert!(!cmd.contains("branch -D"));
        assert!(!cmd.contains("--force"));

        // With branch deletion, without force
        let cmd = build_remove_command(&path, Some("feature-branch"), false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("git branch -D"));
        assert!(cmd.contains("feature-branch"));
        assert!(!cmd.contains("--force"));

        // With force flag
        let cmd = build_remove_command(&path, None, true);
        assert!(cmd.contains("git worktree remove --force"));

        // With branch deletion and force
        let cmd = build_remove_command(&path, Some("feature-branch"), true);
        assert!(cmd.contains("git worktree remove --force"));
        assert!(cmd.contains("git branch -D"));

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        let cmd = build_remove_command(&special_path, Some("feature/branch"), false);
        assert!(cmd.contains("worktree remove"));
    }

    #[test]
    fn test_hook_log_hook_suffix() {
        use worktrunk::git::HookType;

        // Suffix includes sanitized name with hash for collision avoidance
        let log = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        let suffix = log.suffix();
        assert!(
            suffix.starts_with("user-post-start-server-"),
            "Expected pattern: {suffix}"
        );

        let log = HookLog::hook(HookSource::Project, HookType::PostCreate, "build");
        let suffix = log.suffix();
        assert!(
            suffix.starts_with("project-post-create-build-"),
            "Expected pattern: {suffix}"
        );

        let log = HookLog::hook(HookSource::User, HookType::PreRemove, "cleanup");
        let suffix = log.suffix();
        assert!(
            suffix.starts_with("user-pre-remove-cleanup-"),
            "Expected pattern: {suffix}"
        );
    }

    #[test]
    fn test_hook_log_internal_suffix() {
        let log = HookLog::internal(InternalOp::Remove);
        assert_eq!(log.suffix(), "remove");
    }

    #[test]
    fn test_hook_log_filename() {
        use worktrunk::git::HookType;

        // Filenames now include hash suffixes for collision avoidance
        let log = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        let filename = log.filename("main");
        assert!(
            filename.starts_with("main-"),
            "Expected main- prefix: {filename}"
        );
        assert!(
            filename.contains("-user-post-start-"),
            "Expected -user-post-start-: {filename}"
        );
        assert!(
            filename.ends_with(".log"),
            "Expected .log suffix: {filename}"
        );

        let filename = log.filename("feature/auth");
        assert!(
            filename.starts_with("feature-"),
            "Expected feature- prefix (slash sanitized): {filename}"
        );
        assert!(
            filename.contains("-user-post-start-"),
            "Expected -user-post-start-: {filename}"
        );

        let log = HookLog::internal(InternalOp::Remove);
        let filename = log.filename("main");
        assert!(
            filename.starts_with("main-"),
            "Expected main- prefix: {filename}"
        );
        assert!(
            filename.ends_with("-remove.log"),
            "Expected -remove.log suffix: {filename}"
        );
    }

    #[test]
    fn test_hook_log_parse_hook() {
        let log = HookLog::parse("user:post-start:server").unwrap();
        let suffix = log.suffix();
        assert!(
            suffix.starts_with("user-post-start-server-"),
            "Expected pattern: {suffix}"
        );

        let log = HookLog::parse("project:post-create:build").unwrap();
        let suffix = log.suffix();
        assert!(
            suffix.starts_with("project-post-create-build-"),
            "Expected pattern: {suffix}"
        );
    }

    #[test]
    fn test_hook_log_parse_internal() {
        let log = HookLog::parse("internal:remove").unwrap();
        assert_eq!(log, HookLog::Internal(InternalOp::Remove));
        assert_eq!(log.suffix(), "remove");
    }

    #[test]
    fn test_hook_log_parse_invalid_source() {
        let result = HookLog::parse("invalid:post-start:server");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown source"));
    }

    #[test]
    fn test_hook_log_parse_invalid_hook_type() {
        let result = HookLog::parse("user:invalid-hook:server");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown hook type"));
    }

    #[test]
    fn test_hook_log_parse_invalid_internal_op() {
        let result = HookLog::parse("internal:unknown");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown internal operation"));
    }

    #[test]
    fn test_hook_log_parse_invalid_format() {
        // Too many colons
        let result = HookLog::parse("user:post-start:server:extra");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid log spec"));

        // Single word (not valid anymore)
        let result = HookLog::parse("remove");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid log spec"));

        // Two colons but not internal:op
        let result = HookLog::parse("foo:bar");
        assert!(result.is_err());
    }

    #[test]
    fn test_hook_log_roundtrip() {
        // What gets created should match what gets looked up
        use worktrunk::git::HookType;

        // Hook: create the same way hooks.rs does, parse the same way state.rs does
        let created = HookLog::hook(HookSource::User, HookType::PostStart, "server");
        let parsed = HookLog::parse("user:post-start:server").unwrap();
        assert_eq!(created.filename("main"), parsed.filename("main"));

        // Internal: create the same way handlers.rs does, parse from CLI
        let created = HookLog::internal(InternalOp::Remove);
        let parsed = HookLog::parse("internal:remove").unwrap();
        assert_eq!(created.filename("main"), parsed.filename("main"));
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
    }
}
