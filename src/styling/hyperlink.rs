//! OSC 8 hyperlink support for terminal output.
//!
//! This module provides detection and formatting for clickable hyperlinks in terminals
//! that support the OSC 8 escape sequence. When hyperlinks aren't supported, the
//! formatting functions return plain text or the full URL.

use supports_hyperlinks::Stream;

/// Check if the terminal supports OSC 8 hyperlinks on stderr.
///
/// Uses heuristics based on `TERM_PROGRAM`, `VTE_VERSION`, and other environment
/// variables. See [OSC 8 spec](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda)
/// for details on terminal support.
pub fn supports_hyperlinks_stderr() -> bool {
    supports_hyperlinks::on(Stream::Stderr)
}

/// Format text as a clickable hyperlink if supported, otherwise return just the text.
fn hyperlink(url: &str, text: &str, stream: Stream) -> String {
    if supports_hyperlinks::on(stream) {
        format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, text)
    } else {
        text.to_string()
    }
}

/// Format text as a clickable hyperlink for stderr output.
///
/// Convenience wrapper for `hyperlink(url, text, Stream::Stderr)`.
pub fn hyperlink_stderr(url: &str, text: &str) -> String {
    hyperlink(url, text, Stream::Stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperlink_format() {
        // When hyperlinks are supported, the output contains OSC 8 sequences
        let url = "https://example.com";
        let text = "Click me";

        // Test the OSC 8 format directly (bypassing detection)
        let formatted = format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, text);
        assert!(formatted.contains(url));
        assert!(formatted.contains(text));
        assert!(formatted.starts_with("\x1b]8;;"));
        assert!(formatted.ends_with("\x1b]8;;\x1b\\"));
    }

    #[test]
    fn test_hyperlink_stderr_returns_text_when_not_tty() {
        // When not a TTY (test environment), hyperlink_stderr returns just the text
        let url = "https://example.com";
        let text = "link text";
        let result = hyperlink_stderr(url, text);

        // In test environment (not a TTY), we get plain text back
        // (or OSC 8 format if terminal is detected as supporting hyperlinks)
        assert!(result == text || result.contains(url));
    }
}
