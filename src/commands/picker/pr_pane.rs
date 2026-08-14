//! Shared rendering for the picker's `pr` preview pane.
//!
//! Both picker row kinds show a PR/MR through one renderer
//! (`render_pr_pane_body` in [`super::items`]): a worktree row whose branch has
//! one, and a listed `--prs` row (a `PickerRow` with no local checkout). They
//! render the same shape — a bold reference + title header, cyan all-caps
//! labeled metadata lines whose values share one column, and a matching
//! `DESCRIPTION` heading above the full body rendered flush as markdown — so
//! they read alike.
//! They build from these shared pieces rather than each formatting their own.
//! Every label (`BRANCH`, `URL`, `DESCRIPTION`, …) goes through [`field_label`],
//! which renders the app's cyan all-caps title style, so they all match the
//! headings elsewhere in the CLI (`wt config show`, `wt step`, …).
//! The values carry the app's own conventions in turn: the branch name bold and
//! the url underlined (via [`branch_line`]/[`url_line`]), the same way branches
//! and links render across the rest of the CLI.

use anstyle::Reset;
use color_print::cformat;
use worktrunk::styling::{format_heading, format_with_gutter};

use super::super::list::ci_status::PrRef;

/// Column (in cells) where a metadata line's value begins, after its cyan label.
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

/// A field label (`BRANCH`, `URL`, `DESCRIPTION`, …) in the app's cyan all-caps
/// title style, rendered through [`format_heading`] so it matches the section
/// headings across the CLI (`wt config show`, `wt step`, …). A trailing full
/// `{reset}` closes the span: skim's ANSI parser drops color_print's `</>` (the
/// SGR 39 `format_heading` itself emits), so the cyan would otherwise bleed into
/// the value or body (see [`super::items::render_preview_tabs`]). Every label in
/// the pane goes through this one helper, so they all render identically.
fn field_label(text: &str) -> String {
    let reset = Reset;
    format!("{}{reset}", format_heading(&text.to_uppercase(), None))
}

/// One cyan all-caps labeled metadata line (`BRANCH`, `AUTHOR`, `URL`, …). The
/// label pads so the value starts at [`VALUE_COLUMN`], aligning values down the
/// pane and between the two panes. `value` may carry its own styling (e.g. a
/// yellow `draft`) and must close its own spans.
pub(super) fn metadata_line(label: &str, value: &str) -> String {
    let pad = " ".repeat(VALUE_COLUMN.saturating_sub(label.len()));
    format!("{}{pad}{value}\n", field_label(label))
}

/// The `BRANCH` metadata line, with the branch name bold — the app convention
/// for branch identifiers (git and error messages throughout the CLI, the
/// branch summary in `src/summary.rs`). Both panes build
/// the line through here so the styling can't drift between them. The trailing
/// full `{reset}` closes the bold span: skim's ANSI parser drops the SGR 22 that
/// color_print's `</>` emits, exactly as the `DESCRIPTION` label and the `draft`
/// state value handle their own closers.
pub(super) fn branch_line(branch: &str) -> String {
    let reset = Reset;
    metadata_line("branch", &cformat!("<bold>{branch}</>{reset}"))
}

/// The `URL` metadata line, with the url underlined — the app convention for
/// inline links and references (hints, the fork-push notice). Both panes build
/// the line through here so the styling can't drift. The trailing full `{reset}`
/// closes the underline span (skim drops the SGR 24 that `</>` emits).
pub(super) fn url_line(url: &str) -> String {
    let reset = Reset;
    metadata_line("url", &cformat!("<underline>{url}</>{reset}"))
}

/// The description block: a cyan all-caps `DESCRIPTION` label (via
/// [`field_label`], so it matches the metadata labels above it) over the full
/// `body` rendered flush as markdown (bold headers, styled lists, inline code;
/// fenced code blocks dim and flush) via
/// [`render_markdown_flush`](crate::md_help::render_markdown_flush) —
/// nothing is quoted in the house gutter, so the whole body sits flush-left.
/// Blank lines set the label off from the inline metadata above and from its
/// body below. The whole body renders; the preview pane scrolls
/// (`ctrl-u`/`ctrl-d`) through a long one. Empty body → empty string, so the
/// block (label included) is skipped. The leading `\x1b[0m` is a defensive
/// boundary so the label renders clean regardless of what precedes it (the
/// metadata lines already reset their own spans).
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
    let label = field_label("description");
    let rendered = crate::md_help::render_markdown_flush(body, Some(width));
    format!("\n{reset}{label}\n\n{rendered}\n")
}

