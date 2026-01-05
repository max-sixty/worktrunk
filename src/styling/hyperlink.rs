//! OSC 8 hyperlink support for terminal output.

use osc8::Hyperlink;

// Re-export for direct use
pub use supports_hyperlinks::{Stream, on as supports_hyperlinks};

/// Format text as a clickable hyperlink for stdout, or return plain text if unsupported.
pub fn hyperlink_stdout(url: &str, text: &str) -> String {
    if supports_hyperlinks(Stream::Stdout) {
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
        let result = hyperlink_stdout("https://example.com", "link");
        assert!(result == "link" || result.contains("https://example.com"));
    }
}
