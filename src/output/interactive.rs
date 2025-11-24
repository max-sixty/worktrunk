//! Interactive output mode for human users

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{println, stderr, stdout};

use super::handlers::execute_streaming;
use super::traits::OutputHandler;

/// Interactive output mode for human users
///
/// Formats messages with colors, emojis, and formatting.
/// Executes commands directly instead of emitting directives.
pub struct InteractiveOutput {
    /// Target directory for command execution (set by change_directory)
    target_dir: Option<std::path::PathBuf>,
    /// Cached stdout handle
    stdout: io::Stdout,
}

impl InteractiveOutput {
    pub fn new() -> Self {
        Self {
            target_dir: None,
            stdout: io::stdout(),
        }
    }
}

impl OutputHandler for InteractiveOutput {
    fn write_message_line(&mut self, line: &str) -> io::Result<()> {
        // Use styled println for proper color detection
        println!("{line}");
        self.stdout.flush()
    }

    fn gutter(&mut self, content: String) -> io::Result<()> {
        // Gutter content is pre-formatted with its own newlines
        write!(self.stdout, "{content}")?;
        self.stdout.flush()
    }

    fn shell_integration_hint(&mut self, message: String) -> io::Result<()> {
        // Shell integration hints work the same as regular hints in interactive mode
        self.hint(message)
    }

    #[cfg(unix)]
    fn blank_line(&mut self) -> io::Result<()> {
        // Ensure subsequent output starts on a fresh line after interactive UIs like skim
        println!();
        stdout().flush()
    }

    fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // In interactive mode, we can't actually change directory
        // Just store the target for execute commands
        self.target_dir = Some(path.to_path_buf());
        Ok(())
    }

    fn execute(&mut self, command: String) -> anyhow::Result<()> {
        // Execute command in the target directory with streaming output
        let exec_dir = self.target_dir.as_deref().unwrap_or_else(|| Path::new("."));

        // TODO: Consider using exec() to replace wt process with the command
        // Currently: wt spawns command as child, waits, then exits
        // Alternative: use std::os::unix::process::CommandExt::exec() to replace wt entirely
        // Trade-offs:
        // - Pro: Removes wt from process tree (cleaner `ps` output)
        // - Pro: Command becomes direct child of shell (more natural process hierarchy)
        // - Con: Can't show wt's success messages before exec (they'd be lost)
        // - Con: Unix-only (no Windows equivalent)
        // - Con: More complex error handling (exec only returns on error)

        // Use shared streaming execution (no stdout->stderr redirect for --execute)
        execute_streaming(&command, exec_dir, false)?;

        Ok(())
    }

    fn flush_for_stderr_prompt(&mut self) -> io::Result<()> {
        // In interactive mode, flush both streams before stderr prompt
        stdout().flush()?;
        stderr().flush()
    }

    fn terminate_output(&mut self) -> io::Result<()> {
        // No-op in interactive mode - no NUL terminators needed
        Ok(())
    }
}

impl Default for InteractiveOutput {
    fn default() -> Self {
        Self::new()
    }
}
