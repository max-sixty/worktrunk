//! Styled line and string types for composable terminal output
//!
//! Provides types for building complex styled output with proper width calculation.

use ansi_str::AnsiStr;
use anstyle::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate a styled string to a visible width budget, preserving escapes.
/// Escape sequences (ANSI/OSC) are zero-width; ellipsis ("…") is added when truncating.
/// Appends ESC[0m on truncation to avoid style bleed.
pub fn truncate_visible(rendered: &str, max_width: usize) -> String {
    truncate_visible_with_ellipsis(rendered, max_width, "…")
}

/// Truncate a styled string with a custom ellipsis character.
fn truncate_visible_with_ellipsis(rendered: &str, max_width: usize, ellipsis: &str) -> String {
    if max_width == 0 {
        return String::new();
    }

    let plain = rendered.ansi_strip();
    let plain_str = plain.as_ref();
    if UnicodeWidthStr::width(plain_str) <= max_width {
        return rendered.to_owned();
    }

    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    let budget = max_width.saturating_sub(ellipsis_width);
    if budget == 0 {
        let mut out = String::new();
        out.push_str(ellipsis);
        out.push_str("\u{1b}[0m");
        return out;
    }

    let mut cut_at = 0;
    let mut width = 0;
    for (i, ch) in plain_str.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        cut_at = i + ch.len_utf8();
    }

    let mut out = rendered.ansi_cut(..cut_at).into_owned();
    out.push_str(ellipsis);
    out.push_str("\u{1b}[0m");
    out
}

/// A piece of text with an optional style
#[derive(Clone, Debug)]
pub struct StyledString {
    pub text: String,
    pub style: Option<Style>,
}

impl StyledString {
    fn new(text: impl Into<String>, style: Option<Style>) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(text, None)
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(text, Some(style))
    }

    /// Returns the visual width (unicode-aware, ANSI codes stripped)
    pub fn width(&self) -> usize {
        self.text.ansi_strip().width()
    }

    /// Renders to a string with ANSI escape codes
    pub fn render(&self) -> String {
        if let Some(style) = &self.style {
            format!("{}{}{}", style.render(), self.text, style.render_reset())
        } else {
            self.text.clone()
        }
    }
}

/// A line composed of multiple styled strings
#[derive(Clone, Debug, Default)]
pub struct StyledLine {
    pub segments: Vec<StyledString>,
}

