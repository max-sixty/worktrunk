//! Concurrent execution of shell commands with prefixed-line output.
//!
//! Foreground concurrent groups (from `HookStep::Concurrent`) spawn every
//! command at once and combine their output into a single terminal stream,
//! each line prefixed with its command's colored label. Prefixed lines keep
//! full scrollback intact for debugging failures and work identically under
//! a TTY or pipe (CI logs).
//!
//! ## Execution model
//!
//! Children share the caller's foreground process group with closed stdin and
//! piped stdout/stderr. A single Unix poll loop owns all output pipes and renders
//! complete labeled lines. Direct child waits run independently of output EOF.
//!
//! Commands retain EOF semantics, including output from surviving descendants,
//! regardless of ordinary exit status. Cancellation drains queued bytes and
//! releases pipes even when an unowned descendant retains a write end.
//! Once direct waits finish, the signal listener releases its command lease;
//! a newly delivered interrupt cancels any remaining output wait.
//! Ctrl-C reaches the native job through the kernel; a caught key preserves
//! each child's normal status. PID-targeted SIGTERM reaches owned direct children.
//!
//! All direct children complete before the caller receives results in input order.

use shared_child::SharedChild;
#[cfg(not(unix))]
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::mpsc;
#[cfg(not(unix))]
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Instant;

use anyhow::Context;

use worktrunk::command_log::log_command;
#[cfg(unix)]
use worktrunk::git::ErrorExt;
use worktrunk::git::WorktrunkError;
use worktrunk::shell_exec::{
    ShellConfig, apply_cd_directive_env, scrub_directive_env_vars, scrub_git_discovery_env_vars,
    wait_foreground_child,
};
#[cfg(unix)]
use worktrunk::signal_forwarder::ForegroundSignals;
use worktrunk::styling::stderr;
use worktrunk::trace::CommandTrace;

use super::handlers::DirectivePassthrough;

/// One command in a concurrent group.
pub struct ConcurrentCommand<'a> {
    /// Short label used as the line prefix (e.g., the command name).
    pub label: &'a str,
    /// Fully-expanded shell command string.
    pub expanded: &'a str,
    /// Child's working directory.
    pub working_dir: &'a Path,
    /// Optional label for `commands.jsonl` tracing.
    pub log_label: Option<&'a str>,
    /// Directive file env vars to pass through to the child. See
    /// `DirectivePassthrough`: the CD directive file is the only one passed
    /// through — every child scrubs the rest.
    pub directives: &'a DirectivePassthrough,
    /// Scrub inherited git-discovery vars (`GIT_DIR`/`GIT_WORK_TREE`/…) from the
    /// child. `true` for hooks (they operate on the worktree wt targets), `false`
    /// for aliases (they keep wt's inherited context). See issue #3373.
    pub scrub_git_discovery: bool,
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
    if cmds.is_empty() {
        return Ok(Vec::new());
    }
    let prefix_width = cmds.iter().map(|c| c.label.len()).max().unwrap_or(0);
    let shell = ShellConfig::get()?;

    if std::env::var_os("WORKTRUNK_TEST_SERIAL_CONCURRENT").is_some() {
        return run_serial_with_prefix(shell, cmds, prefix_width);
    }

    // Install before spawn so startup signals remain queued.
    #[cfg(unix)]
    let signals = ForegroundSignals::install()?;

    // Spawn each child and record its start time for commands.jsonl. If any
    // spawn fails partway, kill and reap every child we already spawned —
    // otherwise they'd outlive wt as unreaped orphans with nobody draining
    // their pipes (and `Child::drop` does not kill the process on Unix).
    let mut children: Vec<SpawnedChild> = Vec::with_capacity(cmds.len());
    for (i, cmd) in cmds.iter().enumerate() {
        match spawn_child(
            shell,
            i,
            cmd,
            #[cfg(unix)]
            &signals,
        ) {
            Ok(spawned) => children.push(spawned),
            Err(e) => {
                #[cfg(unix)]
                if e.interrupt_signal().is_some() {
                    // Keep admitted children's cleanup and the original
                    // startup cancellation through the normal drain path.
                    return drain_children(children, cmds, 0, prefix_width, signals);
                }
                abort_spawned_children(children);
                return Err(e);
            }
        }
    }

    drain_children(
        children,
        cmds,
        0,
        prefix_width,
        #[cfg(unix)]
        signals,
    )
}

