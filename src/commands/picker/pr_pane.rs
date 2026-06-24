//! Shared rendering for the picker's `pr` preview pane.
//!
//! Two rows show a PR/MR: a worktree row whose branch has one
//! (`render_worktree_pr` in [`super::items`]) and a `--prs` row
//! ([`super::prs::PrSkimItem`]). Both render the same shape — a bold reference +
//! title header, dim-labeled metadata lines whose values share one column, and
//! the description as markdown inside the house gutter — so they read alike.
//! They build from these shared pieces rather than each formatting their own.

use ansi_str::AnsiStr;
use anstyle::Reset;
use color_print::cformat;
use worktrunk::styling::format_with_gutter;

use super::super::list::ci_status::PrRef;

/// Column (in cells) where a metadata line's value begins, after its dim label.
/// The widest labels (`branch`/`author`, 6) plus a 3-space gap; shorter labels
/// (`url`, `state`) pad out to the same column so every value lines up — across
/// lines and across the two panes.
const VALUE_COLUMN: usize = 9;

/// The pane header: a bold PR/MR reference, the title when known, then a blank
/// line. A title-less status (an old cache entry, or a fetch that didn't carry
/// one) renders just the reference.
pub(super) fn header(pr_ref: PrRef, title: Option<&str>) -> String {
    let reset = Reset;
    match title {
        Some(title) => cformat!("<bold>{pr_ref}</>{reset}  {title}\n\n"),
        None => cformat!("<bold>{pr_ref}</>{reset}\n\n"),
    }
}

/// One dim-labeled metadata line (`branch`, `author`, `url`, …). The label pads
/// so the value starts at [`VALUE_COLUMN`], aligning values down the pane and
/// between the two panes. `value` may carry its own styling (e.g. a yellow
/// `draft`) and must close its own spans. A full `{reset}` after the label keeps
/// the dim from bleeding into the value (skim's ANSI parser drops color_print's
/// `</>`); see [`super::items::render_preview_tabs`].
pub(super) fn metadata_line(label: &str, value: &str) -> String {
    let reset = Reset;
    let pad = " ".repeat(VALUE_COLUMN.saturating_sub(label.len()));
    let label = cformat!("<dim>{label}</>{reset}");
    format!("{label}{pad}{value}\n")
}

/// Gutter rows the description preview keeps before trimming. The body is a
/// preview, not the canonical copy, so a long one is cut here rather than left
/// to scroll the pane; the `url` metadata line reads the rest.
const MAX_PREVIEW_ROWS: usize = 6;

/// The description block: `body` rendered as markdown (bold headers, styled
/// lists and inline code — the same renderer the `summary` tab uses) and quoted
/// in the house gutter ([`format_with_gutter`], a bg-color bar that closes each
/// line with a full `\x1b[0m`, skim-safe). A short body renders whole; a long
/// one is trimmed to the first [`MAX_PREVIEW_ROWS`] gutter rows, closed by a dim
/// `…` row (the picker's truncation marker), with the `url` metadata line above
/// reading the rest. Empty body → empty string, so the block is skipped. The
/// leading `\x1b[0m` is a defensive boundary so the first gutter line renders
/// clean regardless of what precedes it (the metadata lines already reset their
/// own spans).
///
/// Trimming bounds the rendered height rather than the source: the cut lands on
/// gutter-row boundaries (each row self-closes with `\x1b[0m`, so no style
/// bleeds past the cut), which also caps a long single-paragraph body that
/// arrives as one soft-wrapped source line.
///
/// `width` is the preview-pane width. The `--prs` pane is built before skim
/// renders, so it passes the list width as a close proxy (Right splits ~50/50;
/// Down gives list and preview the full width); the worktree pane reads the live
/// preview width. The markdown wraps to the gutter's inner width (the bar plus
/// its pad take two columns) so the gutter's own wrap is a no-op rather than
/// re-breaking the already-styled lines.
pub(super) fn description(body: &str, width: usize) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let reset = Reset;
    let rendered = markdown_in_gutter(body, width);
    let rows: Vec<&str> = rendered.lines().collect();
    // Trim only when real content sits past the budget — trailing blank gutter
    // rows (a markdown render's closing newlines) don't earn a `…` marker.
    let truncated = rows
        .iter()
        .skip(MAX_PREVIEW_ROWS)
        .any(|r| !r.ansi_strip().trim().is_empty());
    if !truncated {
        return format!("\n{reset}{rendered}\n");
    }
    let mut kept: Vec<&str> = rows.into_iter().take(MAX_PREVIEW_ROWS).collect();
    // Don't leave the `…` trailing a blank gutter bar.
    while kept
        .last()
        .is_some_and(|r| r.ansi_strip().trim().is_empty())
    {
        kept.pop();
    }
    let more = format_with_gutter(&cformat!("<dim>…</>{reset}"), Some(width));
    format!("\n{reset}{}\n{more}\n", kept.join("\n"))
}

