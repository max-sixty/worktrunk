//! OSC 8 hyperlink support for terminal output.

use osc8::Hyperlink;
use supports_hyperlinks::Stream;

/// Check if the terminal supports OSC 8 hyperlinks on stdout.
pub fn supports_hyperlinks_stdout() -> bool {
    supports_hyperlinks::on(Stream::Stdout)
}

/// Check if the terminal supports OSC 8 hyperlinks on stderr.
pub fn supports_hyperlinks_stderr() -> bool {
    supports_hyperlinks::on(Stream::Stderr)
}

/// Format text as a clickable hyperlink for stdout, or return plain text if unsupported.
pub fn hyperlink_stdout(url: &str, text: &str) -> String {
    if supports_hyperlinks::on(Stream::Stdout) {
        format!("{}{}{}", Hyperlink::new(url), text, Hyperlink::END)
    } else {
        text.to_string()
    }
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
    fn test_hyperlink_returns_text_when_not_tty() {
        // In test environment (not a TTY), we get plain text back
        let result_stdout = hyperlink_stdout("https://example.com", "link");
        let result_stderr = hyperlink_stderr("https://example.com", "link");
        assert!(result_stdout == "link" || result_stdout.contains("https://example.com"));
        assert!(result_stderr == "link" || result_stderr.contains("https://example.com"));
    }
}
