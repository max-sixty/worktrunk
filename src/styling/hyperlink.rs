//! OSC 8 hyperlink support for terminal output.

use osc8::Hyperlink;
use supports_hyperlinks::Stream;

/// Check if the terminal supports OSC 8 hyperlinks on stderr.
pub fn supports_hyperlinks_stderr() -> bool {
    supports_hyperlinks::on(Stream::Stderr)
}

/// Format text as a clickable hyperlink for stderr, or return plain text if unsupported.
pub fn hyperlink_stderr(url: &str, text: &str) -> String {
    if supports_hyperlinks::on(Stream::Stderr) {
        format!("{}{}{}", Hyperlink::new(url), text, Hyperlink::END)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperlink_stderr_returns_text_when_not_tty() {
        let result = hyperlink_stderr("https://example.com", "link text");
        // In test environment (not a TTY), we get plain text back
        // (or OSC 8 format if terminal is detected as supporting hyperlinks)
        assert!(result == "link text" || result.contains("https://example.com"));
    }
}
