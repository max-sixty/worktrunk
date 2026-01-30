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

/// Strip OSC 8 hyperlinks while preserving other ANSI sequences (colors).
///
/// OSC 8 terminal hyperlinks are great for terminal output (clickable links!), but can cause
/// issues in other contexts:
/// - **Web docs**: `ansi_to_html` only handles SGR codes (colors), not OSC sequences—hyperlinks
///   leak through as garbage text
/// - **Test output**: PTY tests using vt100 parser may show hyperlinks as garbage characters
///   (like `^D`) in rendered terminal output
///
/// Clap's `unstable-markdown` feature (added in 4.5.28) converts markdown links like
/// `[text](url)` in doc comments to OSC 8 terminal hyperlinks. Git on some platforms
/// (notably macOS) also generates OSC 8 hyperlinks in diff output.
///
/// There's no runtime toggle to disable OSC 8 generation. Environment variables like
/// `FORCE_HYPERLINK`, `NO_COLOR`, and `TERM=dumb` have no effect because hyperlink
/// generation is separate from `supports-hyperlinks` and `anstyle-query`.
///
/// Uses the `osc8` crate's parser to find hyperlinks and extract just the visible text.
pub fn strip_osc8_hyperlinks(s: &str) -> String {
    let mut result = s.to_string();
    // Keep parsing and removing hyperlinks until none remain
    while let Ok(Some((_, range))) = Hyperlink::parse(&result) {
        // The range covers the opening escape sequence only.
        // We need to find and remove the closing sequence too.
        let after_open = range.end;
        // Find the closing sequence (empty hyperlink that ends the link)
        if let Ok(Some((_, close_range))) = Hyperlink::parse(&result[after_open..]) {
            // Extract the text between opening and closing sequences
            let text_between = result[after_open..after_open + close_range.start].to_string();
            // Replace the entire hyperlink (open + text + close) with just the text
            let full_end = after_open + close_range.end;
            result.replace_range(range.start..full_end, &text_between);
        } else {
            // Malformed hyperlink (no closing sequence) - just remove the opening
            result.replace_range(range.clone(), "");
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperlink_returns_text_when_not_tty() {
        let result = hyperlink_stdout("https://example.com", "link");
        assert!(result == "link" || result.contains("https://example.com"));
    }

    #[test]
    fn test_strip_osc8_hyperlinks_removes_hyperlink() {
        // OSC 8 format: ESC ] 8 ; params ; URL ST TEXT ESC ] 8 ; ; ST
        // ST can be ESC \ or BEL (\x07)
        let url = "https://example.com";
        let text = "link text";
        let input = format!(
            "before {}{}{}after",
            Hyperlink::new(url),
            text,
            Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "before link textafter");
    }

    #[test]
    fn test_strip_osc8_hyperlinks_preserves_sgr_codes() {
        // SGR codes (colors) like ESC [ 0 m should be preserved
        let url = "https://example.com";
        let text = "link";
        let input = format!(
            "\u{1b}[1m{}{}{}bold\u{1b}[0m",
            Hyperlink::new(url),
            text,
            Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "\u{1b}[1mlinkbold\u{1b}[0m");
    }

    #[test]
    fn test_strip_osc8_hyperlinks_handles_no_hyperlinks() {
        let input = "plain text with no hyperlinks";
        let result = strip_osc8_hyperlinks(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_strip_osc8_hyperlinks_handles_multiple() {
        let input = format!(
            "{}first{} and {}second{}",
            Hyperlink::new("url1"),
            Hyperlink::END,
            Hyperlink::new("url2"),
            Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "first and second");
    }
}
