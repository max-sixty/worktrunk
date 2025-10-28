//! Interactive output mode for human users

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{GREEN, println};

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

    pub fn success(&mut self, message: String) -> io::Result<()> {
        println!("✅ {GREEN}{message}{GREEN:#}");
        Ok(())
    }

    pub fn progress(&mut self, message: String) -> io::Result<()> {
        println!("{message}");
        Ok(())
    }

    pub fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // In interactive mode, we can't actually change directory
        // Just store the target for execute commands
        self.target_dir = Some(path.to_path_buf());
        Ok(())
    }

    pub fn execute(&mut self, command: String) -> io::Result<()> {
        // Execute command in the target directory
        let exec_dir = self
            .target_dir
            .as_ref()
            .map(|p| p.as_path())
            .unwrap_or_else(|| Path::new("."));

        crate::output::execute_command_in_worktree(exec_dir, &command)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
    }

    pub fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()?;
        io::stderr().flush()?;
        Ok(())
    }

    pub fn is_interactive(&self) -> bool {
        true
    }
}

impl Default for InteractiveOutput {
    fn default() -> Self {
        Self::new()
    }
}
