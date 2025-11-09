//! Interactive output mode for human users

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{
    HINT_EMOJI, INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, WARNING_EMOJI, println, stderr, stdout,
};

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
        // Success messages automatically include the ✅ emoji
        println!("{SUCCESS_EMOJI} {message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn progress(&mut self, message: String) -> io::Result<()> {
        // Progress messages automatically include the 🔄 emoji
        println!("{PROGRESS_EMOJI} {message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn hint(&mut self, message: String) -> io::Result<()> {
        // Hint messages automatically include the 💡 emoji
        println!("{HINT_EMOJI} {message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn info(&mut self, message: String) -> io::Result<()> {
        // Info messages automatically include the ⚪ emoji
        println!("{INFO_EMOJI} {message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn warning(&mut self, message: String) -> io::Result<()> {
        // Warning messages automatically include the 🟡 emoji
        println!("{WARNING_EMOJI} {message}");
        stdout().flush()?;
        Ok(())
    }

    pub fn blank_line(&mut self) -> io::Result<()> {
        // Ensure subsequent output starts on a fresh line after interactive UIs like skim
        println!();
        stdout().flush()
    }

    pub fn gutter(&mut self, content: String) -> io::Result<()> {
        // Gutter content has its own visual structure - just print it
        use worktrunk::styling::print;
        print!("{content}");
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
    pub fn format_switch_success(
        &self,
        branch: &str,
        path: &Path,
        created_branch: bool,
        base_branch: Option<&str>,
    ) -> String {
        super::format_switch_success_message(branch, path, created_branch, base_branch)
    }
}

impl Default for InteractiveOutput {
    fn default() -> Self {
        Self::new()
    }
}
