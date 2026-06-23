//! Neutralizing terminal control sequences in untrusted free text.
//!
//! # Threat model
//!
//! The interactive picker (`wt switch`) renders text whose content is controlled
//! by something other than worktrunk's maintainers or the user's own keystrokes:
//!
//! - **Forge free text** — PR/MR titles, descriptions, author names, and URLs
//!   from `gh` / `glab` (and the other forges), shown in the picker's `pr`
//!   preview tab.
//! - **LLM output** — `[commit.generation]` model stdout, shown as branch
//!   summaries in the picker and the `wt list` Summary column.
//! - **Git object content** — commit messages and `git log --color` graphs from
//!   a branch that may have been fetched from a malicious remote, shown in the
//!   picker's log/diff preview tabs.
//!
//! A crafted string carrying raw terminal escapes (a `\x1b[2J` screen-clear, an
//! `\x1b[?1049h` alt-screen switch, cursor moves, an `\x1b]52` clipboard write,
//! an OSC-8 hyperlink the user never wrote, or a `\x07` BEL) flows verbatim to
//! the terminal unless something strips it. The impact is bounded to display
//! corruption — garble the pane, recolor, ring the bell, scramble skim's layout,
//! all recoverable on redraw — but it is the first surface that renders directly
//! attacker-influenced free text into the TUI, so it gets a deliberate boundary.
//!
//! # Why not sanitize inside the markdown renderer
//!
//! `md_help::render_markdown_in_help_with_width` is the shared markdown
//! renderer, and it would be the obvious chokepoint — but it is also fed
//! *trusted* ANSI: `wt --help` hands it clap's already-styled output, complete
//! with SGR colors and clap-generated OSC-8 hyperlinks to the docs. The renderer
//! cannot tell a trusted clap hyperlink from an attacker's body hyperlink by
//! type alone, and stripping all escapes there would strip clap's styling too.
//! So sanitization happens at the *trust boundary* instead: where untrusted text
//! enters worktrunk (ingestion) and where the picker hands a preview to skim.
//!
//! # The two policies
//!
//! - [`sanitize_untrusted_text`] strips **every** control/escape sequence. Use
//!   it at ingestion, on raw external strings (forge fields, LLM stdout) that are
//!   plain text and should carry no terminal styling of their own.
//! - [`sanitize_styled_output`] strips every sequence **except** the two
//!   line-bounded CSI forms our own rendering relies on: SGR color/style
//!   (`\x1b[…m`) and erase-in-line (`\x1b[…K`, emitted by pager front-ends like
//!   delta and bat to fill a line's background). Use it on already-styled output
//!   — the picker preview pane mixes worktrunk's own renderer styling, `git
//!   --color` output, and paged diffs with whatever untrusted text was rendered
//!   into it; those must survive so colored diffs and styled markdown still
//!   render, while every cursor move, screen clear, alt-screen switch, OSC, and
//!   stray control byte is removed.
//!
//! Both preserve `\n` (the line structure downstream wrapping relies on) and
//! `\t`; everything else in the C0/C1 control ranges, plus DEL, is dropped.
//!
//! # Bounded by design
//!
//! The kept sequences are all confined to the line they appear on, matching the
//! bounded display-corruption threat model: an attacker-supplied SGR can recolor
//! or conceal (`8m`) a span and an erase-in-line can blank one line, but neither
//! can move the cursor off the line, clear the screen, or persist past skim's
//! next redraw. Tightening this to an SGR allowlist (dropping conceal/blink) is
//! possible but risks dropping a legitimate `git --color` attribute, so the
//! full SGR set is permitted. Unicode spoofing (bidi overrides, zero-width
//! characters — "Trojan Source") is a separate, non-escape concern and is
//! deliberately out of scope here.

use std::borrow::Cow;

/// Strip every terminal control/escape sequence from untrusted plain text.
///
/// For raw external strings at their ingestion edge — forge PR/MR titles,
/// descriptions, author names, and LLM output — which are plain text and should
/// carry no terminal styling. `\n` and `\t` are preserved; all other C0/C1
/// controls, DEL, and every escape sequence (CSI, OSC, SGR, …) are removed.
pub fn sanitize_untrusted_text(s: &str) -> Cow<'_, str> {
    strip_sequences(s, false)
}