impl StyledLine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a raw (unstyled) segment
    pub fn push_raw(&mut self, text: impl Into<String>) {
        self.segments.push(StyledString::raw(text));
    }

    /// Add a styled segment
    pub fn push_styled(&mut self, text: impl Into<String>, style: Style) {
        self.segments.push(StyledString::styled(text, style));
    }

    /// Add a segment (StyledString)
    pub fn push(&mut self, segment: StyledString) {
        self.segments.push(segment);
    }

    /// Append every segment from another styled line.
    pub fn extend(&mut self, other: StyledLine) {
        self.segments.extend(other.segments);
    }

    /// Pad with spaces to reach a specific width
    pub fn pad_to(&mut self, target_width: usize) {
        let current_width = self.width();
        if current_width < target_width {
            self.push_raw(" ".repeat(target_width - current_width));
        }
    }

    /// Returns the total visual width
    pub fn width(&self) -> usize {
        self.segments.iter().map(|s| s.width()).sum()
    }

    /// Renders the entire line with ANSI escape codes
    pub fn render(&self) -> String {
        self.segments.iter().map(|s| s.render()).collect()
    }

    /// Returns the plain text without any styling
    pub fn plain_text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// Truncate if the line exceeds the given width, preserving ANSI codes.
    /// Returns a new StyledLine with truncated content and ellipsis.
    pub fn truncate_to_width(self, max_width: usize) -> StyledLine {
        if self.width() <= max_width {
            return self;
        }
        let rendered = self.render();
        let truncated = truncate_visible(&rendered, max_width);
        let mut new_line = StyledLine::new();
        new_line.push_raw(truncated);
        new_line
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    /// Width calculation ignores ANSI escape codes and OSC 8 hyperlinks.
    #[test]
    fn test_width_ignores_invisible_characters() {
        // OSC 8 hyperlink: visual width is just the link text
        let url = "https://github.com/user/repo/pull/123";
        let hyperlinked = format!(
            "{}{}{}",
            osc8::Hyperlink::new(url),
            "●",
            osc8::Hyperlink::END
        );
        assert_eq!(StyledString::raw(&hyperlinked).width(), 1);

        // SGR color codes are invisible
        use anstyle::{AnsiColor, Color, Style};
        let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
        let colored = format!("{}●{}", green.render(), green.render_reset());
        assert_eq!(StyledString::raw(colored).width(), 1);

        // Combined color + hyperlink
        let yellow = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        let combined = format!(
            "{}{}● passed{}{}",
            yellow.render(),
            osc8::Hyperlink::new("https://example.com"),
            osc8::Hyperlink::END,
            yellow.render_reset()
        );
        assert_eq!(StyledString::raw(&combined).width(), 8); // "● passed"

        // OSC-8 via raw escape sequences
        let s = "\u{1b}]8;;https://example.com\u{1b}\\A\u{1b}]8;;\u{1b}\\";
        assert_eq!(UnicodeWidthStr::width(s.ansi_strip().as_ref()), 1,);
    }

    /// truncate_visible respects visual width, handles emoji, and appends reset codes.
    #[test]
    fn test_truncate_visible() {
        use ansi_str::AnsiStr;

        let visible_width = |s: &str| UnicodeWidthStr::width(s.ansi_strip().as_ref());

        // Truncates colored text to budget, ends with reset
        let colored = "\u{1b}[31mhello\u{1b}[0m";
        let out = truncate_visible(colored, 3);
        assert_eq!(visible_width(&out), 3);
        assert!(out.ends_with("\u{1b}[0m"));

        // Wide emoji (width 2) truncated to budget 1
        let out = truncate_visible("🚀", 1);
        assert_eq!(visible_width(&out), 1);

        // Zero width → empty
        assert!(truncate_visible("hello world", 0).is_empty());

        // No truncation when text fits
        assert_eq!(truncate_visible("short", 100), "short");

        // Budget of 1 stays within limit
        let out = truncate_visible("hello", 1);
        assert!(visible_width(&out) <= 1);
    }

    /// StyledLine composition: push, extend, render, plain_text all produce correct output.
    #[test]
    fn test_styled_line_composition() {
        let mut line = StyledLine::new();
        line.push_raw("hello");
        line.push_styled(" world", Style::new().bold());
        line.push(StyledString::raw("!"));

        assert_eq!(line.segments.len(), 3);
        assert_eq!(line.width(), 12);
        assert_eq!(line.plain_text(), "hello world!");

        // render() includes ANSI codes but preserves text
        assert_snapshot!(line.render(), @"hello[1m world[0m!");

        // extend merges segments
        let mut a = StyledLine::new();
        a.push_raw("hello");
        let mut b = StyledLine::new();
        b.push_raw(" world");
        a.extend(b);
        assert_eq!(a.plain_text(), "hello world");
    }

    /// pad_to adds spaces up to target width; never shrinks.
    #[test]
    fn test_styled_line_pad_to() {
        let mut line = StyledLine::new();
        line.push_raw("hi");
        line.pad_to(5);
        assert_eq!(line.width(), 5);
        assert!(line.plain_text().ends_with("   "));

        // Already wider than target: no change
        line.pad_to(3);
        assert_eq!(line.width(), 5);
    }

    /// truncate_to_width clips long lines and preserves short ones.
    #[test]
    fn test_styled_line_truncate_to_width() {
        let mut short = StyledLine::new();
        short.push_raw("hello");
        assert_eq!(short.clone().truncate_to_width(100).plain_text(), "hello");

        let mut long = StyledLine::new();
        long.push_raw("hello world this is a long message");
        assert!(long.truncate_to_width(10).width() <= 10);
    }

    /// StyledString render with style includes ANSI escape codes.
    #[test]
    fn test_styled_string_render_styled() {
        let style = Style::new().bold();
        let s = StyledString::styled("test", style);
        assert_snapshot!(s.render(), @"[1mtest[0m");
    }
}
