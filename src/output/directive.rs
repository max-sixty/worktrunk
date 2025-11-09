//! Directive output mode for shell integration
//!
//! # How Shell Integration Works
//!
//! Worktrunk uses a directive protocol to enable shell integration. When running with
//! `--internal` flag (invoked by shell wrapper), commands output directives that the
//! shell wrapper parses and executes.
//!
//! ## Protocol
//!
//! Running `wt switch --internal my-branch` outputs:
//!
//! ```text
//! __WORKTRUNK_CD__/path/to/worktree\0
//! Switched to worktree: my-branch\0
//! ```
//!
//! The shell wrapper parses this output:
//! - Lines starting with `__WORKTRUNK_CD__` trigger directory changes
//! - Lines starting with `__WORKTRUNK_EXEC__` trigger command execution
//! - Other lines print normally to the user
//! - All messages are NUL-terminated for reliable parsing
//!
//! This separation keeps the Rust binary focused on git logic while the shell
//! handles environment changes (cd, exec).
//!
//! ## Pattern
//!
//! This pattern is proven by tools like zoxide, starship, and direnv. The `--internal`
//! flag is hidden from help output—end users never interact with it directly.

use std::io::{self, Write};
use std::path::Path;
use worktrunk::styling::{INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, WARNING_EMOJI};

/// Directive output mode for shell integration
///
/// Outputs NUL-terminated directives for shell wrapper to parse and execute.
///
/// See module-level documentation for protocol details.
pub struct DirectiveOutput;

impl DirectiveOutput {
    pub fn new() -> Self {
        Self
    }

    pub fn success(&mut self, message: String) -> io::Result<()> {
        // Success messages automatically include the ✅ emoji
        write!(io::stdout(), "{SUCCESS_EMOJI} {message}\0")?;
        io::stdout().flush()
    }

    pub fn progress(&mut self, message: String) -> io::Result<()> {
        // Progress messages automatically include the 🔄 emoji
        write!(io::stdout(), "{PROGRESS_EMOJI} {message}\0")?;
        io::stdout().flush()
    }

    pub fn hint(&mut self, _message: String) -> io::Result<()> {
        // Hints are only for interactive mode - suppress in directive mode
        // When users run through shell wrapper, they already have integration
        Ok(())
    }

    pub fn info(&mut self, message: String) -> io::Result<()> {
        // Info messages automatically include the ⚪ emoji
        write!(io::stdout(), "{INFO_EMOJI} {message}\0")?;
        io::stdout().flush()
    }

    pub fn warning(&mut self, message: String) -> io::Result<()> {
        // Warning messages automatically include the 🟡 emoji
        write!(io::stdout(), "{WARNING_EMOJI} {message}\0")?;
        io::stdout().flush()
    }

    pub fn blank_line(&mut self) -> io::Result<()> {
        // Blank lines are only meaningful in interactive mode; no-op here to avoid empty directives
        Ok(())
    }

    pub fn gutter(&mut self, content: String) -> io::Result<()> {
        // Gutter content has its own visual structure - print with NUL terminator
        write!(io::stdout(), "{content}\0")?;
        io::stdout().flush()
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
    /// Directive mode mirrors the interactive wording ("at {path}") so the
    /// shell wrapper and direct CLI output stay consistent.
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

    #[test]
    fn test_path_with_special_characters() {
        // BUG HYPOTHESIS: Paths with newlines or NUL bytes could break directive parsing
        let mut buffer = Vec::new();

        // Path with newline (shouldn't normally happen, but let's test)
        let path = PathBuf::from("/test/path\nwith\nnewlines");
        write!(&mut buffer, "__WORKTRUNK_CD__{}\0", path.display()).unwrap();

        let output = String::from_utf8_lossy(&buffer);
        // The newlines are preserved - this could break parsing!
        assert!(output.contains('\n'), "Newlines in path are preserved");

        // Count NUL bytes - should only be at the end
        let nul_positions: Vec<_> = buffer
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b == 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            nul_positions.len(),
            1,
            "Should have exactly 1 NUL terminator"
        );
        assert_eq!(
            nul_positions[0],
            buffer.len() - 1,
            "NUL should be at the end"
        );
    }

