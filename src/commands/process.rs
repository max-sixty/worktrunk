use anyhow::Context;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use worktrunk::git::{HookType, Repository};
use worktrunk::path::{format_path_for_display, sanitize_for_filename};

use worktrunk::config::HookSource;

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
    /// Background cleanup of stale entries in `.git/wt/trash/`
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
    let relative_path = hook_log.path(Path::new(""), branch);
    let log_path = log_dir.join(&relative_path);
    let log_file =
        worktrunk::utils::open_log_file(&log_dir, &relative_path, false).with_context(|| {
            format!(
                "Failed to create log file {}",
                format_path_for_display(&log_path)
            )
        })?;

    Ok((log_path, log_file))
}

/// Sidecar written next to a per-branch hook log: `{path}.head`.
///
/// The log walker only collects `.log` files, so this name stays out of the
/// listing. It holds the branch OID at the moment the log was created.
pub(crate) fn log_head_sidecar(log_path: &Path) -> PathBuf {
    sidecar_path(log_path, ".head")
}

/// Sidecar written next to a per-branch hook log: `{path}.incarnation`.
///
/// Holds the worktree id from [`worktree_incarnation`] at the moment the log
/// was created. A later worktree of the same branch name does not rewrite it.
pub(crate) fn log_incarnation_sidecar(log_path: &Path) -> PathBuf {
    sidecar_path(log_path, ".incarnation")
}