/// Render `body` as markdown and quote it in the house gutter, returning no
/// leading/trailing newline — the inner form behind the `--prs` comments pane
/// (`prs::render_comment_blocks`), where the gutter sets each comment's body
/// off from its author header. The markdown wraps to the gutter's inner width
/// (the bar plus its pad take two columns) so the gutter's own wrap is a no-op
/// rather than re-breaking the already-styled lines.
///
/// The body renders *flush* ([`render_markdown_flush`](crate::md_help::render_markdown_flush)),
/// not gutter-quoted: a comment's own fenced code block would otherwise get its
/// own house gutter from the markdown renderer, and quoting that inside this
/// gutter nests two bars — the outer re-wrap then strands continuation lines
/// without a bar and drops their dim. Flush keeps a single gutter, and flush
/// code blocks pre-wrap to the inner width so this gutter's wrap stays a no-op.
pub(super) fn markdown_in_gutter(body: &str, width: usize) -> String {
    let rendered = crate::md_help::render_markdown_flush(body, Some(width.saturating_sub(2)));
    format_with_gutter(&rendered, Some(width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_in_gutter_code_block_stays_single_guttered_and_dim() {
        use worktrunk::styling::GUTTER;
        // A comment whose body wraps a long line in a fenced code block. The
        // renderer must not nest a second house bar (the code block's own gutter
        // inside this one), which used to strand continuation lines bar-less and
        // un-dimmed and break the alignment.
        let body = "Some intro prose.\n\n```\nfirst second third fourth fifth sixth seventh eighth ninth tenth eleventh twelfth\n```";
        let out = markdown_in_gutter(body, 40);
        let bar = format!("{GUTTER}"); // the house bar's opening SGR

        for line in out.lines() {
            // Exactly one gutter bar per rendered line: the screenshot bug put two
            // on some lines (nested) and zero on the stranded continuations.
            assert_eq!(
                line.matches(&bar).count(),
                1,
                "every line carries exactly one gutter bar: {line:?}"
            );
        }

        // Every wrapped piece of the code block stays dim — the bug dropped the
        // dim on every line after the first.
        let dim = anstyle::Style::new().dimmed();
        let dim_open = format!("{dim}");
        for word in ["first", "seventh", "twelfth"] {
            let line = out
                .lines()
                .find(|l| l.contains(word))
                .unwrap_or_else(|| panic!("code word {word:?} present in {out:?}"));
            assert!(line.contains(&dim_open), "code line stays dim: {line:?}");
        }
    }

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
    fn branch_line_bolds_the_value() {
        use ansi_str::AnsiStr;
        let out = branch_line("feature/auth");
        // Bold (SGR 1) wraps the value — the app convention for branch
        // identifiers — closed by a full reset so it doesn't bleed past the
        // value (skim drops color_print's `</>`).
        assert!(out.contains("\x1b[1m"), "bold value: {out:?}");
        assert!(
            out.contains("\x1b[0m"),
            "full reset closes the span: {out:?}"
        );
        // The value still lands at the shared column behind the cyan label.
        let stripped = out.ansi_strip();
        assert_eq!(
            stripped.find("feature"),
            Some(VALUE_COLUMN),
            "value aligned behind the label: {stripped:?}"
        );
    }

    #[test]
    fn url_line_underlines_the_value() {
        let out = url_line("https://github.com/o/r/pull/7");
        // Underline (SGR 4) wraps the value — the app convention for inline
        // links — closed by a full reset.
        assert!(out.contains("\x1b[4m"), "underline value: {out:?}");
        assert!(
            out.contains("\x1b[0m"),
            "full reset closes the span: {out:?}"
        );
        assert!(
            out.contains("https://github.com/o/r/pull/7"),
            "url preserved intact: {out:?}"
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
    fn description_labels_the_block_like_the_metadata() {
        use ansi_str::AnsiStr;
        let out = description("Fixes the flaky retry logic.", 80);
        // A cyan all-caps `DESCRIPTION` label (SGR 36) heads the block, the same
        // styling the `branch`/`url` metadata labels use via `field_label`.
        assert!(out.contains("\x1b[36m"), "cyan label present: {out:?}");
        let stripped = out.ansi_strip();
        let lines: Vec<&str> = stripped.lines().collect();
        let label_idx = lines
            .iter()
            .position(|l| l.trim() == "DESCRIPTION")
            .expect("standalone `DESCRIPTION` label line");
        // A blank line separates the label from its body.
        assert!(
            lines[label_idx + 1].is_empty(),
            "blank line after the label: {stripped:?}"
        );
        assert!(
            lines[label_idx + 2].contains("Fixes the flaky retry logic."),
            "body follows the blank line: {stripped:?}"
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

    #[test]
    fn description_renders_code_blocks_flush() {
        use ansi_str::AnsiStr;
        // A fenced code block renders flush and dim — no house gutter bar, and
        // the code line sits at column 0, not indented into a gutter.
        let out = description("Run it:\n\n```\nwt switch\n```", 80);
        assert!(!out.contains("\x1b[107m"), "no gutter bar: {out:?}");
        let code = out
            .lines()
            .map(|l| l.ansi_strip().into_owned())
            .find(|l| l.contains("wt switch"))
            .expect("code line present");
        assert_eq!(code, "wt switch", "code flush at column 0: {code:?}");
    }
}
