//! Styled line and string types for composable terminal output
//!
//! Provides types for building complex styled output with proper width calculation.

use ansi_str::AnsiStr;
use anstyle::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate a styled string to a visible width budget, preserving escapes.
/// Escape sequences (ANSI/OSC) are zero-width; ellipsis is added when truncating.
/// Appends ESC[0m on truncation to avoid style bleed.
pub fn truncate_visible(rendered: &str, max_width: usize, ellipsis: &str) -> String {
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
        let truncated = truncate_visible(&rendered, max_width, "…");
        let mut new_line = StyledLine::new();
        new_line.push_raw(truncated);
        new_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_width_strips_osc_hyperlinks() {
        // Text with OSC 8 hyperlink should have visual width of just the text
        let url = "https://github.com/user/repo/pull/123";
        let text_content = "●";
        let hyperlinked = format!(
            "{}{}{}",
            osc8::Hyperlink::new(url),
            text_content,
            osc8::Hyperlink::END
        );

        let styled_str = StyledString::raw(&hyperlinked);
        assert_eq!(
            styled_str.width(),
            1,
            "Hyperlinked '●' should have width 1, not {}",
            styled_str.width()
        );
    }

    #[test]
    fn test_width_strips_sgr_codes() {
        // Text with SGR color codes should have visual width of just the text
        use anstyle::{AnsiColor, Color, Style};
        let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
        let colored = format!("{}●{}", green.render(), green.render_reset());

        let styled_str = StyledString::raw(colored);
        assert_eq!(
            styled_str.width(),
            1,
            "Colored '●' should have width 1, not {}",
            styled_str.width()
        );
    }

    #[test]
    fn test_width_with_combined_ansi_codes() {
        // Text with both color and hyperlink
        use anstyle::{AnsiColor, Color, Style};
        let url = "https://example.com";
        let yellow = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        let combined = format!(
            "{}{}● passed{}{}",
            yellow.render(),
            osc8::Hyperlink::new(url),
            osc8::Hyperlink::END,
            yellow.render_reset()
        );

        let styled_str = StyledString::raw(&combined);
        // "● passed" = 1 + 1 (space) + 6 = 8
        assert_eq!(
            styled_str.width(),
            8,
            "Combined styled text should have width 8, not {}",
            styled_str.width()
        );
    }

    /// Helper to compute visible width for tests
    fn visible_width(rendered: &str) -> usize {
        UnicodeWidthStr::width(rendered.ansi_strip().as_ref())
    }

    #[test]
    fn test_visible_width_strips_osc8() {
        let s = "\u{1b}]8;;https://example.com\u{1b}\\A\u{1b}]8;;\u{1b}\\";
        assert_eq!(visible_width(s), 1, "OSC-8 should be zero-width");
    }

    #[test]
    fn test_truncate_visible_preserves_budget_and_resets() {
        let colored = "\u{1b}[31mhello\u{1b}[0m";
        let out = truncate_visible(colored, 3, "…");
        assert_eq!(visible_width(&out), 3);
        assert!(out.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn test_truncate_visible_handles_wide_emoji() {
        let rocket = "🚀";
        let out = truncate_visible(rocket, 1, "…");
        assert_eq!(visible_width(&out), 1);
        assert!(out.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn test_truncate_visible_zero_width() {
        let text = "hello world";
        let out = truncate_visible(text, 0, "…");
        assert!(out.is_empty(), "Zero width should return empty string");
    }

    #[test]
    fn test_truncate_visible_no_truncation_needed() {
        let text = "short";
        let out = truncate_visible(text, 100, "…");
        assert_eq!(out, text, "No truncation should return original string");
    }

    #[test]
    fn test_truncate_visible_zero_budget() {
        // When max_width equals ellipsis width, budget becomes 0
        let text = "hello";
        let out = truncate_visible(text, 1, "…");
        assert!(
            visible_width(&out) <= 1,
            "Output should be within max_width"
        );
    }

    #[test]
    fn test_styled_string_raw() {
        let s = StyledString::raw("test");
        assert_eq!(s.text, "test");
        assert!(s.style.is_none());
    }

    #[test]
    fn test_styled_string_styled() {
        let style = Style::new().bold();
        let s = StyledString::styled("test", style);
        assert_eq!(s.text, "test");
        assert!(s.style.is_some());
    }

    #[test]
    fn test_styled_string_render_raw() {
        let s = StyledString::raw("test");
        assert_eq!(s.render(), "test");
    }

    #[test]
    fn test_styled_string_render_styled() {
        let style = Style::new().bold();
        let s = StyledString::styled("test", style);
        let rendered = s.render();
        assert!(rendered.contains("test"));
        // Should have escape codes for bold
        assert!(rendered.starts_with("\u{1b}["));
    }

    #[test]
    fn test_styled_line_push_methods() {
        let mut line = StyledLine::new();
        line.push_raw("hello");
        line.push_styled(" world", Style::new().bold());
        line.push(StyledString::raw("!"));

        assert_eq!(line.segments.len(), 3);
        assert_eq!(line.width(), 12); // "hello world!" = 12
    }

    #[test]
    fn test_styled_line_extend() {
        let mut line1 = StyledLine::new();
        line1.push_raw("hello");

        let mut line2 = StyledLine::new();
        line2.push_raw(" world");

        line1.extend(line2);
        assert_eq!(line1.segments.len(), 2);
        assert_eq!(line1.plain_text(), "hello world");
    }

    #[test]
    fn test_styled_line_pad_to() {
        let mut line = StyledLine::new();
        line.push_raw("hi");
        assert_eq!(line.width(), 2);

        line.pad_to(5);
        assert_eq!(line.width(), 5);
        assert!(line.plain_text().ends_with("   ")); // 3 spaces added
    }

    #[test]
    fn test_styled_line_pad_to_no_padding_needed() {
        let mut line = StyledLine::new();
        line.push_raw("hello");
        let original_width = line.width();

        line.pad_to(3); // Target is less than current width
        assert_eq!(line.width(), original_width); // Should not change
    }

    #[test]
    fn test_styled_line_render() {
        let mut line = StyledLine::new();
        line.push_raw("a");
        line.push_styled("b", Style::new().bold());
        line.push_raw("c");

        let rendered = line.render();
        assert!(rendered.contains("a"));
        assert!(rendered.contains("b"));
        assert!(rendered.contains("c"));
    }

    #[test]
    fn test_styled_line_plain_text() {
        let mut line = StyledLine::new();
        line.push_raw("hello");
        line.push_styled(" world", Style::new().bold());

        assert_eq!(line.plain_text(), "hello world");
    }

    #[test]
    fn test_styled_line_truncate_to_width_no_truncation() {
        let mut line = StyledLine::new();
        line.push_raw("hello");
        let truncated = line.clone().truncate_to_width(100);
        assert_eq!(truncated.plain_text(), "hello");
    }

    #[test]
    fn test_styled_line_truncate_to_width_truncates() {
        let mut line = StyledLine::new();
        line.push_raw("hello world this is a long message");
        let truncated = line.truncate_to_width(10);
        assert!(truncated.width() <= 10);
    }

    #[test]
    fn test_styled_line_default() {
        let line = StyledLine::default();
        assert!(line.segments.is_empty());
        assert_eq!(line.width(), 0);
    }
}
