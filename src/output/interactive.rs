//! Interactive output mode for human users

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{println, stderr, stdout};

use super::handlers::execute_streaming;

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
        // Messages now include emoji and color directly for consistency across modes
        println!("{message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn progress(&mut self, message: String) -> io::Result<()> {
        println!("{message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn hint(&mut self, message: String) -> io::Result<()> {
        // Hints are suggestions for interactive users (like "run wt configure-shell")
        println!("{message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // In interactive mode, we can't actually change directory
        // Just store the target for execute commands
        self.target_dir = Some(path.to_path_buf());
        Ok(())
    }

    pub fn execute(&mut self, command: String) -> io::Result<()> {
        // Execute command in the target directory with streaming output
        let exec_dir = self.target_dir.as_deref().unwrap_or_else(|| Path::new("."));

        // Use shared streaming execution (no stdout->stderr redirect for --execute)
        execute_streaming(&command, exec_dir, false)?;

        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        stdout().flush()?;
        stderr().flush()?;
        Ok(())
    }

    pub fn terminate_output(&mut self) -> io::Result<()> {
        // No-op in interactive mode - no NUL terminators needed
        Ok(())
    }

    /// Format a switch success message for interactive mode
    ///
    /// In interactive mode, we can't actually change directories, so we say "at {path}"
    pub fn format_switch_success(&self, branch: &str, path: &Path, created_branch: bool) -> String {
        use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
        let green_bold = GREEN.bold();

        if created_branch {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Created new worktree for {green_bold}{branch}{green_bold:#} at {green_bold}{}{green_bold:#}{GREEN:#}",
                path.display()
            )
        } else {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Switched to worktree for {green_bold}{branch}{green_bold:#} at {green_bold}{}{green_bold:#}{GREEN:#}",
                path.display()
            )
        }
    }
}

impl Default for InteractiveOutput {
    fn default() -> Self {
        Self::new()
    }
}
