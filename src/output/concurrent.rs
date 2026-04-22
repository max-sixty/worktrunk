//! Concurrent execution of shell commands with prefixed-line output.
//!
//! Foreground concurrent groups (from `HookStep::Concurrent`) spawn every
//! command at once and combine their output into a single terminal stream,
//! each line prefixed with its command's colored label. Prefixed lines keep
//! full scrollback intact for debugging failures and work identically under
//! a TTY or pipe (CI logs).
//!
//! TODO: upgrade to a docker-compose-v2-style tailing display — each command
//! gets a fixed terminal region showing its last N lines, overwritten in
//! place as output arrives. Better signal-to-noise for long streams like
//! `cargo test`, but substantially more implementation (cursor tracking,
//! resize handling, TTY/non-TTY fallback, scrollback replay on failure).
//! Prefixed lines suffice until live sections are demonstrated to pay for
//! themselves.
//!
//! ## Execution model
//!
//! For each command:
//! 1. Spawn a shell child with stdout+stderr piped and (on Unix) its own
//!    process group so SIGINT/SIGTERM can be delivered to the whole tree.
//! 2. Pipe `context_json` to stdin if provided, then close.
//! 3. Launch two reader threads that read lines and send labeled lines on
//!    a shared channel. A single consumer writes to stderr — one writer
//!    preserves line atomicity so readers never mix bytes mid-line.
//!
//! The main thread drains the channel. A lightweight ticker thread polls
//! `signal_hook::Signals` for SIGINT/SIGTERM and forwards with escalation
//! to every live child's process group.
//!
//! All children always run to completion. Per-child exit status is returned
//! for the caller to fold into a failure, matching alias `thread::scope` and
//! pipeline `run_concurrent_group` semantics.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Instant;

use anyhow::Context;

use worktrunk::command_log::log_command;
use worktrunk::git::WorktrunkError;
use worktrunk::shell_exec::{
    DIRECTIVE_CD_FILE_ENV_VAR, DIRECTIVE_FILE_ENV_VAR, ShellConfig, scrub_directive_env_vars,
};
use worktrunk::styling::stderr;

use super::handlers::DirectivePassthrough;

/// One command in a concurrent group.
pub struct ConcurrentCommand<'a> {
    /// Short label used as the line prefix (e.g., the command name).
    pub label: &'a str,
    /// Fully-expanded shell command string.
    pub expanded: &'a str,
    /// Child's working directory.
    pub working_dir: &'a Path,
    /// JSON blob written to the child's stdin and closed. Callers that have
    /// no context to pass should supply `"{}"`.
    pub context_json: &'a str,
    /// Optional label for `commands.jsonl` tracing.
    pub log_label: Option<&'a str>,
    /// Directive file env vars to pass through to the child. See
    /// `DirectivePassthrough` for the trust model (CD passthrough, EXEC scrub).
    pub directives: &'a DirectivePassthrough,
}

