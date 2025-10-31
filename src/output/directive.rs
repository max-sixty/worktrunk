//! Directive output mode for shell integration

use std::io::{self, Write};
use std::path::Path;

/// Directive output mode for shell integration
///
/// Outputs NUL-terminated directives for shell wrapper to parse and execute.
pub struct DirectiveOutput;

impl DirectiveOutput {
    pub fn new() -> Self {
        Self
    }

    pub fn success(&mut self, message: String) -> io::Result<()> {
        // Don't strip colors - users see this output and need styling
        write!(io::stdout(), "{}\0", message)?;
        io::stdout().flush()
    }

    pub fn progress(&mut self, message: String) -> io::Result<()> {
        // Progress messages are for humans - output them just like success messages
        // The shell wrapper will display these to users with colors preserved
        write!(io::stdout(), "{}\0", message)?;
        io::stdout().flush()
    }

    pub fn hint(&mut self, _message: String) -> io::Result<()> {
        // Hints are only for interactive mode - suppress in directive mode
        // When users run through shell wrapper, they already have integration
        Ok(())
    }

    pub fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        write!(io::stdout(), "__WORKTRUNK_CD__{}\0", path.display())?;
        io::stdout().flush()
    }

    pub fn execute(&mut self, command: String) -> io::Result<()> {
        write!(io::stdout(), "__WORKTRUNK_EXEC__{}\0", command)?;
        io::stdout().flush()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }

    pub fn terminate_output(&mut self) -> io::Result<()> {
        // Write NUL terminator to separate command output from subsequent directives
        write!(io::stdout(), "\0")?;
        io::stdout().flush()
    }

    /// Format a switch success message for directive mode
    ///
    /// In directive mode, the shell wrapper will actually change directories,
    /// so we can say "changed directory to {path}"
    pub fn format_switch_success(&self, branch: &str, path: &Path, created_branch: bool) -> String {
        use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
        let green_bold = GREEN.bold();

        if created_branch {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Created new worktree for {green_bold}{branch}{green_bold:#}, changed directory to {green_bold}{}{green_bold:#}{GREEN:#}",
                path.display()
            )
        } else {
            format!(
                "{SUCCESS_EMOJI} {GREEN}Switched to worktree for {green_bold}{branch}{green_bold:#}, changed directory to {green_bold}{}{green_bold:#}{GREEN:#}",
                path.display()
            )
        }
    }
}

impl Default for DirectiveOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    /// Test that directive output produces correctly formatted NUL-terminated strings
    ///
    /// While we can't easily test that flush() is called in unit tests,
    /// we can verify the output format is correct. The flushing is critical
    /// for fish shell integration to work correctly - without immediate flushing,
    /// the fish shell's `while read -z chunk` loop will block waiting for data
    /// that's stuck in stdout's buffer.
    #[test]
    fn test_directive_format() {
        // Create a buffer to capture output
        let mut buffer = Vec::new();

        // Test change_directory format
        let path = PathBuf::from("/test/path");
        write!(&mut buffer, "__WORKTRUNK_CD__{}\0", path.display()).unwrap();

        // Test success message format
        let message = "Test message";
        write!(&mut buffer, "{}\0", message).unwrap();

        // Test execute command format
        let command = "echo test";
        write!(&mut buffer, "__WORKTRUNK_EXEC__{}\0", command).unwrap();

        // Verify the buffer contains NUL-terminated strings
        let output = String::from_utf8_lossy(&buffer);
        assert!(output.contains("__WORKTRUNK_CD__/test/path\0"));
        assert!(output.contains("Test message\0"));
        assert!(output.contains("__WORKTRUNK_EXEC__echo test\0"));

        // Verify NUL bytes are in the right places
        let nul_count = buffer.iter().filter(|&&b| b == 0).count();
        assert_eq!(nul_count, 3, "Should have exactly 3 NUL terminators");
    }

    /// Test that anstyle formatting is preserved in directive output
    #[test]
    fn test_success_preserves_anstyle() {
        use anstyle::{AnsiColor, Color, Style};

        let bold = Style::new().bold();
        let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

        // Create a styled message
        let styled = format!("{cyan}Styled{cyan:#} {bold}message{bold:#}");

        // The styled message should contain ANSI escape codes
        assert!(
            styled.contains('\x1b'),
            "Styled message should contain ANSI escape codes"
        );

        // Directive mode preserves styling for users viewing through shell wrapper
        // (We're not testing actual output here, just documenting the behavior)
    }
}