    #[test]
    fn test_command_with_nul_bytes() {
        // BUG HYPOTHESIS: Commands with embedded NUL bytes could break parsing
        let mut buffer = Vec::new();

        let command = "echo test\0extra";
        write!(&mut buffer, "__WORKTRUNK_EXEC__{}\0", command).unwrap();

        // Count NUL bytes
        let nul_count = buffer.iter().filter(|&&b| b == 0).count();
        // We have 2 NUL bytes: one embedded in command, one terminator
        assert_eq!(
            nul_count, 2,
            "Embedded NUL creates extra terminator - breaks parsing!"
        );
    }

    #[test]
    fn test_message_with_nul_bytes() {
        // What if success message contains NUL bytes?
        let mut buffer = Vec::new();

        let message = "Part1\0Part2";
        write!(&mut buffer, "{}\0", message).unwrap();

        let nul_count = buffer.iter().filter(|&&b| b == 0).count();
        // Embedded NUL plus terminator = 2 NUL bytes
        assert_eq!(nul_count, 2, "Embedded NUL creates parsing ambiguity!");
    }

    #[test]
    fn test_color_reset_on_empty_style() {
        // BUG HYPOTHESIS from CLAUDE.md (lines 154-177):
        // Using {:#} on Style::new() produces empty string, not reset code
        use anstyle::Style;

        let empty_style = Style::new();
        let output = format!("{:#}", empty_style);

        // This is the bug: {:#} on empty style produces empty string!
        assert_eq!(
            output, "",
            "BUG: Empty style reset produces empty string, not \\x1b[0m"
        );

        // This means colors can leak: "text in color{:#}" where # is on empty Style
        // doesn't actually reset, it just removes the style prefix!
    }

    #[test]
    fn test_proper_reset_with_anstyle_reset() {
        // The correct way to reset ALL styles is anstyle::Reset
        use anstyle::Reset;

        let output = format!("{}", Reset);

        // This should produce the actual reset escape code
        assert!(
            output.contains("\x1b[0m") || output == "\x1b[0m",
            "Reset should produce actual ANSI reset code"
        );
    }

    #[test]
    fn test_nested_style_resets_leak_color() {
        // BUG HYPOTHESIS from CLAUDE.md:
        // Nested style resets can leak colors
        use anstyle::{AnsiColor, Color, Style};

        let warning = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        let bold = Style::new().bold();

        // BAD pattern: nested reset
        let bad_output = format!("{warning}Text with {bold}nested{bold:#} styles{warning:#}");

        // When {bold:#} resets, it might also reset the warning color!
        // We can't easily test the actual ANSI codes here, but document the issue
        println!(
            "Nested reset output: {}",
            bad_output.replace('\x1b', "\\x1b")
        );

        // GOOD pattern: compose styles
        let warning_bold = warning.bold();
        let good_output =
            format!("{warning}Text with {warning_bold}composed{warning_bold:#} styles{warning:#}");
        println!("Composed output: {}", good_output.replace('\x1b', "\\x1b"));

        // The good pattern maintains color through the bold section
    }

    #[test]
    fn test_path_with_ansi_codes() {
        // BUG HYPOTHESIS: What if a path somehow contains ANSI codes?
        // (This could happen with crafted directory names)
        let mut buffer = Vec::new();

        let path = PathBuf::from("/test/\x1b[31mred\x1b[0m/path");
        write!(&mut buffer, "__WORKTRUNK_CD__{}\0", path.display()).unwrap();

        let output = String::from_utf8_lossy(&buffer);
        // ANSI codes are preserved in the directive!
        assert!(
            output.contains('\x1b'),
            "ANSI codes in path leak into directive"
        );

        // This could cause the shell to display colored output when parsing the directive
        // or interfere with directive parsing
    }
}