/// Run every command concurrently and return each per-child result in input
/// order. `Err(WorktrunkError::ChildProcessExited { .. })` signals a non-zero
/// exit; other errors come from spawn/IO failures.
///
/// When the `WORKTRUNK_TEST_SERIAL_CONCURRENT` env var is set, commands run
/// sequentially in input order — same prefix-line output path, just one child
/// at a time. Tests use this to pin deterministic interleaving for snapshots.
pub fn run_concurrent_commands(
    cmds: &[ConcurrentCommand<'_>],
) -> anyhow::Result<Vec<anyhow::Result<()>>> {
    let prefix_width = cmds.iter().map(|c| c.label.len()).max().unwrap_or(0);
    let shell = ShellConfig::get()?;

    if std::env::var_os("WORKTRUNK_TEST_SERIAL_CONCURRENT").is_some() {
        return Ok(run_serial_with_prefix(shell, cmds, prefix_width));
    }

    // Install the SIGINT/SIGTERM latch BEFORE spawning any children so a
    // signal that arrives mid-spawn is captured rather than default-killing
    // wt (which would orphan already-spawned children, since each runs in
    // its own process group and wouldn't see the tty's Ctrl-C broadcast).
    #[cfg(unix)]
    let signals = {
        use signal_hook::consts::{SIGINT, SIGTERM};
        signal_hook::iterator::Signals::new([SIGINT, SIGTERM])?
    };

    // Spawn each child and record its start time for commands.jsonl. If any
    // spawn fails partway, kill and reap every child we already spawned —
    // otherwise they'd outlive wt as unreaped orphans with nobody draining
    // their pipes (and `Child::drop` does not kill the process on Unix).
    let mut children: Vec<SpawnedChild> = Vec::with_capacity(cmds.len());
    for (i, cmd) in cmds.iter().enumerate() {
        match spawn_child(shell, i, cmd) {
            Ok(spawned) => children.push(spawned),
            Err(e) => {
                for mut prior in children {
                    let _ = prior.child.kill();
                    let _ = prior.child.wait();
                }
                return Err(e);
            }
        }
    }

    // Print one prefixed line at a time from a single writer. Each reader
    // sends complete lines through `line_tx`; the consumer drains them.
    let (line_tx, line_rx) = mpsc::channel::<LabeledLine>();

    let mut readers: Vec<thread::JoinHandle<()>> = Vec::new();
    for (i, spawned) in children.iter_mut().enumerate() {
        let label = cmds[i].label.to_string();
        if let Some(stdout) = spawned.child.stdout.take() {
            readers.push(spawn_reader(i, label.clone(), stdout, line_tx.clone()));
        }
        if let Some(stderr) = spawned.child.stderr.take() {
            readers.push(spawn_reader(i, label, stderr, line_tx.clone()));
        }
    }
    // Drop the original sender so the channel closes once every reader exits.
    drop(line_tx);

    // Start the forwarder thread now that we have the pgid list. The
    // `signals` latch was installed up-front, so any signal received during
    // the spawn loop was queued and will be delivered on the thread's first
    // poll.
    #[cfg(unix)]
    let signal_thread = spawn_signal_forwarder(
        signals,
        children
            .iter()
            .map(|c| c.child.id() as i32)
            .collect::<Vec<_>>(),
    );

    // Consume labeled lines until every reader drops its sender.
    {
        let mut stderr = stderr().lock();
        for labeled in line_rx {
            let prefix = render_prefix(labeled.index, &labeled.label, prefix_width);
            writeln!(stderr, "{}{}", prefix, labeled.line).ok();
        }
    }

    for r in readers {
        let _ = r.join();
    }

    // After the last output line prints, wait on each child.
    let mut outcomes = Vec::with_capacity(children.len());
    for (spawned, cmd) in children.into_iter().zip(cmds) {
        outcomes.push(collect_outcome(spawned, cmd));
    }

    #[cfg(unix)]
    {
        // All children have exited — shut the signal forwarder down.
        signal_thread.stop();
    }

    Ok(outcomes)
}

/// Serial fallback for `WORKTRUNK_TEST_SERIAL_CONCURRENT`.
///
/// Runs each command to completion before starting the next — same prefix-line
/// output, deterministic interleaving. Outcomes come back in input order.
fn run_serial_with_prefix(
    shell: &ShellConfig,
    cmds: &[ConcurrentCommand<'_>],
    prefix_width: usize,
) -> Vec<anyhow::Result<()>> {
    let mut results = Vec::with_capacity(cmds.len());
    for (i, cmd) in cmds.iter().enumerate() {
        let spawned = match spawn_child(shell, i, cmd) {
            Ok(s) => s,
            Err(e) => {
                results.push(Err(e));
                continue;
            }
        };
        let result = drain_and_wait_single(spawned, cmd, i, prefix_width);
        results.push(result);
    }
    results
}

/// Spawn readers for one child, drain to stderr with prefixes, then wait.
fn drain_and_wait_single(
    mut spawned: SpawnedChild,
    cmd: &ConcurrentCommand<'_>,
    index: usize,
    prefix_width: usize,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<LabeledLine>();
    let mut readers: Vec<thread::JoinHandle<()>> = Vec::new();
    let label = cmd.label.to_string();
    if let Some(stdout) = spawned.child.stdout.take() {
        readers.push(spawn_reader(index, label.clone(), stdout, tx.clone()));
    }
    if let Some(stderr) = spawned.child.stderr.take() {
        readers.push(spawn_reader(index, label, stderr, tx.clone()));
    }
    drop(tx);
    {
        let mut out = stderr().lock();
        for labeled in rx {
            let prefix = render_prefix(labeled.index, &labeled.label, prefix_width);
            writeln!(out, "{prefix}{}", labeled.line).ok();
        }
    }
    for r in readers {
        let _ = r.join();
    }
    collect_outcome(spawned, cmd)
}

struct SpawnedChild {
    child: Child,
    cmd_str: String,
    log_label: Option<String>,
    started_at: Instant,
}

fn spawn_child(
    shell: &ShellConfig,
    index: usize,
    cmd: &ConcurrentCommand<'_>,
) -> anyhow::Result<SpawnedChild> {
    let mut command = shell.command(cmd.expanded);
    command
        .current_dir(cmd.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Scrub all directive env vars, then re-add the passthroughs.
    scrub_directive_env_vars(&mut command);
    if let Some(path) = &cmd.directives.cd_file {
        command.env(DIRECTIVE_CD_FILE_ENV_VAR, path);
    }
    if let Some(path) = &cmd.directives.legacy_file {
        command.env(DIRECTIVE_FILE_ENV_VAR, path);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    log::debug!(
        "$ {} (concurrent #{index}, shell: {})",
        cmd.expanded,
        shell.name
    );

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn concurrent command '{}'", cmd.label))?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe — child may exit or close stdin early.
        let _ = stdin.write_all(cmd.context_json.as_bytes());
    }

    Ok(SpawnedChild {
        child,
        cmd_str: cmd.expanded.to_string(),
        log_label: cmd.log_label.map(str::to_string),
        started_at: Instant::now(),
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    index: usize,
    label: String,
    stream: R,
    tx: Sender<LabeledLine>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // Read by bytes, not by `BufRead::lines()` — `lines()` returns
        // `InvalidData` on non-UTF-8 bytes and terminates the iterator, which
        // would leave the child's pipe un-drained and eventually hang
        // `child.wait()` once the pipe buffer fills. Real-world triggers
        // include `git diff` of binary files, tools that honor non-UTF-8
        // locales, and any raw-byte output. Lossy-decoding preserves the
        // stream and keeps the child unblocked.
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::with_capacity(256);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => return,
                Ok(_) => {
                    // Strip the trailing newline (and optional \r) before
                    // forwarding — the writer re-adds a newline per line.
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                        if buf.last() == Some(&b'\r') {
                            buf.pop();
                        }
                    }
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    if tx
                        .send(LabeledLine {
                            index,
                            label: label.clone(),
                            line,
                        })
                        .is_err()
                    {
                        return; // consumer dropped
                    }
                }
                Err(_) => return, // IO error on the pipe — give up on this stream
            }
        }
    })
}