/// Reap owned children when the group cannot finish spawning. Resolve their
/// traces even though their normal outcome collectors will never run.
fn abort_spawned_children(children: Vec<SpawnedChild>) {
    for mut spawned in children {
        let _ = spawned.child.kill();
        let _ = spawned.child.wait();
        spawned.trace.complete(false);
    }
}

/// Wait independently of output EOF. Cancellation releases remaining pipes
/// after all direct waits; ordinary exits preserve descendant output to EOF.
#[cfg(unix)]
fn drain_children(
    mut children: Vec<SpawnedChild>,
    cmds: &[ConcurrentCommand<'_>],
    offset: usize,
    prefix_width: usize,
    signals: ForegroundSignals,
) -> anyhow::Result<Vec<anyhow::Result<()>>> {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::io::ErrorKind;
    use std::os::fd::AsFd;

    // One wake pair serves the whole group, regardless of its number of pipes.
    // Allocate it and prepare every pipe before starting any waiter.
    let setup = (|| {
        let (control, wake) = worktrunk::shell_exec::socket_pair()?;
        control.set_nonblocking(true)?;
        wake.set_nonblocking(true)?;
        let mut pipes = Vec::with_capacity(children.len() * 2);
        for (index, child) in children.iter().enumerate() {
            let prefix = render_prefix(index + offset, cmds[index].label, prefix_width);
            if let Some(stdout) = child.child.take_stdout() {
                pipes.push(OutputPipe::new(prefix.clone(), stdout)?);
            }
            if let Some(stderr) = child.child.take_stderr() {
                pipes.push(OutputPipe::new(prefix, stderr)?);
            }
        }
        Ok::<_, std::io::Error>((control, wake, pipes))
    })();
    let (control, wake, mut pipes) = match setup {
        Ok(prepared) => prepared,
        Err(error) => {
            for child in &mut children {
                let _ = child.child.kill();
                let _ = child.child.wait();
                child.trace.fail(&error);
            }
            return Err(error).context("Failed to prepare concurrent command output");
        }
    };
    let processes: Vec<_> = children.iter().map(|child| child.child.clone()).collect();
    let (tx, rx) = mpsc::channel();
    let notify = WakeSender {
        tx,
        wake: Arc::new(wake),
    };
    let signal_notify = notify.clone();
    let mut signal_thread = signals.forward_to_children(processes.clone(), move || {
        signal_notify.wake();
    });
    let child_count = children.len();
    let mut outcomes = Vec::with_capacity(child_count);
    let mut output_error = None;
    thread::scope(|scope| {
        // Observe every exit independently: a later child's interruption must
        // cancel the operation while an earlier child is still cleaning up.
        for (index, (child, cmd)) in children.into_iter().zip(cmds).enumerate() {
            let notify = notify.clone();
            scope.spawn(move || notify.send(index, collect_outcome(child, cmd)));
        }
        loop {
            let result = (|| {
                // Consume the wake before the queue: a message racing the queue
                // read must leave a byte to wake the next poll.
                let mut wake_bytes = [0; 256];
                match nix::unistd::read(&control, &mut wake_bytes).map_err(std::io::Error::from) {
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::WouldBlock | ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error),
                }
                while let Ok(outcome) = rx.try_recv() {
                    outcomes.push(outcome);
                }
                if outcomes.len() == child_count {
                    // The execution lease ends here; the provider observes only
                    // newly delivered INT while descendants retain output pipes.
                    signal_thread.finish_children()?;
                }
                if outcomes.len() == child_count && signal_thread.interrupt_signal().is_some() {
                    for pipe in &mut pipes {
                        pipe.finish_available()?;
                    }
                    pipes.clear();
                }
                if outcomes.len() == child_count && pipes.is_empty() {
                    return Ok(true);
                }
                let ready = {
                    let mut fds = pipes
                        .iter()
                        .map(|pipe| PollFd::new(pipe.stream.as_fd(), PollFlags::POLLIN))
                        .collect::<Vec<_>>();
                    fds.push(PollFd::new(control.as_fd(), PollFlags::POLLIN));
                    match poll(&mut fds, PollTimeout::NONE) {
                        Ok(_) => {}
                        Err(nix::errno::Errno::EINTR) => return Ok(false),
                        Err(error) => return Err(error.into()),
                    }
                    fds.into_iter()
                        .take(pipes.len())
                        .map(|fd| fd.revents().unwrap_or(PollFlags::empty()))
                        .collect::<Vec<_>>()
                };
                // Read one chunk per ready pipe so a continuous writer cannot
                // starve completion messages or another child's output.
                for (pipe, ready) in pipes.iter_mut().zip(ready) {
                    if !ready.is_empty() && !pipe.read_ready()? {
                        pipe.closed = true;
                    }
                }
                pipes.retain(|pipe| !pipe.closed);
                Ok::<_, std::io::Error>(false)
            })();
            match result {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    // Closing pipes releases writers; waiters still reap every
                    // owned direct child before we return the I/O failure.
                    pipes.clear();
                    for child in &processes {
                        let _ = child.kill();
                    }
                    output_error = Some(error);
                    break;
                }
            }
        }
    });
    let cancelled = signal_thread
        .stop()
        .context("Failed to stop concurrent signal listener")?;
    if let Some(error) = output_error {
        return Err(error).context("Failed to read concurrent command output");
    }
    if let Some(signal) = cancelled {
        return Err(WorktrunkError::Interrupted { signal, hint: None }.into());
    }
    outcomes.sort_unstable_by_key(|(index, _)| *index);
    Ok(outcomes.into_iter().map(|(_, outcome)| outcome).collect())
}

