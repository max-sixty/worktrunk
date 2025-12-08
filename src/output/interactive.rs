//! Interactive output mode for human users

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{eprintln, stderr};

#[cfg(not(unix))]
use super::handlers::execute_streaming;
use super::traits::OutputHandler;

/// Interactive output mode for human users
///
/// Formats messages with colors, emojis, and formatting.
/// Executes commands directly instead of emitting directives.
pub struct InteractiveOutput {
    /// Target directory for command execution (set by change_directory)
    target_dir: Option<std::path::PathBuf>,
}

impl InteractiveOutput {
    pub fn new() -> Self {
        Self { target_dir: None }
    }
}

impl OutputHandler for InteractiveOutput {
    fn write_message_line(&mut self, line: &str) -> io::Result<()> {
        // All messages go to stderr (stdout reserved for data like JSON)
        eprintln!("{line}");
        stderr().flush()
    }

    fn gutter(&mut self, content: String) -> io::Result<()> {
        // Gutter uses write! (no newline) rather than writeln!
        write!(stderr(), "{content}")?;
        stderr().flush()
    }

    fn data(&mut self, content: String) -> io::Result<()> {
        // Structured data (JSON, etc.) goes to stdout for piping
        use worktrunk::styling::println;
        println!("{content}");
        io::stdout().flush()
    }

    fn shell_integration_hint(&mut self, message: String) -> io::Result<()> {
        // Shell integration hints work the same as regular hints in interactive mode
        use worktrunk::styling::hint_message;
        self.write_message_line(&hint_message(&message))
    }

    fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // In interactive mode, we can't actually change directory
        // Just store the target for execute commands
        self.target_dir = Some(path.to_path_buf());
        Ok(())
    }

    #[cfg(unix)]
    fn execute(&mut self, command: String) -> anyhow::Result<()> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let exec_dir = self.target_dir.as_deref().unwrap_or_else(|| Path::new("."));

        // Use exec() to replace wt process with the command.
        // This gives the command full TTY access (stdin, stdout, stderr all inherited),
        // enabling interactive programs like `claude` to work properly.
        let err = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(exec_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .exec();

        // exec() only returns on error
        Err(anyhow::anyhow!("Failed to exec '{}': {}", command, err))
    }

    #[cfg(not(unix))]
    fn execute(&mut self, command: String) -> anyhow::Result<()> {
        // On non-Unix platforms, fall back to spawn-and-wait
        let exec_dir = self.target_dir.as_deref().unwrap_or_else(|| Path::new("."));
        execute_streaming(&command, exec_dir, false, None)?;
        Ok(())
    }

    fn flush_for_stderr_prompt(&mut self) -> io::Result<()> {
        // All messages go to stderr, so just flush stderr
        stderr().flush()
    }

    fn terminate_output(&mut self) -> io::Result<()> {
        // No-op in interactive mode - shell script only emitted in directive mode
        Ok(())
    }
}

impl Default for InteractiveOutput {
    fn default() -> Self {
        Self::new()
    }
}
