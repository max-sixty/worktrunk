//! Shared rendering for the picker's `pr` preview pane.
//!
//! Two rows show a PR/MR: a worktree row whose branch has one
//! (`render_worktree_pr` in [`super::items`]) and a `--prs` row
//! ([`super::prs::PrSkimItem`]). Both render the same shape — a bold reference +
//! title header, dim-labeled metadata lines whose values share one column, and
//! the full description rendered flush as markdown — so they read alike.
//! They build from these shared pieces rather than each formatting their own.

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

/// The description block: the full `body` rendered as markdown (bold headers,
/// styled lists and inline code — the same renderer the `summary` tab uses),
/// flush at the pane width rather than quoted in a gutter. The whole body
/// renders; the preview pane scrolls (`ctrl-u`/`ctrl-d`) through a long one.
/// Empty body → empty string, so the block is skipped. The leading `\x1b[0m`
/// is a defensive boundary so the first line renders clean regardless of what
/// precedes it (the metadata lines already reset their own spans).
///
/// `width` is the preview-pane width, which the markdown wraps prose to. The
/// `--prs` pane is built before skim renders, so it passes the list width as a
/// close proxy (Right splits ~50/50; Down gives list and preview the full
/// width); the worktree pane reads the live preview width.
pub(super) fn description(body: &str, width: usize) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let reset = Reset;
    let rendered = crate::md_help::render_markdown_in_help_with_width(body, Some(width));
    format!("\n{reset}{rendered}\n")
}

/// Render `body` as markdown and quote it in the house gutter, returning no
/// leading/trailing newline — the inner form behind the `--prs` comments pane
/// (`prs::render_comment_blocks`), where the gutter sets each comment's body
/// off from its author header. The markdown wraps to the gutter's inner width
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
        // pane shows nothing.
        assert_eq!(description("", 80), "");
        assert_eq!(description("   \n\t \n", 80), "");
    }

    #[test]
    fn description_renders_flush_without_a_gutter() {
        let out = description("Fixes the flaky retry logic.", 80);
        // Leading full reset clears inherited style; the body renders flush, so
        // no house-gutter bg bar (`\x1b[107m`) wraps it.
        assert!(out.starts_with("\n\x1b[0m"), "leading reset: {out:?}");
        assert!(!out.contains("\x1b[107m"), "no gutter bar: {out:?}");
        assert!(
            out.contains("Fixes the flaky retry logic."),
            "body: {out:?}"
        );
    }

    #[test]
    fn description_renders_the_whole_body() {
        // One item per line; the whole body renders with no truncation, so both
        // the first and last item survive and there's no `…` marker.
        let body = (0..50)
            .map(|i| format!("- word{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = description(&body, 80);
        assert!(out.contains("word0"), "head kept: {out:?}");
        assert!(out.contains("word49"), "tail kept: {out:?}");
        assert!(!out.contains('…'), "no truncation marker: {out:?}");
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