fn sidecar_path(log_path: &Path, suffix: &str) -> PathBuf {
    let mut name = log_path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn record_sidecar(path: &Path, value: Option<&str>) -> std::io::Result<()> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => std::fs::write(path, format!("{value}\n")),
        None => match std::fs::remove_file(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
}

fn record_sidecar_best_effort(path: &Path, value: Option<&str>, what: &str) {
    if let Err(err) = record_sidecar(path, value) {
        tracing::debug!("failed to record {what} {}: {err}", path.display());
    }
}

/// Record `head` beside `log_path`, or remove a sidecar left by an earlier run.
///
/// A missing OID must not leave the previous run's file in place: the log is
/// truncated on each run, and a stale sidecar would attribute the new output
/// to the old commit.
pub(crate) fn record_log_head(log_path: &Path, head: Option<&str>) -> std::io::Result<()> {
    record_sidecar(&log_head_sidecar(log_path), head)
}

/// Record the OID, ignoring a failure. A missing sidecar reads back as
/// `head: null`, and failing the spawn or the step would skip the hook itself.
pub(crate) fn record_log_head_best_effort(log_path: &Path, head: Option<&str>) {
    if let Err(err) = record_log_head(log_path, head) {
        tracing::debug!(
            "failed to record hook log head {}: {err}",
            log_head_sidecar(log_path).display()
        );
    }
}

pub(crate) fn record_log_incarnation_best_effort(log_path: &Path, id: Option<&str>) {
    record_sidecar_best_effort(
        &log_incarnation_sidecar(log_path),
        id,
        "hook log incarnation",
    );
}

/// Branch-tip OID, only when `refs/heads/<branch>` still exists.
///
/// No fallback to the worktree HEAD: `post-remove` runs in the destination
/// after the branch is gone, and that HEAD is a different commit.
fn branch_ref_oid(repo: &Repository, branch: &str) -> Option<String> {
    if branch.is_empty() || branch == "HEAD" || branch == "detached" {
        return None;
    }
    let spec = format!("refs/heads/{branch}");
    repo.run_command(&["rev-parse", "--verify", "--end-of-options", &spec])
        .ok()
        .map(|oid| oid.trim().to_string())
        .filter(|oid| !oid.is_empty())
}

fn read_sidecar(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub(crate) fn read_log_head(log_path: &Path) -> Option<String> {
    read_sidecar(&log_head_sidecar(log_path))
}

pub(crate) fn read_log_incarnation(log_path: &Path) -> Option<String> {
    read_sidecar(&log_incarnation_sidecar(log_path))
}

/// Id of this worktree's life, stored as `wt-incarnation` in its git directory.
///
/// The same path returns the same id. Removing the worktree deletes the file
/// with the git directory, and the next checkout of that branch name mints
/// another one, including when both checkouts point at the same commit.
///
/// Minting happens when a hook log is written. Planning a removal only reads
/// the file, so a dry run does not create one.
pub(crate) fn worktree_incarnation(repo: &Repository, worktree_path: &Path) -> Option<String> {
    let dir = repo.worktree_at(worktree_path).git_dir().ok()?;
    read_or_mint_incarnation(&dir.join("wt-incarnation"))
}

/// `wt-incarnation` if a hook already wrote one. Does not create the file.
pub(crate) fn read_worktree_incarnation(repo: &Repository, worktree_path: &Path) -> Option<String> {
    let dir = repo.worktree_at(worktree_path).git_dir().ok()?;
    read_sidecar(&dir.join("wt-incarnation"))
}

/// Id to store beside a hook log.
///
/// A removal pipeline passes the id it read while the worktree still existed,
/// including `None` when that checkout never minted one. Any other run,
/// including a manual `wt hook post-remove` in a live worktree, reads or mints
/// the id of `worktree_path`.
pub(crate) fn hook_log_incarnation(
    repo: &Repository,
    worktree_path: &Path,
    from_removal: bool,
    removed_incarnation: Option<&str>,
) -> Option<String> {
    if from_removal {
        removed_incarnation.map(str::to_owned)
    } else {
        worktree_incarnation(repo, worktree_path)
    }
}

fn read_or_mint_incarnation(path: &Path) -> Option<String> {
    if let Some(id) = read_sidecar(path) {
        return Some(id);
    }
    let id = format!("{:032x}", fastrand::u128(..));
    // A lost race or an unwritable git dir both re-read. The winner's id is
    // kept; a failed write with nothing on disk leaves the log `null`.
    let _ = worktrunk::utils::write_new_atomically(path, &format!("{id}\n"));
    read_sidecar(path)
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
    args: &[OsString],
    branch: &str,
    hook_log: &HookLog,
) -> anyhow::Result<PathBuf> {
    let shell = worktrunk::shell_exec::ShellConfig::get()?;
    let args: Vec<_> = shell
        .args
        .iter()
        .map(OsString::from)
        .chain(args.iter().cloned())
        .collect();
    let (head, incarnation) = if matches!(hook_log, HookLog::Shared(_)) {
        (None, None)
    } else {
        (
            branch_ref_oid(repo, branch),
            // Internal removal logs run in the destination. The removed
            // checkout's id belongs on the post-remove hook pipeline, which
            // receives it from the plan. Minting one here would write into
            // the destination's git directory.
            None::<String>,
        )
    };
    let log_path = spawn_detached_exec(
        repo,
        worktree_path,
        &shell.executable,
        &args,
        branch,
        hook_log,
        &[],
    )?;
    if !matches!(hook_log, HookLog::Shared(_)) {
        record_log_head_best_effort(&log_path, head.as_deref());
        record_log_incarnation_best_effort(&log_path, incarnation.as_deref());
    }
    Ok(log_path)
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
    args: &[impl AsRef<OsStr>],
    branch: &str,
    hook_log: &HookLog,
    stdin_bytes: &[u8],
) -> anyhow::Result<std::path::PathBuf> {
    let (log_path, log_file) = create_detach_log(repo, branch, hook_log)?;

    tracing::debug!(
        program = %program.display(),
        "$ {} {} (detached, logging to {})",
        program.display(),
        args.iter().map(|arg| arg.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" "),
        log_path.file_name().unwrap_or_default().to_string_lossy()
    );

    // See [`worktrunk::priority`] for the priority-lowering rationale.
    let low_priority = matches!(hook_log, HookLog::Internal(_) | HookLog::Shared(_));
    // Detached children run in a `wt`-chosen directory and outlive this
    // process, so they never keep an inherited `GIT_DIR`.
    let mut cmd = worktrunk::priority::command(program, low_priority)
        .args(args)
        .current_dir(worktree_path)
        .scrub_git_discovery_env();
    if matches!(hook_log, HookLog::Hook { .. }) {
        cmd = cmd.env(
            worktrunk::priority::FOREGROUND_ENV_VAR,
            worktrunk::priority::BACKGROUND_HOOK_VALUE,
        );
    }
    let mut cmd = cmd.into_command()?;
    cmd.stdin(
        worktrunk::shell_exec::buffered_stdin(stdin_bytes)
            .context("Failed to prepare detached input")?,
    )
    .stdout(Stdio::from(
        log_file
            .try_clone()
            .context("Failed to clone log file handle")?,
    ))
    .stderr(Stdio::from(log_file));
    detach(&mut cmd);
    worktrunk::shell_exec::spawn(&mut cmd).context("Failed to spawn detached process")?;

    Ok(log_path)
}

/// A new process group, so the child survives the terminal closing.
#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

/// A new process group with no console window, detached from this one.
#[cfg(windows)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
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
/// 1. [`sweep_trash`] — restart deletion of `.git/wt/trash/` entries left
///    by an interrupted background removal.
/// 2. [`worktrunk::git::fsmonitor::reap_orphan_fsmonitor_daemons`] — terminate
///    `git fsmonitor--daemon` processes whose worktree no longer exists.
///    Defense-in-depth for daemons orphaned by paths that bypass `wt remove`
///    (plain `git worktree remove`, manual `rm -rf`, a crashed `wt`); the
///    `wt remove` source itself stops the daemon synchronously.
pub fn run_internal_sweep(repo: &Repository) {
    let _span = worktrunk::trace::Span::new("internal-sweep");
    sweep_trash(repo);
    worktrunk::git::fsmonitor::reap_orphan_fsmonitor_daemons(repo);
}

/// Fire-and-forget restart of trash deletion.
///
/// Worktree removal renames the worktree into
/// `.git/wt/trash/<name>-<timestamp>/` and starts a detached
/// [`worktrunk::git::drain_trash`] to delete it. If that process is
/// interrupted (SIGKILL, reboot, disk full), its entries stay behind. `wt
/// remove` calls this function after its primary user-visible output — so the
/// sweep never delays the progress or success message — and starts another
/// drain when the trash holds anything and no drain is running.
///
/// Best effort: spawn failures are logged at debug level and otherwise
/// ignored. The sweep is purely additive — the primary `wt remove` operation
/// proceeds regardless of outcome.
pub fn sweep_trash(repo: &Repository) {
    let trash_dir = repo.wt_trash_dir();
    if worktrunk::git::staged_trash_entries(&trash_dir).is_empty()
        || worktrunk::git::trash_drain_running(&trash_dir)
    {
        return;
    }

    // The sweep is repo-wide (not branch-scoped), so it logs to a top-level
    // shared file alongside `commands.jsonl` and `trace.log`. The branch
    // argument is ignored for the `Shared` variant.
    let spawned = std::env::current_exe()
        .context("Failed to locate Worktrunk executable")
        .and_then(|executable| {
            spawn_detached(
                repo,
                &repo.wt_dir(),
                &build_drain_trash_command(executable, repo),
                "",
                &HookLog::Shared(InternalOp::TrashSweep),
            )
        });
    if let Err(e) = spawned {
        tracing::debug!(error = %format!("{e:#}"), "Failed to spawn stale trash sweep");
    }
}

/// Shell argv that runs `wt -C <git-common-dir> _drain-trash`: a fixed script,
/// its `$0`, then native arguments. Paths are never interpolated into shell
/// source. The drain derives the trash directory from the repository it
/// discovers, so it can't be pointed at a directory outside a trash. `-C`
/// names that repository because the spawn's cwd may be the placeholder of a
/// worktree the removal just deleted.
pub fn build_drain_trash_command(executable: PathBuf, repo: &Repository) -> Vec<OsString> {
    vec![
        OsString::from(r#"exec "$@""#),
        OsString::from("wt"),
        executable.into_os_string(),
        OsString::from("-C"),
        repo.git_common_dir().as_os_str().to_os_string(),
        OsString::from("_drain-trash"),
    ]
}

/// Remove the empty shell-PWD placeholder after the wrapper consumes its cd directive.
/// `rmdir` preserves any files written into the original path.
pub fn build_remove_placeholder_command(path: &Path) -> Vec<OsString> {
    vec![
        r#"sleep 1 && rmdir -- "$1" 2>/dev/null"#.into(),
        "wt".into(),
        path.as_os_str().into(),
    ]
}

/// Fallback when staging fails. Only flags and sequencing enter the script;
/// the checkout remains a separate process argument.
pub fn build_remove_command(
    path: &Path,
    force_worktree: bool,
    changed_directory: bool,
    tail: Option<&[OsString]>,
) -> Vec<OsString> {
    let delay = if changed_directory { "sleep 1 && " } else { "" };
    let force = if force_worktree { " --force" } else { "" };
    let mut script = format!(r#"{delay}git worktree remove{force} -- "$1""#);
    if tail.is_some() {
        script.push_str(r#" && shift && "$@""#);
    }
    let mut args = vec![script.into(), "wt".into(), path.as_os_str().into()];
    if let Some(tail) = tail {
        args.extend_from_slice(tail);
    }
    args
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
    fn drain_command_passes_paths_as_their_own_arguments() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let executable = PathBuf::from("/opt/with space-'$(echo PWNED)\n/wt");
        let args = build_drain_trash_command(executable.clone(), &repo);
        assert_eq!(args[0], r#"exec "$@""#);
        assert_eq!(args[2], executable.as_os_str());
        assert_eq!(args[3], "-C");
        assert_eq!(args[4], repo.git_common_dir().as_os_str());
        assert_eq!(args[5], "_drain-trash");
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
    fn record_log_head_writes_and_clears() {
        let dir = worktrunk::testing::test_tempdir();
        let log = dir.path().join("hook.log");
        std::fs::write(&log, "output").unwrap();

        record_log_head(&log, Some("abc\n")).unwrap();
        assert_eq!(read_log_head(&log).as_deref(), Some("abc"));

        record_log_head(&log, None).unwrap();
        assert_eq!(read_log_head(&log), None);
        record_log_head(&log, None).unwrap();
    }

    #[test]
    fn branch_ref_oid_is_that_branch_tip() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let main = test.head_sha();
        test.run_git(&["checkout", "-b", "feature"]);
        test.commit("feature");
        let feature = test.head_sha();
        test.run_git(&["checkout", "main"]);

        let repo = worktrunk::git::Repository::at(test.root_path()).unwrap();
        assert_eq!(
            branch_ref_oid(&repo, "feature").as_deref(),
            Some(feature.as_str())
        );
        assert_ne!(
            branch_ref_oid(&repo, "feature").as_deref(),
            Some(main.as_str())
        );
        assert_eq!(branch_ref_oid(&repo, "missing"), None);
        assert_eq!(branch_ref_oid(&repo, "HEAD"), None);
        assert_eq!(branch_ref_oid(&repo, "detached"), None);
    }

    /// A sidecar that exists but holds no OID reads as `null`, the same as a
    /// missing one. The JSON entry must not surface a stale or empty `head`.
    #[test]
    fn read_log_head_is_none_for_an_empty_sidecar() {
        let dir = worktrunk::testing::test_tempdir();
        let log = dir.path().join("hook.log");
        std::fs::write(&log, "output").unwrap();
        std::fs::write(log_head_sidecar(&log), "").unwrap();

        assert_eq!(read_log_head(&log), None);

        std::fs::write(log_head_sidecar(&log), "  \n").unwrap();
        assert_eq!(read_log_head(&log), None, "whitespace-only is empty too");
    }

    /// A sidecar write that fails is logged, not returned: the caller is about
    /// to run a hook, and skipping it over bookkeeping would be the wrong
    /// trade. A directory where the sidecar should be makes the write fail.
    #[test]
    fn record_log_head_best_effort_swallows_a_failed_write() {
        let dir = worktrunk::testing::test_tempdir();
        let log = dir.path().join("hook.log");
        std::fs::write(&log, "output").unwrap();
        std::fs::create_dir(log_head_sidecar(&log)).unwrap();

        record_log_head_best_effort(&log, Some("abc"));

        // The hook log itself is untouched, and `head` reads as null.
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "output");
        assert_eq!(read_log_head(&log), None);
        assert!(log_head_sidecar(&log).is_dir());
    }

    /// The same worktree keeps one id. A second checkout of the name, even at
    /// the same commit, does not.
    #[test]
    fn worktree_incarnation_changes_when_the_worktree_is_replaced() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.root_path()).unwrap();
        let first = worktree_incarnation(&repo, test.root_path()).unwrap();
        assert_eq!(
            worktree_incarnation(&repo, test.root_path()).as_deref(),
            Some(first.as_str())
        );

        let path = test.root_path().parent().unwrap().join("repo.feature");
        test.run_git(&["worktree", "add", "-b", "feature", path.to_str().unwrap()]);
        let feature = worktree_incarnation(&repo, &path).unwrap();
        assert_ne!(feature, first);

        test.run_git(&["worktree", "remove", "--force", path.to_str().unwrap()]);
        test.run_git(&["branch", "-D", "feature"]);
        test.run_git(&["worktree", "add", "-b", "feature", path.to_str().unwrap()]);
        let again = worktree_incarnation(&repo, &path).unwrap();
        assert_ne!(again, feature);
        assert_eq!(
            test.git_output(&["rev-parse", "feature"]),
            test.git_output(&["rev-parse", "main"])
        );
    }

    /// Planning reads an id a hook already wrote and does not create one.
    /// A removal log uses that captured value instead of the destination.
    #[test]
    fn removal_uses_the_id_it_read_and_does_not_mint() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = worktrunk::git::Repository::at(test.root_path()).unwrap();
        let path = test.root_path().parent().unwrap().join("repo.feature");
        test.run_git(&["worktree", "add", "-b", "feature", path.to_str().unwrap()]);

        assert_eq!(read_worktree_incarnation(&repo, &path), None);
        let git_dir = repo.worktree_at(&path).git_dir().unwrap();
        assert!(!git_dir.join("wt-incarnation").exists());

        let id = worktree_incarnation(&repo, &path).unwrap();
        assert_eq!(
            read_worktree_incarnation(&repo, &path).as_deref(),
            Some(id.as_str())
        );

        assert_eq!(
            hook_log_incarnation(&repo, test.root_path(), true, Some(&id)).as_deref(),
            Some(id.as_str())
        );
        assert_eq!(
            hook_log_incarnation(&repo, test.root_path(), true, None),
            None
        );
        assert!(!repo.wt_dir().join("incarnations").exists());
    }
}