#[cfg(unix)]
#[derive(Clone)]
struct WakeSender {
    tx: mpsc::Sender<(usize, anyhow::Result<()>)>,
    wake: Arc<std::os::unix::net::UnixStream>,
}

#[cfg(unix)]
impl WakeSender {
    fn send(&self, index: usize, outcome: anyhow::Result<()>) {
        if self.tx.send((index, outcome)).is_ok() {
            self.wake();
        }
    }

    fn wake(&self) {
        worktrunk::shell_exec::pipe::wake(&*self.wake);
    }
}

#[cfg(unix)]
struct OutputPipe {
    prefix: String,
    stream: worktrunk::shell_exec::pipe::PipeReader,
    pending: Vec<u8>,
    closed: bool,
}

#[cfg(unix)]
impl OutputPipe {
    fn new(prefix: String, stream: impl Into<std::os::fd::OwnedFd>) -> std::io::Result<Self> {
        let stream = worktrunk::shell_exec::pipe::PipeReader::new(stream, None, None)?;
        Ok(Self {
            prefix,
            stream,
            pending: Vec::new(),
            closed: false,
        })
    }

    fn output(&mut self, bytes: &[u8]) {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.pending.extend_from_slice(part);
            if part.ends_with(b"\n") {
                self.flush_line();
            }
        }
    }

    fn flush_line(&mut self) {
        let line = worktrunk::shell_exec::output_line(&self.pending);
        writeln!(stderr().lock(), "{}{}", self.prefix, line).ok();
        self.pending.clear();
    }

    fn read_ready(&mut self) -> std::io::Result<bool> {
        use std::io::ErrorKind;
        let mut bytes = [0; 8192];
        match self.stream.read_ready(&mut bytes) {
            Ok(0) => {
                if !self.pending.is_empty() {
                    self.flush_line();
                }
                Ok(false)
            }
            Ok(count) => {
                self.output(&bytes[..count]);
                Ok(true)
            }
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    /// Snapshot queued bytes once: draining until WouldBlock can run forever
    /// when an unowned descendant continuously writes without newlines.
    fn finish_available(&mut self) -> std::io::Result<()> {
        use std::io::ErrorKind;
        self.stream.cancel()?;
        let mut bytes = [0; 8192];
        loop {
            match self.stream.read_ready(&mut bytes) {
                Ok(0) => break,
                Ok(count) => self.output(&bytes[..count]),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if !self.pending.is_empty() {
            self.flush_line();
        }
        Ok(())
    }
}

/// Platforms without Unix poll retain blocking readers and direct-child waits.
#[cfg(not(unix))]
fn drain_children(
    children: Vec<SpawnedChild>,
    cmds: &[ConcurrentCommand<'_>],
    offset: usize,
    prefix_width: usize,
) -> anyhow::Result<Vec<anyhow::Result<()>>> {
    let (tx, rx) = mpsc::channel::<Event>();
    let mut readers = Vec::new();
    for (index, child) in children.iter().enumerate() {
        let label = cmds[index].label.to_string();
        if let Some(stdout) = child.child.take_stdout() {
            readers.push(spawn_reader(
                index + offset,
                label.clone(),
                stdout,
                tx.clone(),
            ));
        }
        if let Some(stderr) = child.child.take_stderr() {
            readers.push(spawn_reader(index + offset, label, stderr, tx.clone()));
        }
    }
    drop(tx);
    let mut output_error = None;
    let outcomes = thread::scope(|scope| {
        let waiter = scope.spawn(move || {
            children
                .into_iter()
                .zip(cmds)
                .map(|(child, cmd)| collect_outcome(child, cmd))
                .collect()
        });
        for event in rx {
            match event {
                Event::Line(labeled) => {
                    let prefix = render_prefix(labeled.index, &labeled.label, prefix_width);
                    writeln!(stderr().lock(), "{}{}", prefix, labeled.line).ok();
                }
                Event::ReaderClosed(Err(error)) => {
                    output_error.get_or_insert(error);
                }
                Event::ReaderClosed(Ok(())) => {}
            }
        }
        waiter
            .join()
            .map_err(|_| anyhow::anyhow!("concurrent child waiter panicked"))
    })?;
    for reader in readers {
        reader
            .join()
            .map_err(|_| anyhow::anyhow!("concurrent output reader panicked"))?;
    }
    if let Some(error) = output_error {
        return Err(error).context("Failed to read concurrent command output");
    }
    Ok(outcomes)
}

fn run_serial_with_prefix(
    shell: &ShellConfig,
    cmds: &[ConcurrentCommand<'_>],
    prefix_width: usize,
) -> anyhow::Result<Vec<anyhow::Result<()>>> {
    let mut outcomes = Vec::with_capacity(cmds.len());
    for (index, cmd) in cmds.iter().enumerate() {
        #[cfg(unix)]
        let signals = ForegroundSignals::install()?;
        let spawned = spawn_child(
            shell,
            index,
            cmd,
            #[cfg(unix)]
            &signals,
        )?;
        outcomes.extend(drain_children(
            vec![spawned],
            std::slice::from_ref(cmd),
            index,
            prefix_width,
            #[cfg(unix)]
            signals,
        )?);
    }
    Ok(outcomes)
}

struct SpawnedChild {
    child: Arc<SharedChild>,
    cmd_str: String,
    log_label: Option<String>,
    started_at: Instant,
    /// `[wt-trace]` record for this child, captured at spawn time and resolved
    /// in `collect_outcome`. Held across the output-draining window so the
    /// recorded duration is the full spawn → wait span, not just the wait.
    trace: CommandTrace,
}

fn spawn_child(
    shell: &ShellConfig,
    index: usize,
    cmd: &ConcurrentCommand<'_>,
    #[cfg(unix)] signals: &ForegroundSignals,
) -> anyhow::Result<SpawnedChild> {
    // Siblings cannot share interactive stdin, but each retains native access
    // to /dev/tty through the shell-established foreground group.
    let mut command = shell.command(cmd.expanded);
    command
        .current_dir(cmd.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // User hooks discover their repo from the cwd wt sets, not an inherited
    // GIT_DIR/GIT_WORK_TREE (issue #3373). Aliases keep the inherited context.
    if cmd.scrub_git_discovery {
        scrub_git_discovery_env_vars(&mut command);
    }

    // Scrub all directive env vars, then re-add the CD passthrough.
    scrub_directive_env_vars(&mut command);
    if let Some(path) = &cmd.directives.cd_file {
        apply_cd_directive_env(&mut command, path);
    }

    tracing::debug!(
        command = %cmd.expanded,
        shell = %shell.name,
        "$ {} (concurrent #{index}, shell: {})",
        cmd.expanded,
        shell.name
    );

    // Start the trace just before spawning so its duration brackets the real
    // spawn → wait span (the child keeps running while we drain its output).
    let mut trace = CommandTrace::new(None, cmd.expanded);
    #[cfg(unix)]
    let spawned = signals.spawn(&mut command);
    #[cfg(not(unix))]
    let spawned =
        worktrunk::shell_exec::spawn_shared_child(&mut command).map_err(anyhow::Error::from);
    let child = match spawned {
        Ok(child) => Arc::new(child),
        Err(e) => {
            trace.fail(&e);
            return Err(e)
                .with_context(|| format!("failed to spawn concurrent command '{}'", cmd.label));
        }
    };

    Ok(SpawnedChild {
        child,
        cmd_str: cmd.expanded.to_string(),
        log_label: cmd.log_label.map(str::to_string),
        started_at: Instant::now(),
        trace,
    })
}

#[cfg(not(unix))]
fn send_line(index: usize, label: &str, bytes: &[u8], tx: &Sender<Event>) {
    let _ = tx.send(Event::Line(LabeledLine {
        index,
        label: label.to_owned(),
        line: worktrunk::shell_exec::output_line(bytes).into_owned(),
    }));
}

#[cfg(not(unix))]
fn spawn_reader<R: Read + Send + 'static>(
    index: usize,
    label: String,
    stream: R,
    tx: Sender<Event>,
) -> thread::JoinHandle<()> {
    use std::io::{BufRead, BufReader};
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut pending = Vec::new();
        let result = (|| {
            while reader.read_until(b'\n', &mut pending)? != 0 {
                send_line(index, &label, &pending, &tx);
                pending.clear();
            }
            Ok(())
        })();
        let _ = tx.send(Event::ReaderClosed(result));
    })
}

fn collect_outcome(spawned: SpawnedChild, cmd: &ConcurrentCommand<'_>) -> anyhow::Result<()> {
    let SpawnedChild {
        child,
        cmd_str,
        log_label,
        started_at,
        mut trace,
    } = spawned;

    let status = match wait_foreground_child(&child) {
        Ok(status) => status,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            trace.fail(&e);
            return Err(e)
                .with_context(|| format!("failed to wait for concurrent command '{}'", cmd.label));
        }
    };
    trace.complete(status.success());

    let duration = started_at.elapsed();
    let outcome = WorktrunkError::from_child_status(&status, None);

    if let Some(label) = log_label {
        log_command(&label, &cmd_str, outcome.exit_code(), Some(duration));
    }

    if status.success() {
        Ok(())
    } else {
        Err(outcome.into())
    }
}

#[cfg(not(unix))]
enum Event {
    Line(LabeledLine),
    ReaderClosed(std::io::Result<()>),
}

#[cfg(not(unix))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_cmds_returns_empty() {
        let outcomes = run_concurrent_commands(&[]).expect("no spawn should happen");
        assert!(outcomes.is_empty());
    }
}
