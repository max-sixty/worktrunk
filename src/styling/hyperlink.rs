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

/// Check if the terminal supports OSC 8 hyperlinks on stdout.
pub fn supports_hyperlinks_stdout() -> bool {
    supports_hyperlinks::on(Stream::Stdout)
}

/// Format text as a clickable hyperlink if supported, otherwise return just the text.
///
/// When hyperlinks are supported, returns an OSC 8 sequence that makes `text` clickable
/// and links to `url`. When not supported, returns just `text`.
///
/// # Arguments
/// * `url` - The URL to link to
/// * `text` - The visible text to display
/// * `stream` - Which stream to check for hyperlink support (Stderr for status messages, Stdout for data)
pub fn hyperlink(url: &str, text: &str, stream: Stream) -> String {
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

/// Format text as a clickable hyperlink for stdout output.
///
/// Convenience wrapper for `hyperlink(url, text, Stream::Stdout)`.
pub fn hyperlink_stdout(url: &str, text: &str) -> String {
    hyperlink(url, text, Stream::Stdout)
}

/// Get a description of why hyperlinks are or aren't supported.
///
/// Returns a human-readable explanation of the hyperlink support detection result,
/// useful for diagnostics output. The result explains the terminal detection,
/// which may differ from the final `supports_hyperlinks_stderr()` result if
/// the output is not a TTY.
pub fn hyperlink_support_reason() -> (&'static str, Option<String>) {
    // Check for explicit override first
    if let Ok(val) = std::env::var("FORCE_HYPERLINK") {
        if val == "1" || val.eq_ignore_ascii_case("true") {
            return ("Forced on", Some("FORCE_HYPERLINK=1".to_string()));
        }
        if val == "0" || val.eq_ignore_ascii_case("false") {
            return ("Forced off", Some("FORCE_HYPERLINK=0".to_string()));
        }
    }

    // Check known terminals in order of detection priority
    // (matching supports-hyperlinks crate logic)

    if std::env::var("DOMTERM").is_ok() {
        return ("DomTerm", Some("DOMTERM".to_string()));
    }

    if let Ok(version) = std::env::var("VTE_VERSION")
        && let Ok(v) = version.parse::<u32>()
        && v >= 5000
    {
        return ("VTE-based", Some(format!("VTE_VERSION={version}")));
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let supported = matches!(
            term_program.as_str(),
            "Hyper" | "iTerm.app" | "terminology" | "WezTerm" | "vscode" | "ghostty" | "zed"
        );
        if supported {
            return ("Detected", Some(format!("TERM_PROGRAM={term_program}")));
        }
    }

    if let Ok(term) = std::env::var("TERM")
        && (term == "xterm-kitty" || term.starts_with("alacritty"))
    {
        return ("Detected", Some(format!("TERM={term}")));
    }

    if let Ok(colorterm) = std::env::var("COLORTERM")
        && colorterm == "xfce4-terminal"
    {
        return ("Detected", Some(format!("COLORTERM={colorterm}")));
    }

    if std::env::var("WT_SESSION").is_ok() {
        return ("Windows Terminal", Some("WT_SESSION".to_string()));
    }

    if std::env::var("KONSOLE_VERSION").is_ok() {
        return ("Konsole", Some("KONSOLE_VERSION".to_string()));
    }

    // No detection matched
    ("Not detected", None)
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
    fn test_hyperlink_support_reason_returns_something() {
        // Should always return a reason, even if detection fails
        let (reason, _env_var) = hyperlink_support_reason();
        assert!(!reason.is_empty());
    }
}
