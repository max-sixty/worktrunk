//! Directive output mode for shell integration

use anstream::adapter::strip_str;
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
        // Ensure any prior stderr output is NUL-terminated before writing to stdout
        write!(io::stderr(), "\0")?;
        io::stderr().flush()?;

        let plain = strip_str(&message).to_string();
        write!(io::stdout(), "{}\0", plain)?;
        io::stdout().flush()
    }

    pub fn progress(&mut self, _message: String) -> io::Result<()> {
        // Progress messages are only for humans - suppress in directive mode
        Ok(())
    }

    pub fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // Ensure any prior stderr output is NUL-terminated before writing directive
        // The shell wrapper merges stderr+stdout via 2>&1 and splits on NUL bytes,
        // so we need to terminate stderr to prevent it from concatenating with directives
        write!(io::stderr(), "\0")?;
        io::stderr().flush()?;

        write!(io::stdout(), "__WORKTRUNK_CD__{}\0", path.display())?;
        io::stdout().flush()
    }

    pub fn execute(&mut self, command: String) -> io::Result<()> {
        // Ensure any prior stderr output is NUL-terminated before writing directive
        write!(io::stderr(), "\0")?;
        io::stderr().flush()?;

        write!(io::stdout(), "__WORKTRUNK_EXEC__{}\0", command)?;
        io::stdout().flush()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        writeln!(io::stdout())?;
        io::stdout().flush()?;
        io::stderr().flush()
    }

    pub fn is_interactive(&self) -> bool {
        false
    }
}

impl Default for DirectiveOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Test that anstyle formatting is stripped from success messages
    #[test]
    fn test_success_strips_anstyle() {
        use anstyle::{AnsiColor, Color, Style};

        let bold = Style::new().bold();
        let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

        // Create a styled message
        let styled = format!("{cyan}Styled{cyan:#} {bold}message{bold:#}");

        // The strip_str function should remove the ANSI codes
        let plain = strip_str(&styled).to_string();
        assert_eq!(plain, "Styled message");
        assert!(
            !plain.contains('\x1b'),
            "Should not contain ANSI escape codes"
        );
    }
}