/// Render `body` as markdown and quote it in the house gutter, returning no
/// leading/trailing newline — the shared inner form behind [`description`] and
/// the `--prs` comments pane (`prs::render_comment_blocks`), so the PR/MR body
/// and each comment read alike. The markdown wraps to the gutter's inner width
/// (the bar plus its pad take two columns) so the gutter's own wrap is a no-op
/// rather than re-breaking the already-styled lines.
pub(super) fn markdown_in_gutter(body: &str, width: usize) -> String {
    let rendered =
        crate::md_help::render_markdown_in_help_with_width(body, Some(width.saturating_sub(2)));
    format_with_gutter(&rendered, Some(width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_with_and_without_title() {
        let with = header(PrRef::pr(42), Some("Fix the flaky test"));
        assert!(with.contains("#42"), "reference: {with:?}");
        assert!(with.contains("Fix the flaky test"), "title: {with:?}");
        assert!(with.ends_with("\n\n"), "blank line after header: {with:?}");

        // A title-less status renders just the reference: the styled `#42`
        // closes with a full reset, then the blank line — no trailing spaces
        // where the title would be.
        let without = header(PrRef::pr(42), None);
        assert!(without.contains("#42"), "reference: {without:?}");
        assert!(
            without.ends_with("\x1b[0m\n\n"),
            "ends right after the styled reference: {without:?}"
        );
        use ansi_str::AnsiStr;
        assert_eq!(
            without.ansi_strip(),
            "#42\n\n",
            "no title slot: {without:?}"
        );
    }

    #[test]
    fn metadata_line_aligns_values_to_one_column() {
        use ansi_str::AnsiStr;
        // The value column is fixed regardless of label length, so a short label
        // (`url`) and a long one (`branch`) put their values at the same column.
        let url = metadata_line("url", "https://example.com")
            .ansi_strip()
            .to_string();
        let branch = metadata_line("branch", "feature/auth")
            .ansi_strip()
            .to_string();
        assert_eq!(
            url.find("https"),
            Some(VALUE_COLUMN),
            "url value at the shared column: {url:?}"
        );
        assert_eq!(
            branch.find("feature"),
            Some(VALUE_COLUMN),
            "branch value at the shared column: {branch:?}"
        );
    }

    #[test]
    fn description_empty_or_blank_renders_nothing() {
        // No body, or whitespace-only — the block is skipped entirely so the
        // pane doesn't show an empty gutter.
        assert_eq!(description("", 80), "");
        assert_eq!(description("   \n\t \n", 80), "");
    }

    #[test]
    fn description_wraps_into_the_house_gutter() {
        let out = description("Fixes the flaky retry logic.", 80);
        // Leading full reset clears inherited style; the house gutter sets a
        // bg color and closes each line with a skim-safe `\x1b[0m`.
        assert!(out.starts_with("\n\x1b[0m"), "leading reset: {out:?}");
        assert!(out.contains("\x1b[107m"), "house gutter bg: {out:?}");
        assert!(
            out.contains("Fixes the flaky retry logic."),
            "body: {out:?}"
        );
    }

    #[test]
    fn description_keeps_a_short_body_whole() {
        // A body that fits the row budget renders in full, with no `…` marker.
        let body = "- one\n- two\n- three";
        let out = description(body, 80);
        assert!(out.contains("one"), "head kept: {out:?}");
        assert!(out.contains("three"), "tail kept: {out:?}");
        assert!(!out.contains('…'), "no truncation marker: {out:?}");
    }

    #[test]
    fn description_trims_a_long_body() {
        use ansi_str::AnsiStr;
        // One item per line so each renders as its own gutter row; well past the
        // budget, so the head is kept, the tail is dropped, and a `…` marks the
        // cut. The body carries no `…` of its own, so finding one proves the
        // marker.
        let body = (0..50)
            .map(|i| format!("- word{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = description(&body, 80);
        assert!(out.contains("word0"), "head kept: {out:?}");
        assert!(!out.contains("word49"), "tail dropped: {out:?}");
        assert!(out.contains('…'), "truncation marker: {out:?}");
        // The marker is the last row, so nothing renders below it.
        let last = out.trim_end().lines().last().unwrap_or_default();
        assert!(
            last.ansi_strip().trim().ends_with('…'),
            "marker last: {out:?}"
        );
    }

    #[test]
    fn description_renders_markdown() {
        // Markdown is styled, not shown verbatim: a bold span carries the SGR-1
        // termimad emits, and the literal `**` markers are gone.
        let out = description("Fixes the **flaky** retry.", 80);
        assert!(out.contains("\x1b[1m"), "bold rendered: {out:?}");
        assert!(!out.contains("**"), "markers consumed: {out:?}");
    }
}