/// Strip every terminal control/escape sequence except SGR color/style.
///
/// For already-styled output handed to the terminal — the picker preview pane,
/// which interleaves worktrunk's renderer styling and `git --color` output (both
/// legitimate SGR) with untrusted text. SGR (`\x1b[…m`) survives so colors and
/// styles still render; cursor moves, screen/line erases, alt-screen switches,
/// scroll regions, OSC (titles, clipboard, hyperlinks), and stray control bytes
/// are removed.
pub fn sanitize_styled_output(s: &str) -> Cow<'_, str> {
    strip_sequences(s, true)
}

/// True for the only characters an untrusted string may contain untouched in the
/// fast path: anything that is not a control character, plus `\n` and `\t`.
fn is_plain(c: char) -> bool {
    !c.is_control() || c == '\n' || c == '\t'
}

type Scan<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Whether a CSI body (parameters + final byte, e.g. `31m`, `0K`, `?25h`) is one
/// of the few sequences [`sanitize_styled_output`] keeps:
///
/// - **SGR** (`…m`) — color and style, emitted by our renderer and `git --color`.
/// - **Erase-in-line** (`…K`) — emitted by pager front-ends (delta, bat) to fill
///   a line's background to the pane edge.
///
/// Both are bounded to the current line: they cannot move the cursor off the
/// line, erase the display, or switch screen buffers, so they fall inside the
/// bounded display-corruption threat model. Every other final (cursor motion,
/// erase-in-display `J`, mode set/reset `h`/`l`, scroll regions, …) is dropped.
/// DEC-private forms (`?…`, `>…`) carry a non-numeric parameter byte, so they are
/// never kept.
fn is_kept_csi(body: &str) -> bool {
    let Some(final_byte) = body.chars().last() else {
        return false;
    };
    if !matches!(final_byte, 'm' | 'K') {
        return false;
    }
    let params = &body[..body.len() - final_byte.len_utf8()];
    params
        .chars()
        .all(|c| c.is_ascii_digit() || c == ';' || c == ':')
}

/// Consume a CSI body after the introducer (7-bit `ESC [` or 8-bit `\u{9b}`):
/// parameter bytes (`0x30-0x3F`), intermediate bytes (`0x20-0x2F`), then one
/// final byte (`0x40-0x7E`). Returns the consumed body for SGR classification.
fn consume_csi_body(chars: &mut Scan<'_>) -> String {
    let mut body = String::new();
    while let Some(&p) = chars.peek() {
        if ('\u{20}'..='\u{3f}').contains(&p) {
            body.push(p);
            chars.next();
        } else {
            break;
        }
    }
    if let Some(&f) = chars.peek()
        && ('\u{40}'..='\u{7e}').contains(&f)
    {
        body.push(f);
        chars.next();
    }
    body
}

fn strip_sequences(s: &str, keep_sgr: bool) -> Cow<'_, str> {
    // Fast path: no control characters at all (escape sequences start with ESC
    // or an 8-bit C1 byte, all of which are control chars), so nothing to strip.
    if s.chars().all(is_plain) {
        return Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // 7-bit escapes — ESC introduces; the next char selects the kind.
            '\x1b' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    let body = consume_csi_body(&mut chars);
                    // Keep only the line-bounded 7-bit CSI our own renderer,
                    // `git --color`, and pagers emit (SGR color/style and
                    // erase-in-line); drop every other CSI.
                    if keep_sgr && is_kept_csi(&body) {
                        out.push('\x1b');
                        out.push('[');
                        out.push_str(&body);
                    }
                }
                // OSC / DCS / SOS / PM / APC: string sequences (BEL- or
                // ST-terminated). Always dropped — the preview has no legitimate
                // OSC, and a body-supplied OSC-8 hyperlink or OSC-52 clipboard
                // write is exactly the threat.
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    chars.next();
                    consume_string_terminator(&mut chars);
                }
                // Charset designators (`ESC ( B`), RIS (`ESC c`), keypad modes
                // (`ESC =`), etc.: intermediates then one final byte. Dropped.
                Some(_) => {
                    while let Some(&p) = chars.peek() {
                        if ('\u{20}'..='\u{2f}').contains(&p) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    chars.next();
                }
                None => {} // lone trailing ESC
            },
            // 8-bit C1 introducers (the UTF-8-decoded single-char forms a
            // terminal in 8-bit mode honors). We never emit 8-bit SGR ourselves,
            // so even a C1 CSI is dropped wholesale rather than kept.
            '\u{9b}' => {
                let _ = consume_csi_body(&mut chars); // CSI
            }
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                consume_string_terminator(&mut chars); // DCS / SOS / OSC / PM / APC
            }
            // Keep the line structure and tabs; drop every other control
            // character (the rest of C0, DEL, and the remaining C1 range).
            '\n' | '\t' => out.push(c),
            _ if c.is_control() => {}
            _ => out.push(c),
        }
    }

    Cow::Owned(out)
}

