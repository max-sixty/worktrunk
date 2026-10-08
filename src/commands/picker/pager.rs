//! Pager detection and execution.
//!
//! Handles detection and use of diff pagers (delta, bat, etc.) for preview windows.
//! Input is prepared in an anonymous file before spawn. On Unix, one deadline
//! bounds both output consumption and the direct child wait,
//! including descendants retaining the pipes. Timeout closes our pipe endpoints
//! and kills/reaps only the owned pager; preview falls back to the original text.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use shared_child::SharedChild;

use worktrunk::git::Repository;
use worktrunk::shell::extract_filename_from_path;

use crate::pager::git_pager;

/// Cached pager command, ready to use. None means no pager.
static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

/// Maximum time to wait for pager to complete.
///
/// Pager blocking can freeze skim's event loop, making the UI unresponsive.
/// If the pager takes longer than this, kill it and fall back to raw diff.
pub(super) const PAGER_TIMEOUT: Duration = Duration::from_millis(2000);

/// Check if a pager spawns its own internal pager (e.g., less).
/// Delta and bat spawn `less` by default, which hangs in non-TTY contexts.
fn needs_paging_disabled(pager_cmd: &str) -> bool {
    pager_cmd
        .split_whitespace()
        .next()
        .and_then(extract_filename_from_path)
        .is_some_and(|basename| {
            basename.eq_ignore_ascii_case("delta")
                || basename.eq_ignore_ascii_case("bat")
                || basename.eq_ignore_ascii_case("batcat")
        })
}

/// Get the cached pager command, ready to use.
///
/// Returns the pager command with any necessary flags (like `--paging=never`)
/// already appended. Precedence:
/// 1. `[switch.picker] pager` in user config, with any `[projects."<id>"]`
///    override for `repo` applied (used as-is). A deprecated `[select] pager`
///    is migrated into `[switch.picker]` before the config parses.
/// 2. `GIT_PAGER` environment variable (auto-detection applied)
/// 3. `core.pager` git config (auto-detection applied)
///
/// The cache is process-wide: the picker runs against one repository.
pub(super) fn diff_pager(repo: &Repository) -> Option<&'static String> {
    CACHED_PAGER
        .get_or_init(|| {
            // Configured pager first - use exactly as specified (no auto-detection)
            if let Some(pager) = repo.config().switch_picker.pager()
                && !pager.trim().is_empty()
            {
                return Some(pager.to_string());
            }

            // GIT_PAGER or core.pager - apply auto-detection for delta/bat
            let pager = git_pager(false);

            pager.map(|p| {
                if needs_paging_disabled(&p) {
                    format!("{} --paging=never", p)
                } else {
                    p
                }
            })
        })
        .as_ref()
}

/// Pipe text through the configured pager for display.
///
/// Returns the paged output, or the original text if the pager fails or times out.
/// Sets `COLUMNS` environment variable for pagers like delta with side-by-side mode.
pub(super) fn pipe_through_pager(text: &str, pager_cmd: &str, width: usize) -> String {
    tracing::debug!(pager_cmd = %pager_cmd, "Piping through pager: {}", pager_cmd);

    let mut trace = worktrunk::trace::CommandTrace::new(None, pager_cmd).reads_stdin(true);
    let input = match worktrunk::shell_exec::buffered_stdin(text.as_bytes()) {
        Ok(input) => input,
        Err(error) => {
            trace.fail(&error);
            return text.to_string();
        }
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(pager_cmd)
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("COLUMNS", width.to_string());
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    let child = match worktrunk::shell_exec::spawn_shared_child(&mut cmd) {
        Ok(child) => child,
        Err(e) => {
            trace.fail(&e);
            tracing::debug!(error = %e, "Failed to spawn pager: {}", e);
            return text.to_string();
        }
    };

    let deadline = Instant::now() + PAGER_TIMEOUT;
    match pager_output(&child, deadline) {
        Ok(output) => {
            trace.complete(true);
            if let Ok(output) = String::from_utf8(output) {
                return output;
            }
        }
        Err(error) => {
            trace.fail(&error);
            tracing::debug!(%error, "Pager failed; using unpaged text");
            // SharedChild retains ownership even after reaping. Never signal a
            // process group: descendants are not owned by this preview.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    text.to_string()
}

/// Consume output and wait for the direct pager within the same deadline.
#[cfg(unix)]
fn pager_output(child: &SharedChild, deadline: Instant) -> std::io::Result<Vec<u8>> {
    let stdout = child
        .take_stdout()
        .ok_or_else(|| std::io::Error::other("Pager stdout was not captured"))?;
    let mut reader = worktrunk::shell_exec::pipe::PipeReader::new(stdout, Some(deadline), None)?;
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    let status = worktrunk::shell_exec::wait_shared_child(child, Some(deadline))?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "Pager exited with status: {status}"
        )));
    }
    Ok(output)
}