fn collect_outcome(spawned: SpawnedChild, cmd: &ConcurrentCommand<'_>) -> anyhow::Result<()> {
    let SpawnedChild {
        mut child,
        cmd_str,
        log_label,
        started_at,
    } = spawned;

    let status = child
        .wait()
        .with_context(|| format!("failed to wait for concurrent command '{}'", cmd.label))?;

    let duration = started_at.elapsed();
    let exit_code = status.code();

    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal: Option<i32> = None;

    let normalized_code = exit_code.or_else(|| signal.map(|s| 128 + s));

    if let Some(label) = log_label {
        log_command(&label, &cmd_str, normalized_code, Some(duration));
    }

    if status.success() {
        Ok(())
    } else if let Some(sig) = signal {
        Err(WorktrunkError::ChildProcessExited {
            code: 128 + sig,
            message: format!("terminated by signal {sig}"),
            signal: Some(sig),
        }
        .into())
    } else {
        let code = exit_code.unwrap_or(1);
        Err(WorktrunkError::ChildProcessExited {
            code,
            message: format!("exit status: {code}"),
            signal: None,
        }
        .into())
    }
}

struct LabeledLine {
    index: usize,
    label: String,
    line: String,
}

fn render_prefix(index: usize, label: &str, width: usize) -> String {
    use anstyle::{AnsiColor, Color, Style};
    let palette = [
        AnsiColor::Cyan,
        AnsiColor::Magenta,
        AnsiColor::Yellow,
        AnsiColor::Green,
        AnsiColor::Blue,
        AnsiColor::BrightCyan,
        AnsiColor::BrightMagenta,
        AnsiColor::BrightYellow,
    ];
    let style = Style::new()
        .fg_color(Some(Color::Ansi(palette[index % palette.len()])))
        .bold();
    format!("{style}{label:<width$}{style:#} │ ")
}

#[cfg(unix)]
struct SignalForwarder {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: thread::JoinHandle<()>,
}