/// Consume a string-sequence body up to and including its terminator, or to end
/// of input. Terminators: BEL, 7-bit ST (`ESC \`), or 8-bit ST (`\u{9c}`). The
/// leading introducer (`]`, `P`, the C1 byte, …) has already been consumed.
fn consume_string_terminator(chars: &mut Scan<'_>) {
    while let Some(c) = chars.next() {
        match c {
            '\x07' | '\u{9c}' => break,
            '\x1b' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                break;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_borrowed_unchanged() {
        // No control chars — returned as-is without allocating.
        let s = "Fix the **flaky** `retry` logic (see #123)";
        assert!(matches!(sanitize_untrusted_text(s), Cow::Borrowed(_)));
        assert_eq!(sanitize_untrusted_text(s), s);
        assert!(matches!(sanitize_styled_output(s), Cow::Borrowed(_)));
        assert_eq!(sanitize_styled_output(s), s);
    }

    #[test]
    fn newlines_and_tabs_survive() {
        let s = "line one\nline\ttwo\nline three";
        assert_eq!(sanitize_untrusted_text(s), s);
        assert_eq!(sanitize_styled_output(s), s);
    }

    #[test]
    fn strips_csi_screen_and_cursor_sequences() {
        // Screen clear, alt-screen, cursor move, scroll region — all dropped,
        // leaving only the surrounding text.
        let s = "a\x1b[2Jb\x1b[?1049hc\x1b[5Ad\x1b[1;40re";
        assert_eq!(sanitize_untrusted_text(s), "abcde");
        assert_eq!(sanitize_styled_output(s), "abcde");
    }

    #[test]
    fn strips_c0_controls_and_del_and_c1() {
        // BEL, backspace, vertical tab, form feed, CR, DEL, and a non-introducer
        // C1 byte (U+0080). (C1 sequence introducers like U+009B are covered in
        // strips_8bit_c1_introducers_and_their_bodies.)
        let s = "a\x07b\x08c\x0bd\x0ce\rf\x7fg\u{80}h";
        assert_eq!(sanitize_untrusted_text(s), "abcdefgh");
        assert_eq!(sanitize_styled_output(s), "abcdefgh");
    }

    #[test]
    fn strips_osc_including_hyperlinks_and_clipboard() {
        // OSC-8 hyperlink (BEL-terminated), OSC-0 window title (ST-terminated),
        // OSC-52 clipboard write — every OSC is dropped under both policies.
        let link = "\x1b]8;;https://evil.example\x07click\x1b]8;;\x07";
        assert_eq!(sanitize_untrusted_text(link), "click");
        assert_eq!(sanitize_styled_output(link), "click");

        let title = "before\x1b]0;pwned\x1b\\after";
        assert_eq!(sanitize_untrusted_text(title), "beforeafter");
        assert_eq!(sanitize_styled_output(title), "beforeafter");

        let clip = "x\x1b]52;c;cGF3bz==\x07y";
        assert_eq!(sanitize_styled_output(clip), "xy");
    }

    #[test]
    fn untrusted_policy_strips_sgr_too() {
        // Raw external text should carry no styling of its own.
        let s = "\x1b[31mred\x1b[0m and \x1b[1mbold\x1b[0m";
        assert_eq!(sanitize_untrusted_text(s), "red and bold");
    }

    #[test]
    fn styled_policy_keeps_sgr() {
        // Already-styled output keeps its colors so diffs and markdown render.
        let s = "\x1b[31mred\x1b[0m and \x1b[1mbold\x1b[0m";
        assert_eq!(sanitize_styled_output(s), s);
        // Multi-parameter SGR (truecolor) is preserved.
        let tc = "\x1b[38;2;255;0;0mhi\x1b[0m";
        assert_eq!(sanitize_styled_output(tc), tc);
        // An empty-parameter SGR (`\x1b[m`, reset) is valid and kept.
        assert_eq!(sanitize_styled_output("\x1b[mx"), "\x1b[mx");
    }

    #[test]
    fn styled_policy_drops_dangerous_csi_but_keeps_adjacent_sgr() {
        // A DEC-private show-cursor (`\x1b[?25h`), a screen clear (`\x1b[2J`), and
        // a cursor move (`\x1b[5A`) are dropped, even though they share the CSI
        // shape with colors; the adjacent SGR survives.
        let s = "\x1b[32mgreen\x1b[0m\x1b[?25h\x1b[2J\x1b[5Ax";
        assert_eq!(sanitize_styled_output(s), "\x1b[32mgreen\x1b[0mx");
        // A `>`-prefixed parameter is not a color SGR even with an `m` final.
        assert_eq!(sanitize_styled_output("\x1b[>1mx"), "x");
    }

    #[test]
    fn styled_policy_keeps_erase_in_line() {
        // Pager front-ends (delta, bat) emit erase-in-line (`\x1b[0K`/`\x1b[K`)
        // to fill a diff line's background to the pane edge. It is line-bounded,
        // so it is kept — stripping it gave configured-pager diffs a ragged edge.
        for el in ["\x1b[K", "\x1b[0K", "\x1b[2K"] {
            let s = format!("\x1b[41mline{el}");
            assert_eq!(sanitize_styled_output(&s), s, "EL {el:?} should survive");
        }
        // Erase-in-display (`\x1b[2J`) is NOT line-bounded and is still dropped.
        assert_eq!(sanitize_styled_output("a\x1b[2Jb"), "ab");
        // The strict (ingestion) policy drops erase-in-line too.
        assert_eq!(sanitize_untrusted_text("a\x1b[Kb"), "ab");
    }

    #[test]
    fn handles_truncated_sequences_at_end_of_input() {
        // Malformed/truncated escapes at the end are consumed, not echoed.
        assert_eq!(sanitize_untrusted_text("text\x1b"), "text");
        assert_eq!(sanitize_untrusted_text("text\x1b["), "text");
        assert_eq!(sanitize_untrusted_text("text\x1b[31"), "text");
        assert_eq!(sanitize_untrusted_text("text\x1b]8;;url"), "text");
    }

    #[test]
    fn strips_8bit_c1_introducers_and_their_bodies() {
        // A terminal in 8-bit mode honors C1 controls: U+009B is CSI, U+009D is
        // OSC, U+0090 is DCS. The whole sequence (introducer + body) is removed,
        // not just the introducer byte — so no inert `2J`/`8;;url` litter remains.
        let csi = "a\u{9b}2Jb\u{9b}?1049hc";
        assert_eq!(sanitize_untrusted_text(csi), "abc");
        assert_eq!(sanitize_styled_output(csi), "abc");

        // 8-bit OSC terminated by 8-bit ST (U+009C).
        let osc = "x\u{9d}8;;https://evil\u{9c}y";
        assert_eq!(sanitize_styled_output(osc), "xy");

        // An 8-bit SGR-shaped CSI is still dropped — we only keep the 7-bit form.
        let c1_sgr = "\u{9b}31mred";
        assert_eq!(sanitize_styled_output(c1_sgr), "red");
    }

    #[test]
    fn strips_charset_designators_and_ris() {
        // ESC ( B (select ASCII charset), ESC c (full reset) — dropped.
        assert_eq!(sanitize_untrusted_text("a\x1b(Bb\x1bcc"), "abc");
        assert_eq!(sanitize_styled_output("a\x1b(Bb\x1bcc"), "abc");
    }
}