#[cfg(not(unix))]
fn pager_output(child: &SharedChild, deadline: Instant) -> std::io::Result<Vec<u8>> {
    // Drain output while waiting so a full stdout pipe cannot block the pager.
    let stdout = child.take_stdout();
    let reader_thread = std::thread::spawn(move || {
        stdout.map(|mut stdout| {
            let mut output = Vec::new();
            let _ = stdout.read_to_end(&mut output);
            output
        })
    });

    // Wait for pager with timeout
    match worktrunk::shell_exec::wait_shared_child(child, Some(deadline)) {
        Ok(status) => {
            // Pager exited within timeout
            if let Ok(Some(output)) = reader_thread.join()
                && status.success()
            {
                return Ok(output);
            }
            tracing::debug!(status = %status, "Pager exited with status: {}", status);
        }
        // Timed out, or the wait failed outright. Either way the pager owes us
        // nothing more: kill it, reap it, and fall back to the raw text below.
        outcome => {
            tracing::debug!(?outcome, timeout = ?PAGER_TIMEOUT, "Pager did not exit within {:?}", PAGER_TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader_thread.join();
        }
    }

    Err(std::io::Error::other("Pager failed or timed out"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_paging_disabled() {
        // delta - plain command name
        assert!(needs_paging_disabled("delta"));
        // delta - with arguments
        assert!(needs_paging_disabled("delta --side-by-side"));
        assert!(needs_paging_disabled("delta --paging=always"));
        // delta - full path
        assert!(needs_paging_disabled("/usr/bin/delta"));
        assert!(needs_paging_disabled(
            "/opt/homebrew/bin/delta --line-numbers"
        ));
        // bat - also spawns less by default
        assert!(needs_paging_disabled("bat"));
        assert!(needs_paging_disabled("/usr/bin/bat"));
        assert!(needs_paging_disabled("bat --style=plain"));
        // Pagers that don't spawn sub-pagers
        assert!(!needs_paging_disabled("less"));
        assert!(!needs_paging_disabled("diff-so-fancy"));
        assert!(!needs_paging_disabled("colordiff"));
        // Edge cases - similar names but not delta/bat
        assert!(!needs_paging_disabled("delta-preview"));
        assert!(!needs_paging_disabled("/path/to/delta-preview"));
        assert!(needs_paging_disabled("batcat")); // Debian's bat package name

        // Case-insensitive matching (Windows command names)
        assert!(needs_paging_disabled("Delta"));
        assert!(needs_paging_disabled("DELTA"));
        assert!(needs_paging_disabled("BAT"));
        assert!(needs_paging_disabled("Bat"));
        assert!(needs_paging_disabled("BatCat"));
        assert!(needs_paging_disabled("delta.exe"));
        assert!(needs_paging_disabled("Delta.EXE"));
    }

    #[test]
    fn test_pipe_through_pager_passthrough() {
        // Use cat as a simple pager that passes through input unchanged
        // A large complete input reaches the pager without pipe-capacity limits.
        let input = "line 1\nline 2\nline 3\n".repeat(25_000);
        let result = pipe_through_pager(&input, "cat", 80);
        assert_eq!(result, input);
    }

    #[test]
    fn test_pipe_through_pager_with_transform() {
        // Use tr to transform input (proves pager is actually being invoked)
        let input = "hello world";
        let result = pipe_through_pager(input, "tr 'a-z' 'A-Z'", 80);
        assert_eq!(result, "HELLO WORLD");
    }

    /// A pager that never exits must not freeze skim's event loop: it is killed at
    /// `PAGER_TIMEOUT` and the preview falls back to the unpaged text.
    #[test]
    #[cfg(unix)]
    fn test_pipe_through_pager_times_out() {
        let input = "line 1\nline 2";
        let start = std::time::Instant::now();
        let result = pipe_through_pager(input, "exec sleep 30", 80);
        let elapsed = start.elapsed();
        assert_eq!(result, input);
        assert!(
            elapsed < PAGER_TIMEOUT * 4,
            "the timeout did not bound the wait: {elapsed:?}"
        );
    }

    /// Descendants retaining output must not extend the preview deadline;
    /// retaining only buffered stdin must not delay completion at all.
    /// The release watchdog prevents a broken implementation from hanging the
    /// test. Correctness is the fallback output, not a wall-clock threshold.
    #[test]
    #[cfg(unix)]
    fn test_pipe_through_pager_descendant_pipes_time_out() {
        for redirect in ["", ">/dev/null"] {
            let fixture = tempfile::tempdir().unwrap();
            let release = fixture.path().join("release");
            let done = fixture.path().join("done");
            let quote =
                |path: &std::path::Path| shell_escape::escape(path.to_string_lossy()).into_owned();
            let command = format!(
                "(while [ ! -f {} ]; do sleep 0.02; done; : > {}) <&0 {} & printf transformed; exit 0",
                quote(&release),
                quote(&done),
                redirect,
            );
            let (tx, rx) = std::sync::mpsc::channel();
            let watchdog = std::thread::spawn(move || {
                let _ = rx.recv_timeout(PAGER_TIMEOUT * 3);
                std::fs::write(release, "").unwrap();
            });
            // Closing descendant stdout leaves only its inherited input file.
            // It cannot prolong the completed direct pager's lifetime.
            let input = "input\n".repeat(if redirect.is_empty() { 1 } else { 100_000 });
            let output = pipe_through_pager(&input, &command, 80);
            let _ = tx.send(());
            watchdog.join().unwrap();
            let cleanup_deadline = Instant::now() + PAGER_TIMEOUT * 3;
            while !done.exists() {
                assert!(Instant::now() < cleanup_deadline, "descendant did not exit");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                output,
                if redirect.is_empty() {
                    input.as_str()
                } else {
                    "transformed"
                },
                "descendant redirect: {redirect}"
            );
        }
    }

    #[test]
    fn test_pipe_through_pager_invalid_command() {
        // Invalid pager command should return original text
        let input = "original text";
        let result = pipe_through_pager(input, "nonexistent-command-xyz", 80);
        assert_eq!(result, input);
    }

    #[test]
    fn test_pipe_through_pager_failing_command() {
        // Pager that exits with error should return original text
        let input = "original text";
        let result = pipe_through_pager(input, "false", 80);
        assert_eq!(result, input);
    }

    #[test]
    fn test_pipe_through_pager_invalid_utf8_returns_original() {
        let input = "original text";
        assert_eq!(pipe_through_pager(input, "printf '\\377'", 80), input);
    }
}