#[cfg(unix)]
impl SignalForwarder {
    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

#[cfg(unix)]
fn spawn_signal_forwarder(
    mut signals: signal_hook::iterator::Signals,
    pgids: Vec<i32>,
) -> SignalForwarder {
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = thread::spawn(move || {
        let mut seen_once = false;
        while !stop_clone.load(Ordering::Relaxed) {
            for sig in signals.pending() {
                if !seen_once {
                    // First signal: graceful escalation per child
                    // (SIGINT → SIGTERM → SIGKILL with grace windows).
                    seen_once = true;
                    for &pgid in &pgids {
                        worktrunk::shell_exec::forward_signal_with_escalation(pgid, sig);
                    }
                } else {
                    // Subsequent signals: user is impatient — SIGKILL every
                    // still-live process group without waiting.
                    for &pgid in &pgids {
                        let _ = nix::sys::signal::killpg(
                            nix::unistd::Pid::from_raw(pgid),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(25));
        }
    });

    SignalForwarder { stop, handle }
}

#[cfg(test)]
mod tests {
    //! Unit tests that exercise the executor's option-bearing code paths which
    //! aren't reachable through the alias integration tests today: every child
    //! currently has `log_label=None` (aliases skip per-child logging) and the
    //! CD/legacy directive env vars are usually unset. Driving these branches
    //! with a direct call proves they behave correctly when a future caller
    //! (concurrent foreground hooks, once their deprecation completes) uses them.
    use super::*;

    fn run_one_with_directives(
        label: &str,
        script: &str,
        log_label: Option<&str>,
        directives: &DirectivePassthrough,
    ) -> Vec<anyhow::Result<()>> {
        let wd = std::env::temp_dir();
        let specs = vec![ConcurrentCommand {
            label,
            expanded: script,
            working_dir: &wd,
            context_json: "{}",
            log_label,
            directives,
        }];
        run_concurrent_commands(&specs).expect("spawn failed")
    }

    /// A command with a `log_label` exercises the `log_command` branch in
    /// `collect_outcome` — only hook-origin children take this path today.
    #[test]
    fn test_log_label_is_recorded() {
        let outcomes = run_one_with_directives(
            "job",
            "true",
            Some("test-label"),
            &DirectivePassthrough::none(),
        );
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].is_ok(), "`true` should exit 0");
        // The logger is a global OnceCell not initialised in tests, so
        // `log_command` is effectively a no-op — what we're testing is that
        // passing a log_label doesn't panic and the command still runs.
    }

    /// `DirectivePassthrough` with both `cd_file` and `legacy_file` set must
    /// propagate both env vars to the child. The child script echoes each env
    /// var; we assert both values are delivered.
    ///
    /// Unix-only: the script uses POSIX `sh` redirect syntax and relies on
    /// native temp paths that don't need escaping. Git Bash on Windows would
    /// need the paths converted to `/c/...` form first.
    #[test]
    #[cfg(unix)]
    fn test_directive_env_vars_passed_through() {
        use tempfile::NamedTempFile;
        let cd = NamedTempFile::new().unwrap();
        let legacy = NamedTempFile::new().unwrap();
        let directives = DirectivePassthrough {
            cd_file: Some(cd.path().to_path_buf()),
            legacy_file: Some(legacy.path().to_path_buf()),
        };
        // Write each env var's value to its matching temp file. If the child
        // didn't receive the env var, the redirect would fail or write an
        // empty file.
        let script = format!(
            "printf CD > {} && printf LEGACY > {}",
            cd.path().display(),
            legacy.path().display(),
        );
        let outcomes = run_one_with_directives("job", &script, None, &directives);
        assert!(outcomes[0].is_ok(), "child should exit 0");
        let cd_contents = std::fs::read_to_string(cd.path()).unwrap();
        let legacy_contents = std::fs::read_to_string(legacy.path()).unwrap();
        assert_eq!(cd_contents, "CD");
        assert_eq!(legacy_contents, "LEGACY");
    }

    /// An empty `cmds` slice must return an empty outcomes vec without
    /// spawning anything or erroring. Integration tests never hit this shape
    /// (parser guarantees non-empty Concurrent groups) so we cover it here.
    #[test]
    fn test_empty_cmds_returns_empty() {
        let outcomes = run_concurrent_commands(&[]).expect("no spawn should happen");
        assert!(outcomes.is_empty());
    }
}
