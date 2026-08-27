//! README and config synchronization tests
//!
//! Verifies that README.md examples stay in sync with their source snapshots and help output.
//! Also syncs default templates from src/llm.rs to dev/config.example.toml
//! and generates dev/wt.example.toml from the project config section.
//! Automatically updates sections when out of sync.
//!
//! Run with: `cargo test --test integration readme_sync`
//!
//! Skipped on Windows: These tests verify documentation sync using help output which has
//! platform-specific formatting differences (clap markdown rendering, line endings).
//!
//! ## Architecture
//!
//! The sync system uses a unified pipeline:
//!
//! 1. **Parsing**: `parse_snapshot_raw()` extracts content from snapshot files
//! 2. **Placeholders**: `replace_placeholders()` normalizes test paths to display paths
//! 3. **Formatting**: `OutputFormat` enum controls the final output (plain text vs HTML)
//! 4. **Updating**: `update_section()` finds markers and replaces content
#![cfg(not(windows))]

use crate::common::wt_command;
use ansi_str::AnsiStr;
use ansi_to_tui::IntoText;
use ratatui::style::{Color, Modifier, Style};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use worktrunk::docs::{MARKER_CLOSE, MARKER_OPEN_PREFIX};

/// Wrap a body in `<!-- ⚠️ AUTO-GENERATED ... -->` markers.
/// Inner whitespace ("\n\n{body}\n\n") matches the historical layout that
/// downstream regexes and visual review depend on.
fn wrap_in_marker(id: &str, source_label: &str, body: &str) -> String {
    format!(
        "{MARKER_OPEN_PREFIX}{id} — edit {source_label} to update -->\n\n{body}\n\n{MARKER_CLOSE}"
    )
}

/// Unified pattern for all AUTO-GENERATED markers.
/// Format: `<!-- ⚠️ AUTO-GENERATED from <id> — edit <source> to update -->`
/// ID types: path.snap (snapshot), `cmd` (help), path#anchor (section).
/// Content may be wrapped in ```console``` (snapshots) or unwrapped (help/sections).
static MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?s){}([^\n]+?) — edit [^\n]+ to update -->\n+([\s\S]*?)\n*{}",
        regex::escape(MARKER_OPEN_PREFIX),
        regex::escape(MARKER_CLOSE),
    ))
    .unwrap()
});

/// Regex for literal bracket notation (as stored in snapshots) - used by literal_to_escape
static ANSI_LITERAL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[0-9;]*m").unwrap());

/// Regex to find snapshot-driven markers in standalone docs files
/// (worktrunk.md, llm-commits.md, etc.) for in-place refresh. Matching the
/// marker envelope instead of its body keeps snapshot sync independent of the
/// documentation renderer.
static DOCS_SNAPSHOT_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?s){}([^\s]+\.snap) — edit source to update -->\n+(.*?)\n+{}",
        regex::escape(MARKER_OPEN_PREFIX),
        regex::escape(MARKER_CLOSE),
    ))
    .unwrap()
});

/// Regex for HASH placeholder (used by shell_wrapper tests)
static HASH_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[HASH\]").unwrap());

/// Regex for TMPDIR paths with branch suffix (e.g., [TMPDIR]/repo.fix-auth)
static TMPDIR_BRANCH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[TMPDIR\]/repo\.([^\s/]+)").unwrap());

/// Regex for TMPDIR paths without branch suffix (e.g., [TMPDIR]/repo at end or followed by space/newline)
/// Matches [TMPDIR]/repo when followed by end-of-string, whitespace, or non-word character (but not dot)
static TMPDIR_MAIN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[TMPDIR\]/repo(\s|$)").unwrap());

/// Regex for REPO placeholder
static REPO_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[REPO\]").unwrap());

/// Regex for _REPO_ placeholder (used in insta-cmd snapshots)
/// Matches _REPO_ followed by optional .branch suffix
static REPO_UNDERSCORE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"_REPO_(\.([a-zA-Z0-9_-]+))?").unwrap());

/// Regex to extract user config section from src/cli/mod.rs
/// Matches content between USER_CONFIG_START and USER_CONFIG_END markers
static USER_CONFIG_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- USER_CONFIG_START -->\n(.*?)\n<!-- USER_CONFIG_END -->").unwrap()
});

/// Regex to extract project config section from src/cli/mod.rs
/// Matches content between PROJECT_CONFIG_START and PROJECT_CONFIG_END markers
static PROJECT_CONFIG_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- PROJECT_CONFIG_START -->\n(.*?)\n<!-- PROJECT_CONFIG_END -->").unwrap()
});

/// Regex to find DEFAULT_TEMPLATE marker in user config section (markdown format)
static DEFAULT_TEMPLATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(<!-- DEFAULT_TEMPLATE_START -->\n).*?(<!-- DEFAULT_TEMPLATE_END -->)")
        .unwrap()
});

/// Regex to find DEFAULT_SQUASH_TEMPLATE marker in user config section (markdown format)
static SQUASH_TEMPLATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)(<!-- DEFAULT_SQUASH_TEMPLATE_START -->\n).*?(<!-- DEFAULT_SQUASH_TEMPLATE_END -->)",
    )
    .unwrap()
});

/// Regex to extract Rust raw string constants (single pound)
static RUST_RAW_STRING_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##"(?s)const (DEFAULT_TEMPLATE|DEFAULT_SQUASH_TEMPLATE): &str = r#"(.*?)"#;"##)
        .unwrap()
});

/// Regex to convert site-root documentation links to full URLs.
/// Matches: [text](/page/) or [text](/page/#anchor).
///
/// Link text tolerates `]` characters when they appear inside a backticked
/// code span (e.g. `[[block]]`), alternating "a `...` code span" with "any
/// non-`]`-non-backtick char". Bare backticks are forbidden so the regex
/// can't bridge across two unrelated code spans on the same line.
static SITE_LINK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[((?:`[^`]*`|[^\]`])+)\]\(/([^)/]+)/(#[^)]*)?\)").unwrap());

/// Guardrail for root-relative or legacy Zola links on generated non-site surfaces.
static UNTRANSFORMED_SITE_LINK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\((?:/|@/)[^)]+\)").unwrap());

/// Guardrail for every generated non-site surface that rewrites internal links.
///
/// Root-relative links work on the site but not in installed skill files or
/// generated config examples. Both rewrites are regexes over link text, so an
/// unsupported shape can otherwise survive silently.
///
/// So each surface asserts the negative afterwards: no `](/…)` or legacy
/// `](@/…)` may remain.
/// `surface` names the generated content for the failure message.
fn assert_no_untransformed_site_links(content: &str, surface: &str) {
    if let Some(m) = UNTRANSFORMED_SITE_LINK_PATTERN.find(content) {
        let snippet_start = content[..m.start()].rfind('\n').map_or(0, |i| i + 1);
        let snippet_end = content[m.end()..]
            .find('\n')
            .map_or(content.len(), |i| m.end() + i);
        panic!(
            "Failed to transform an internal site link in {surface} — likely \
             legacy syntax or an unsupported character in the link text. Offending line:\n{}",
            &content[snippet_start..snippet_end]
        );
    }
}

/// Regex to convert figure/picture elements to simple markdown images
/// Matches: <figure class="demo">...<img src="/assets/X.gif" alt="Y"...>...</figure>
/// Extracts: src path and alt text from the <img> tag
/// Note: Maps /assets/X to assets/X in the worktrunk-assets repo
static HTML_FIGURE_TO_IMAGE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<figure class="demo">\s*<picture>.*?<img src="/assets/([^"]+)" alt="([^"]*)"[^>]*>.*?</picture>.*?</figure>"#,
    )
    .unwrap()
});

// =============================================================================
// Unified Template Infrastructure
// =============================================================================

/// Output format for section updates
enum OutputFormat {
    /// Docs: framework-neutral console fence.
    DocsMarkdown,
    /// Unwrapped: raw markdown content (help commands, doc sections)
    Unwrapped,
}

/// Marker ID type, detected from the ID string
#[derive(Clone, Copy)]
enum MarkerType {
    /// Snapshot (.snap extension) - content wrapped in ```console```
    Snapshot,
    /// Help command (backticks) - unwrapped content
    Help,
    /// Doc section (#anchor) - unwrapped content
    Section,
}

impl MarkerType {
    /// Detect marker type from ID string
    fn from_id(id: &str) -> Self {
        if id.starts_with('`') && id.ends_with('`') {
            Self::Help
        } else if id.contains('#') {
            Self::Section
        } else {
            Self::Snapshot
        }
    }
}

/// Parse a snapshot file, returning the user-facing output content
///
/// Handles:
/// - YAML front matter removal
/// - insta_cmd stdout/stderr section extraction (both streams, in terminal order)
/// - Malformed snapshots (returns raw content rather than erroring)
fn parse_snapshot_raw(content: &str) -> String {
    // Remove YAML front matter
    let content = if content.starts_with("---") {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() >= 3 {
            parts[2].trim().to_string()
        } else {
            content.to_string()
        }
    } else {
        content.to_string()
    };

    // Handle insta_cmd format with stdout/stderr sections. Show both streams
    // in terminal order (stdout, then stderr) — a command like `wt list` puts
    // the table on stdout and the summary/warnings on stderr, and the docs
    // block should read like the terminal.
    if content.contains("----- stdout -----") {
        let stdout = extract_section(&content, "----- stdout -----\n", "----- stderr -----");
        let stderr = extract_section(&content, "----- stderr -----\n", "----- ");
        return match (stdout.is_empty(), stderr.is_empty()) {
            (false, false) => format!("{stdout}\n{stderr}"),
            (true, _) => stderr, // both-empty also lands here, returning ""
            (false, true) => stdout,
        };
    }

    // Plain content (PTY-based tests without section markers)
    content
}

/// Extract a section between start marker and end marker
///
/// Returns empty string if start marker not found.
/// If end marker missing, returns content from start marker to EOF.
fn extract_section(content: &str, start_marker: &str, end_marker: &str) -> String {
    if let Some(start) = content.find(start_marker) {
        let after_header = &content[start + start_marker.len()..];
        if let Some(end) = after_header.find(end_marker) {
            after_header[..end].trim_end().to_string()
        } else {
            after_header.trim_end().to_string()
        }
    } else {
        String::new()
    }
}

/// Extract command line from snapshot YAML header
///
/// Parses the YAML front matter to extract program and args, returning the command line.
/// Returns None if the snapshot doesn't have command info (e.g., non-insta_cmd snapshots).
fn extract_command_from_snapshot(content: &str) -> Option<String> {
    // Extract YAML front matter
    if !content.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    let yaml = parts[1];

    // Extract program (line: "  program: wt")
    let program = yaml
        .lines()
        .find(|l| l.trim().starts_with("program:"))
        .map(|l| l.trim().strip_prefix("program:").unwrap().trim())?;

    // Extract args (lines: "  args:\n    - switch\n    - --create\n    - feature")
    let args_start = yaml.find("args:")?;
    let args_section = &yaml[args_start..];
    let args: Vec<&str> = args_section
        .lines()
        .skip(1) // Skip "args:" line
        .take_while(|l| l.trim().starts_with("- "))
        .map(|l| l.trim().strip_prefix("- ").unwrap().trim_matches('"'))
        .collect();

    if args.is_empty() {
        Some(program.to_string())
    } else {
        Some(format!("{} {}", program, args.join(" ")))
    }
}

/// Replace test placeholders with display-friendly values
///
/// Transforms:
/// - `[HASH]` → `a1b2c3d`
/// - `[TMPDIR]/repo.branch` → `../repo.branch`
/// - `[TMPDIR]/repo` → `../repo`
/// - `[REPO]` → `../repo`
/// - `_REPO_` → `~/repo` (worktree path; tilde so it reads as a path, not a project name)
/// - `_REPO_.branch` → `~/repo.branch`
fn replace_placeholders(content: &str) -> String {
    let content = HASH_REGEX.replace_all(content, "a1b2c3d");
    let content = TMPDIR_BRANCH_REGEX.replace_all(&content, "../repo.$1");
    let content = TMPDIR_MAIN_REGEX.replace_all(&content, "../repo$1");
    let content = REPO_REGEX.replace_all(&content, "../repo");
    // Handle _REPO_.branch -> ~/repo.branch and _REPO_ -> ~/repo
    REPO_UNDERSCORE_REGEX
        .replace_all(&content, |caps: &regex::Captures| {
            if let Some(branch) = caps.get(2) {
                format!("~/repo.{}", branch.as_str())
            } else {
                "~/repo".to_string()
            }
        })
        .into_owned()
}

/// Format replacement content based on output format. The `wrap_in_marker`
/// envelope is identical for both; only the body construction varies.
fn format_body(content: &str, format: &OutputFormat) -> String {
    match format {
        OutputFormat::DocsMarkdown => format!("```console\n{content}\n```"),
        OutputFormat::Unwrapped => content.to_string(),
    }
}

fn format_replacement(id: &str, content: &str, format: &OutputFormat) -> String {
    wrap_in_marker(id, "source", &format_body(content, format))
}

/// Update sections matching a pattern in content
///
/// Unified function for all section types. The `get_replacement` closure
/// receives (id, current_content) and returns the new content.
fn update_section(
    content: &str,
    pattern: &Regex,
    format: OutputFormat,
    get_replacement: impl Fn(&str, &str) -> Result<String, String>,
) -> Result<(String, usize, usize), Vec<String>> {
    let mut result = content.to_string();
    let mut errors = Vec::new();
    let mut updated = 0;

    // Collect all matches first (to avoid borrowing issues)
    let matches: Vec<_> = pattern
        .captures_iter(content)
        .map(|cap| {
            let full_match = cap.get(0).unwrap();
            let id = cap.get(1).unwrap().as_str().to_string();
            let current = cap.get(2).unwrap().as_str().to_string();
            (full_match.start(), full_match.end(), id, current)
        })
        .collect();

    let total = matches.len();

    // Process in reverse order to preserve positions
    for (start, end, id, current) in matches.into_iter().rev() {
        let expected = match get_replacement(&id, &current) {
            Ok(content) => content,
            Err(e) => {
                errors.push(format!("❌ {}: {}", id, e));
                continue;
            }
        };

        if current != format_body(&expected, &format) {
            let replacement = format_replacement(&id, &expected, &format);
            result.replace_range(start..end, &replacement);
            updated += 1;
        }
    }

    if errors.is_empty() {
        Ok((result, updated, total))
    } else {
        Err(errors)
    }
}

#[test]
fn test_markdown_snapshot_section_converges() {
    let content = "<!-- ⚠️ AUTO-GENERATED from tests/example.snap — edit source to update -->\n\n```console\n$ wt list\noutput\n```\n\n<!-- END AUTO-GENERATED -->";
    let (unchanged, updated, total) = update_section(
        content,
        &DOCS_SNAPSHOT_MARKER_PATTERN,
        OutputFormat::DocsMarkdown,
        |_, _| Ok("$ wt list\noutput".to_string()),
    )
    .unwrap();
    assert_eq!(unchanged, content);
    assert_eq!((updated, total), (0, 1));

    let (changed, updated, _) = update_section(
        content,
        &DOCS_SNAPSHOT_MARKER_PATTERN,
        OutputFormat::DocsMarkdown,
        |_, _| Ok("$ wt list\nnew output".to_string()),
    )
    .unwrap();
    assert_eq!(updated, 1);
    assert!(changed.contains("```console\n$ wt list\nnew output\n```"));

    let content_with_trailing_space = content.replace("output\n```", "output \n```");
    let (cleaned, updated, _) = update_section(
        &content_with_trailing_space,
        &DOCS_SNAPSHOT_MARKER_PATTERN,
        OutputFormat::DocsMarkdown,
        |_, _| Ok("$ wt list\noutput".to_string()),
    )
    .unwrap();
    assert_eq!(updated, 1);
    assert_eq!(cleaned, content);
}

#[test]
fn test_snapshot_plain_text_has_no_trailing_whitespace() {
    let rendered = trim_lines(&parse_snapshot_content("before\n\x1b[107m \x1b[0m \nafter"));
    assert_eq!(rendered, "before\n\nafter");

    let snapshot =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "tests/snapshots/integration__integration_tests__merge__docs_merge_squash_llm.snap",
        ))
        .unwrap();
    let rendered = trim_lines(&parse_snapshot_content(&snapshot));
    assert!(
        rendered.lines().all(|line| line.trim_end() == line),
        "snapshot output retained trailing whitespace"
    );
}

// =============================================================================
// End Unified Infrastructure
// =============================================================================

/// Regex to find command placeholder comments in help pages.
///
/// A placeholder is an HTML comment `<!-- wt <id> -->` followed by a `bash` or
/// `console` fence with an optional `$ ` prompt and optional captured output.
///
/// Capture groups:
/// 1. placeholder id (e.g. `wt list (markers)`) — drives snapshot lookup
/// 2. display command
static COMMAND_PLACEHOLDER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- (wt [^>\n]+) -->\n```(?:bash|console)\n(?:\$ )?(wt [^\n]+).*?\n```")
        .unwrap()
});

/// Snapshot-backed command examples shared by Markdown generation and the
/// website's ANSI-style manifest.
const COMMAND_SNAPSHOTS: &[(&str, &str)] = &[
    (
        "wt list",
        "integration__integration_tests__list__readme_example_list.snap",
    ),
    (
        "wt list --full",
        "integration__integration_tests__list__readme_example_list_full.snap",
    ),
    (
        "wt list --branches --full",
        "integration__integration_tests__list__readme_example_list_branches.snap",
    ),
    (
        "wt list (markers)",
        "integration__integration_tests__list__readme_example_list_marker.snap",
    ),
    // Docs-page example snapshots drive the static command-output blocks on
    // pages otherwise dominated by GIFs. See the convention in merge.rs.
    (
        "wt merge (docs-example)",
        "integration__integration_tests__merge__docs_merge_pre_merge_hook.snap",
    ),
    (
        "wt step commit (docs-example)",
        "integration__integration_tests__merge__docs_step_commit_llm.snap",
    ),
    (
        "wt remove (docs-example)",
        "integration__integration_tests__remove__docs_remove_pre_remove_hook.snap",
    ),
    (
        "wt hook pre-merge (docs-example)",
        "integration__integration_tests__user_hooks__docs_hook_pre_merge.snap",
    ),
];

/// Map commands to their snapshot files for help page expansion.
fn command_to_snapshot(command: &str) -> Option<&'static str> {
    COMMAND_SNAPSHOTS
        .iter()
        .find_map(|(candidate, snapshot)| (*candidate == command).then_some(*snapshot))
}

/// Expand command placeholders in help page content into rendered snapshot blocks.
///
/// Finds `<!-- wt <id> -->` + ```bash\n[$ ]wt <cmd>\n``` blocks, looks up the
/// snapshot for the placeholder id (e.g. `wt list (markers)`), and replaces
/// the block with a portable console rendering.
///
/// The placeholder id drives snapshot lookup so disambiguation suffixes like
/// `(markers)` don't have to appear in the displayed command. Commands without
/// a snapshot mapping are left unchanged.
fn expand_command_placeholders(content: &str, snapshots_dir: &Path) -> Result<String, String> {
    let mut result = content.to_string();
    let mut errors = Vec::new();

    for cap in COMMAND_PLACEHOLDER_PATTERN.captures_iter(content) {
        let full_match = cap.get(0).unwrap().as_str();
        let placeholder_id = cap.get(1).unwrap().as_str();
        let display_cmd = cap.get(2).unwrap().as_str();

        let Some(snapshot_name) = command_to_snapshot(placeholder_id) else {
            continue;
        };

        let snapshot_path = snapshots_dir.join(snapshot_name);
        if !snapshot_path.exists() {
            errors.push(format!(
                "Snapshot file not found: {} (for command '{}')",
                snapshot_path.display(),
                placeholder_id
            ));
            continue;
        }

        let snapshot_content = fs::read_to_string(&snapshot_path)
            .map_err(|e| format!("Failed to read {}: {}", snapshot_path.display(), e))?;

        let plain = trim_lines(&parse_snapshot_content(&snapshot_content));
        let body = if plain.is_empty() {
            format!("$ {display_cmd}")
        } else {
            format!("$ {display_cmd}\n{plain}")
        };
        let replacement = format!("```console\n{body}\n```");

        result = result.replace(full_match, &replacement);
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    Ok(result)
}

/// Convert literal bracket notation [32m to actual escape sequences \x1b[32m
fn literal_to_escape(text: &str) -> String {
    ANSI_LITERAL_REGEX
        .replace_all(text, |caps: &regex::Captures| {
            let code = caps.get(0).unwrap().as_str();
            format!("\x1b{code}")
        })
        .to_string()
}

/// Trim trailing whitespace from each line and overall.
/// Preserves leading spaces (e.g., two-space gutter before table headers in `wt list`).
fn trim_lines(content: &str) -> String {
    content
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Parse snapshot content into portable Markdown code-block text.
fn parse_snapshot_content(content: &str) -> String {
    let content = parse_snapshot_raw(content);
    let content = replace_placeholders(&content);
    let content = literal_to_escape(&content);
    content.ansi_strip().into_owned()
}

/// CSS class for the semantic ANSI foreground carried by a snapshot span.
fn terminal_color_class(color: Color) -> Result<Option<&'static str>, String> {
    match color {
        Color::Red | Color::LightRed => Ok(Some("wt-terminal-red")),
        Color::Green | Color::LightGreen => Ok(Some("wt-terminal-green")),
        Color::Yellow | Color::LightYellow => Ok(Some("wt-terminal-yellow")),
        Color::Blue | Color::LightBlue => Ok(Some("wt-terminal-blue")),
        Color::Magenta | Color::LightMagenta => Ok(Some("wt-terminal-magenta")),
        Color::Cyan | Color::LightCyan => Ok(Some("wt-terminal-cyan")),
        Color::DarkGray | Color::Gray => Ok(Some("wt-terminal-gray")),
        Color::Reset => Ok(None),
        Color::Black | Color::White | Color::Rgb(..) | Color::Indexed(_) => {
            Err(format!("unsupported ANSI foreground color {color:?}"))
        }
    }
}

/// CSS class for the bright execution gutter rendered by Worktrunk output.
fn terminal_background_class(color: Color) -> Result<Option<&'static str>, String> {
    match color {
        Color::Gray | Color::White => Ok(Some("wt-terminal-gutter")),
        Color::Reset => Ok(None),
        Color::Black
        | Color::Red
        | Color::Green
        | Color::Yellow
        | Color::Blue
        | Color::Magenta
        | Color::Cyan
        | Color::DarkGray
        | Color::LightRed
        | Color::LightGreen
        | Color::LightYellow
        | Color::LightBlue
        | Color::LightMagenta
        | Color::LightCyan
        | Color::Rgb(..)
        | Color::Indexed(_) => Err(format!("unsupported ANSI background color {color:?}")),
    }
}

fn terminal_style_classes(style: Style) -> Result<Vec<String>, String> {
    let mut classes = Vec::new();
    if let Some(color) = style.fg
        && let Some(class) = terminal_color_class(color)?
    {
        classes.push(class.to_string());
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        classes.push("wt-terminal-bold".to_string());
    }
    if style.add_modifier.contains(Modifier::DIM) {
        classes.push("wt-terminal-dim".to_string());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        classes.push("wt-terminal-italic".to_string());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        classes.push("wt-terminal-underline".to_string());
    }
    if let Some(color) = style.bg
        && let Some(class) = terminal_background_class(color)?
    {
        classes.push(class.to_string());
    }
    Ok(classes)
}

/// Parse one snapshot's ANSI output into the span data the website renderer
/// consumes. The reconstructed visible text must equal the portable Markdown
/// generated from the same snapshot, which prevents style and content from
/// drifting into two parallel examples.
fn styled_snapshot_lines(content: &str) -> Result<(String, serde_json::Value), String> {
    let raw = parse_snapshot_raw(content);
    let raw = replace_placeholders(&raw);
    let ansi = literal_to_escape(&raw);
    let parsed = ansi
        .as_str()
        .into_text()
        .map_err(|error| format!("ANSI parsing failed: {error}"))?;
    let plain = trim_lines(&parse_snapshot_content(content));
    let expected_lines: Vec<&str> = if plain.is_empty() {
        Vec::new()
    } else {
        plain.split('\n').collect()
    };

    if parsed.lines.len() < expected_lines.len() {
        return Err(format!(
            "ANSI parser returned {} line(s), expected {}",
            parsed.lines.len(),
            expected_lines.len()
        ));
    }

    let mut rendered_lines = Vec::with_capacity(expected_lines.len());
    for (line_index, expected) in expected_lines.into_iter().enumerate() {
        let line = &parsed.lines[line_index];
        let mut segments: Vec<(String, Vec<String>)> = Vec::new();
        for span in &line.spans {
            if span.content.is_empty() {
                continue;
            }
            let classes = terminal_style_classes(parsed.style.patch(line.style).patch(span.style))
                .map_err(|error| format!("styled line {}: {error}", line_index + 1))?;
            if let Some((text, previous_classes)) = segments.last_mut()
                && *previous_classes == classes
            {
                text.push_str(&span.content);
            } else {
                segments.push((span.content.to_string(), classes));
            }
        }

        while let Some((text, _)) = segments.last_mut() {
            let trimmed = text.trim_end().to_string();
            if trimmed.is_empty() {
                segments.pop();
            } else {
                *text = trimmed;
                break;
            }
        }

        let actual = segments
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<String>();
        if actual != expected {
            return Err(format!(
                "styled line {} differs from portable text\nexpected: {expected:?}\nactual:   {actual:?}",
                line_index + 1
            ));
        }

        rendered_lines.push(serde_json::Value::Array(
            segments
                .into_iter()
                .map(|(text, classes)| serde_json::json!({ "text": text, "classes": classes }))
                .collect(),
        ));
    }

    Ok((plain, serde_json::Value::Array(rendered_lines)))
}

#[test]
fn test_styled_snapshot_lines_preserve_text_and_ansi_roles() {
    let (plain, lines) = styled_snapshot_lines(
        "\x1b[107m \x1b[0m \x1b[1mBranch\x1b[0m  \x1b[32m+2\x1b[0m  \x1b[2m\x1b[34m#42\x1b[0m",
    )
    .unwrap();
    assert_eq!(plain, "  Branch  +2  #42");
    assert_eq!(
        lines,
        serde_json::json!([[
            { "text": " ", "classes": ["wt-terminal-gutter"] },
            { "text": " ", "classes": [] },
            { "text": "Branch", "classes": ["wt-terminal-bold"] },
            { "text": "  ", "classes": [] },
            { "text": "+2", "classes": ["wt-terminal-green"] },
            { "text": "  ", "classes": [] },
            { "text": "#42", "classes": ["wt-terminal-blue", "wt-terminal-dim"] }
        ]])
    );

    let error = styled_snapshot_lines("\x1b[38;2;1;2;3mcustom\x1b[0m").unwrap_err();
    assert!(error.contains("unsupported ANSI foreground color Rgb(1, 2, 3)"));
}

/// Get help output for a command
///
/// Expected format: `wt <subcommand> --help-md` (ID includes backticks from marker)
fn help_output(id: &str, project_root: &Path) -> Result<String, String> {
    // Strip backticks from ID (captured by MARKER_PATTERN)
    let command = id.trim_matches('`');
    let args: Vec<&str> = command.split_whitespace().collect();
    if args.is_empty() {
        return Err("Empty command".to_string());
    }

    // Validate command format
    if args.first() != Some(&"wt") {
        return Err(format!("Command must start with 'wt': {}", command));
    }

    // Validate it ends with --help-md
    if args.last() != Some(&"--help-md") {
        return Err(format!("Command must end with '--help-md': {}", command));
    }

    // Use the already-built binary from cargo test (wt_command provides isolation)
    let output = wt_command()
        .env("NO_COLOR", "1") // Plain text for README
        .args(&args[1..]) // Skip "wt" prefix
        .current_dir(project_root)
        .output()
        .map_err(|e| format!("Failed to run command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Help goes to stdout
    let help_output = if !stdout.is_empty() {
        stdout.to_string()
    } else {
        stderr.to_string()
    };

    // Trim trailing whitespace from each line and join
    let help_output = help_output
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    // Format for README display:
    // 1. Replace " - " with em dash in first line (command description)
    // 2. Split at first ## header - synopsis in code block, rest as markdown
    // 3. Increase heading levels in docs section (## -> ###, ### -> ####)
    //    so they become children of the command heading (which is ##)
    let result = if let Some(first_newline) = help_output.find('\n') {
        let (first_line, rest) = help_output.split_at(first_newline);
        // Replace hyphen-minus with em dash in command description
        let first_line = first_line.replacen(" - ", " — ", 1);

        if let Some(header_pos) = rest.find("\n## ") {
            // Split at first H2 header
            let (synopsis, docs) = rest.split_at(header_pos);
            let docs = docs.trim_start_matches('\n');
            // Increase heading levels so docs headings become children of command heading
            let docs = increase_heading_levels(docs);
            format!("```\n{}{}\n```\n\n{}", first_line, synopsis, docs)
        } else {
            // No documentation section, wrap everything in code block
            format!("```\n{}{}\n```", first_line, rest)
        }
    } else {
        // Single line output
        help_output.replacen(" - ", " — ", 1)
    };

    Ok(result)
}

/// Increase markdown heading levels by one (## -> ###, ### -> ####, etc.)
/// This makes help output headings children of the command heading in docs.
/// Only transforms actual markdown headings, not code block content.
fn increase_heading_levels(content: &str) -> String {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        // Track code block boundaries (``` or ````+)
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            result.push(line.to_string());
            continue;
        }

        // Only transform headings outside code blocks
        if !in_code_block && line.starts_with('#') {
            result.push(format!("#{}", line));
        } else {
            result.push(line.to_string());
        }
    }

    result.join("\n")
}

/// Extract templates from llm.rs source
fn extract_templates(content: &str) -> std::collections::HashMap<String, String> {
    RUST_RAW_STRING_PATTERN
        .captures_iter(content)
        .map(|cap| {
            let name = cap.get(1).unwrap().as_str().to_string();
            let template = cap.get(2).unwrap().as_str().to_string();
            (name, template)
        })
        .collect()
}

// =============================================================================
// Docs-to-README Section Sync
// =============================================================================

/// Extract sections from markdown content by anchor range
///
/// If `anchor` contains `..`, extracts from start anchor through end anchor (inclusive).
/// Otherwise extracts a single section.
fn extract_section_by_anchor(content: &str, anchor: &str) -> Option<String> {
    let (start_anchor, end_anchor) = if let Some((start, end)) = anchor.split_once("..") {
        (start, Some(end))
    } else {
        (anchor, None)
    };

    let lines: Vec<&str> = content.lines().collect();

    // Find the start heading
    let start_idx = lines.iter().position(|line| {
        line.strip_prefix("## ")
            .or_else(|| line.strip_prefix("### "))
            .is_some_and(|text| heading_to_anchor(text) == start_anchor)
    })?;

    // Find the end: either after end_anchor section, or next same-level heading
    let end_idx = if let Some(end_anchor) = end_anchor {
        // Find where end_anchor's section ends
        let end_heading_idx = lines.iter().skip(start_idx + 1).position(|line| {
            line.strip_prefix("## ")
                .or_else(|| line.strip_prefix("### "))
                .is_some_and(|text| heading_to_anchor(text) == end_anchor)
        })? + start_idx
            + 1;

        // Find the next ## heading after end_anchor (or EOF)
        lines
            .iter()
            .skip(end_heading_idx + 1)
            .position(|line| line.starts_with("## "))
            .map(|i| i + end_heading_idx + 1)
            .unwrap_or(lines.len())
    } else {
        // Single section: find next ## heading
        lines
            .iter()
            .skip(start_idx + 1)
            .position(|line| line.starts_with("## "))
            .map(|i| i + start_idx + 1)
            .unwrap_or(lines.len())
    };

    let section = lines[start_idx..end_idx].join("\n").trim().to_string();
    Some(section)
}

/// Convert heading text to anchor format (lowercase, spaces to hyphens)
fn heading_to_anchor(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Transform site Markdown for embedding in GitHub-rendered files.
///
/// Converts:
/// - `[text](/page/)` → `[text](https://worktrunk.dev/page/)`
/// - `<figure class="demo">...<img src="/assets/X.gif"...>...</figure>` → `![alt](raw.githubusercontent.com/.../X.gif)`
/// - AUTO-GENERATED marker comments → removed, leaving their Markdown body
fn transform_docs_to_github(content: &str) -> String {
    // Transform internal links
    let content = SITE_LINK_PATTERN
        .replace_all(content, |caps: &regex::Captures| {
            let text = caps.get(1).unwrap().as_str();
            let page = caps.get(2).unwrap().as_str();
            let anchor = caps.get(3).map_or("", |m| m.as_str());
            format!("[{text}](https://worktrunk.dev/{page}/{anchor})")
        })
        .into_owned();
    let content = AUTO_GENERATED_MARKER_PATTERN
        .replace_all(&content, "")
        .into_owned();
    let content = content.replace(worktrunk::docs::BADGE_EXPERIMENTAL_HTML, "[experimental]");

    // Transform figure/picture elements to markdown images with GitHub raw URLs
    let content = HTML_FIGURE_TO_IMAGE_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            let filename = caps.get(1).unwrap().as_str();
            let alt = caps.get(2).unwrap().as_str();
            format!(
                "![{alt}](https://raw.githubusercontent.com/max-sixty/worktrunk-assets/main/assets/{filename})"
            )
        })
        .into_owned();
    assert_no_untransformed_site_links(&content, "README content");
    content
}

/// Get section content from docs file, transformed for README
///
/// Parses `path#anchor` ID format, extracts section(s) by anchor
/// (supports ranges like `start..end`), and makes site links absolute.
fn docs_section_for_readme(id: &str, project_root: &Path) -> Result<String, String> {
    let (path, anchor) = id
        .split_once('#')
        .ok_or_else(|| format!("Invalid section ID (missing #): {}", id))?;

    let docs_path = project_root.join(path);
    let content = fs::read_to_string(&docs_path)
        .map_err(|e| format!("Failed to read {}: {}", docs_path.display(), e))?;

    let section = extract_section_by_anchor(&content, anchor)
        .ok_or_else(|| format!("Section '{}' not found in {}", anchor, docs_path.display()))?;

    Ok(transform_docs_to_github(&section))
}

/// Get content for a README marker based on its type
///
/// Handles help (`cmd`) and section (#anchor) markers.
fn generate_readme_content(
    id: &str,
    _current_content: &str,
    project_root: &Path,
) -> Result<String, String> {
    match MarkerType::from_id(id) {
        MarkerType::Snapshot => unreachable!("README has no snapshot markers"),
        MarkerType::Help => help_output(id, project_root),
        MarkerType::Section => docs_section_for_readme(id, project_root).map(|c| trim_lines(&c)),
    }
}

/// Sync all README markers in a single pass
///
/// Processes all AUTO-GENERATED markers in one regex traversal:
/// - Help commands (`cmd`) - rendered markdown from --help-md
/// - Doc sections (#anchor) - extracted content from docs
fn sync_readme_markers(
    readme_content: &str,
    project_root: &Path,
) -> Result<(String, usize, usize), Vec<String>> {
    let mut result = readme_content.to_string();
    let mut errors = Vec::new();
    let mut updated = 0;

    // Collect all matches first
    let matches: Vec<_> = MARKER_PATTERN
        .captures_iter(readme_content)
        .map(|cap| {
            let full_match = cap.get(0).unwrap();
            let id = cap.get(1).unwrap().as_str().trim().to_string();
            let current = cap.get(2).unwrap().as_str().to_string();
            (full_match.start(), full_match.end(), id, current)
        })
        .collect();

    let total = matches.len();

    // Process in reverse order to preserve positions. README markers are always
    // Help/Section (unwrapped); generated snapshot marker comments are removed
    // when docs sections are embedded.
    for (start, end, id, current) in matches.into_iter().rev() {
        if matches!(MarkerType::from_id(&id), MarkerType::Snapshot) {
            errors.push(format!(
                "❌ {id}: README must not contain snapshot markers — \
                 transform_docs_to_github should have stripped this"
            ));
            continue;
        }

        let expected = match generate_readme_content(&id, &current, project_root) {
            Ok(content) => content,
            Err(e) => {
                errors.push(format!("❌ {}: {}", id, e));
                continue;
            }
        };

        // Compare with trim_lines normalization applied once to each side
        if trim_lines(&current) != trim_lines(&expected) {
            let replacement = format_replacement(&id, &expected, &OutputFormat::Unwrapped);
            result.replace_range(start..end, &replacement);
            updated += 1;
        }
    }

    if errors.is_empty() {
        Ok((result, updated, total))
    } else {
        Err(errors)
    }
}

/// Transform user config markdown to config.example.toml format
///
/// # Design
///
/// The source content is the user config section in `src/cli/mod.rs`, embedded between
/// `<!-- USER_CONFIG_START -->` and `<!-- USER_CONFIG_END -->` markers. This markdown
/// is designed as a great explainer for configuration options, containing prose
/// explanations and TOML code blocks showing example values.
///
/// The generated file (`dev/config.example.toml`) is the entire source with every line
/// `# ` prefixed and code fence markers stripped. This creates a fully-commented config
/// file that serves as inline documentation. Code blocks show default values (single `#`
/// prefix in the output); users uncomment the relevant `key = value` line to customize.
///
/// # Transform Rules
///
/// 1. Code fence markers (```` ``` ````, ```` ```toml ````) → stripped entirely
/// 2. Markdown links → converted to plain URLs (config files aren't rendered as markdown)
/// 3. All other lines → prefixed with `# `
/// 4. Trailing empty comment lines → trimmed
fn transform_config_source_to_toml(source: &str) -> String {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in source.lines() {
        let trimmed = line.trim();

        // Strip code fence markers
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        // Convert markdown links to plain text for config file readability
        // [Link text](/page/) → Link text (https://worktrunk.dev/page/)
        // [Link text](https://...) → Link text (https://...)
        let line = convert_markdown_links_for_config(line);

        // Comment all lines
        if line.is_empty() {
            result.push(String::from("#"));
        } else {
            result.push(format!("# {}", line));
        }
    }

    // Clean up: remove trailing empty comment lines
    while result.last().is_some_and(|l| l == "#" || l.is_empty()) {
        result.pop();
    }

    let result = result.join("\n");

    // Guardrail: a link `convert_markdown_links_for_config` declined to match
    // survives as raw markdown into the generated example file, which
    // `wt config create` writes to the user's config and `wt config create
    // --help` prints — so a root-relative site link would reach the user verbatim.
    assert_no_untransformed_site_links(&result, "a generated config example");

    result
}

/// Convert markdown links to plain text with URL in parentheses.
///
/// Config files aren't rendered as markdown, so links need to be readable as plain text.
/// - `[Link text](/page/)` → `Link text (https://worktrunk.dev/page/)`
/// - `[Link text](https://example.com)` → `Link text (https://example.com)`
///
/// The link text may itself contain a bracketed span — a TOML section name in
/// backticks (``[pattern-keyed `[projects]` entry](/config/#…)``) is the
/// shape that occurs here. A link that isn't converted survives as raw
/// markdown into the generated example file, which `wt config create` writes
/// to the user's config and `wt config create --help` prints, so a site-relative
/// target reaches the user verbatim. Brackets in these link texts always sit
/// inside a backticked code span, so the text class alternates a code span
/// with any non-`]`-non-backtick char — the same rule `SITE_LINK_PATTERN`
/// uses above, which also covers `[[…]]` array-of-tables names.
fn convert_markdown_links_for_config(line: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static MARKDOWN_LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[((?:`[^`]*`|[^\]`])+)\]\(([^)]+)\)").unwrap());

    MARKDOWN_LINK
        .replace_all(line, |caps: &regex::Captures| {
            let text = &caps[1];
            let url = &caps[2];

            let url = if let Some(path) = url.strip_prefix('/') {
                format!("https://worktrunk.dev/{path}")
            } else {
                url.to_string()
            };

            format!("{text} ({url})")
        })
        .to_string()
}

/// Every link form the config sections use converts to plain text, including
/// one whose text carries a bracketed span of its own.
///
/// The generated file is what `wt config create` writes and what `wt config
/// create --help` prints, so a link this misses ships a raw site target
/// to the user. Nothing downstream catches that: the sync test compares the
/// example against this same transform, so an unconverted link is "in sync".
#[test]
fn test_config_markdown_links_convert_to_plain_text() {
    let cases = [
        // Site page link, and the same with an anchor.
        (
            "See [hooks](/hook/) for details",
            "See hooks (https://worktrunk.dev/hook/) for details",
        ),
        (
            "See [forge platform](/config/#forge-platform).",
            "See forge platform (https://worktrunk.dev/config/#forge-platform).",
        ),
        // An absolute URL passes through untouched.
        (
            "See [the spec](https://example.com/a) too",
            "See the spec (https://example.com/a) too",
        ),
        // Link text containing a bracketed span — a TOML section name in
        // backticks. The pre-fix regex stopped at the inner `]` and left the
        // whole link as raw markdown.
        (
            "name it once with a [pattern-keyed `[projects]` entry](/config/#user-project-specific-settings) instead",
            "name it once with a pattern-keyed `[projects]` entry (https://worktrunk.dev/config/#user-project-specific-settings) instead",
        ),
        // The same shape naming an array-of-tables. These sections document
        // `[[projects."…".post-start]]` pipelines, so a link naming one is the
        // next form to arrive; the code-span class covers it.
        (
            "see [`[[projects.\"…\".post-start]]` hooks](/config/#hooks) for the pipeline form",
            "see `[[projects.\"…\".post-start]]` hooks (https://worktrunk.dev/config/#hooks) for the pipeline form",
        ),
        // Two links on one line still both convert.
        (
            "[a](/hook/) and [b](/config/)",
            "a (https://worktrunk.dev/hook/) and b (https://worktrunk.dev/config/)",
        ),
        // A bare bracketed span is not a link and must survive verbatim —
        // `[forge]` and `[list]` appear all over these sections.
        (
            "A repository's own `[forge]` still wins, field by field.",
            "A repository's own `[forge]` still wins, field by field.",
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(
            convert_markdown_links_for_config(input),
            expected,
            "input: {input}"
        );
    }
}

/// A link shape the rewrite declines to match fails loudly rather than
/// shipping its site-relative target.
///
/// This is the backstop for the case the test above can't anticipate: the next
/// unsupported link text. The generated config example runs through
/// `transform_config_source_to_toml`, so the assertion is what turns "the
/// regex silently declined" into a test failure naming the line.
#[test]
#[should_panic(expected = "a generated config example")]
fn test_untransformed_site_link_fails_the_config_transform() {
    // An unbalanced backtick in the link text: the code-span alternative can't
    // close, so the rewrite declines and the raw target would survive.
    transform_config_source_to_toml("See [a `broken span](/config/#hooks) here");
}

#[test]
#[should_panic(expected = "legacy syntax")]
fn test_legacy_zola_link_fails_the_guardrail() {
    assert_no_untransformed_site_links("See [hooks](@/hook.md)", "test content");
}

/// Extract a config section from src/cli/mod.rs by marker pattern.
fn extract_config_section(cli_mod_content: &str, pattern: &Regex, label: &str) -> String {
    pattern
        .captures(cli_mod_content)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| panic!("{label} markers not found in src/cli/mod.rs"))
}

/// Verify a config example file is in sync with its source section in mod.rs.
///
/// If out of sync, overwrites the file and panics so CI fails.
fn assert_config_example_in_sync(
    cli_mod_content: &str,
    pattern: &Regex,
    marker_label: &str,
    example_path: &Path,
) {
    let source = extract_config_section(cli_mod_content, pattern, marker_label);
    let expected = trim_lines(&transform_config_source_to_toml(&source));

    let current = fs::read_to_string(example_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", example_path.display(), e));
    let current = trim_lines(&current);

    if current != expected {
        fs::write(example_path, format!("{}\n", expected)).unwrap();
        panic!(
            "{} out of sync with {} section in src/cli/mod.rs. \
             Run tests locally and commit the changes.",
            example_path.file_name().unwrap().to_string_lossy(),
            marker_label,
        );
    }
}

#[test]
fn test_config_source_generates_example_toml() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_mod_content = fs::read_to_string(project_root.join("src/cli/mod.rs"))
        .unwrap_or_else(|e| panic!("Failed to read src/cli/mod.rs: {e}"));

    assert_config_example_in_sync(
        &cli_mod_content,
        &USER_CONFIG_PATTERN,
        "USER_CONFIG_START/END",
        &project_root.join("dev/config.example.toml"),
    );
}

#[test]
fn test_project_config_source_generates_example_toml() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_mod_content = fs::read_to_string(project_root.join("src/cli/mod.rs"))
        .unwrap_or_else(|e| panic!("Failed to read src/cli/mod.rs: {e}"));

    assert_config_example_in_sync(
        &cli_mod_content,
        &PROJECT_CONFIG_PATTERN,
        "PROJECT_CONFIG_START/END",
        &project_root.join("dev/wt.example.toml"),
    );
}

/// Verify that all user config struct fields are documented in the user config example.
///
/// Section names are derived from `UserConfig`'s JsonSchema, so adding a new field
/// to the struct automatically fails this test if the docs aren't updated.
#[test]
fn test_config_docs_include_all_sections() {
    use std::collections::HashSet;
    use strum::IntoEnumIterator;
    use worktrunk::config::{DEPRECATED_SECTION_KEYS, valid_user_config_keys};
    use worktrunk::git::HookType;

    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_mod_path = project_root.join("src/cli/mod.rs");
    let cli_mod_content = fs::read_to_string(&cli_mod_path).unwrap();
    let user_config_content =
        extract_config_section(&cli_mod_content, &USER_CONFIG_PATTERN, "USER_CONFIG");

    let all_keys = valid_user_config_keys();

    // Hook keys from HookType enum + `pre-create`/`post-create` aliases (see
    // `HooksConfig` in src/config/hooks.rs).
    let hook_keys: HashSet<String> = HookType::iter()
        .map(|h| h.to_string())
        .chain(["pre-create".to_string(), "post-create".to_string()])
        .collect();

    // Keys that are bare scalars or internal flags, not TOML section headers
    let non_section_keys: HashSet<&str> = [
        "worktree-path",
        "skip-shell-integration-prompt",
        "skip-commit-generation-prompt",
    ]
    .into();

    // Separate schema keys into section keys (excluding hooks and bare scalars)
    let section_keys: Vec<&String> = all_keys
        .iter()
        .filter(|k| !hook_keys.contains(*k) && !non_section_keys.contains(k.as_str()))
        .collect();

    // Check non-deprecated sections appear as TOML headers ([key] or [key.something])
    for key in &section_keys {
        if DEPRECATED_SECTION_KEYS
            .iter()
            .any(|d| d.key == key.as_str())
        {
            let header = format!("[{key}]");
            assert!(
                !user_config_content.contains(&header),
                "Deprecated section `{header}` should not appear in user config docs.\n\
                 Use the new section name instead."
            );
        } else {
            let header = format!("[{key}]");
            let nested = format!("[{key}.");
            assert!(
                user_config_content.contains(&header) || user_config_content.contains(&nested),
                "Config section `[{key}]` (from UserConfig schema) is missing from user \
                 config docs in src/cli/mod.rs.\nAll config sections must be documented between \
                 USER_CONFIG_START/END markers."
            );
        }
    }
}

/// Verify that all project config struct fields are documented in the project config example.
///
/// Section names are derived from `ProjectConfig`'s JsonSchema, so adding a new field
/// to the struct automatically fails this test if the docs aren't updated.
#[test]
fn test_project_config_docs_include_all_sections() {
    use std::collections::HashSet;
    use strum::IntoEnumIterator;
    use worktrunk::config::{DEPRECATED_SECTION_KEYS, valid_project_config_keys};
    use worktrunk::git::HookType;

    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_mod_path = project_root.join("src/cli/mod.rs");
    let cli_mod_content = fs::read_to_string(&cli_mod_path).unwrap();
    let project_config_content =
        extract_config_section(&cli_mod_content, &PROJECT_CONFIG_PATTERN, "PROJECT_CONFIG");

    let all_keys = valid_project_config_keys();

    // Hook keys from HookType enum + `pre-create`/`post-create` aliases (see
    // `HooksConfig` in src/config/hooks.rs).
    let hook_keys: HashSet<String> = HookType::iter()
        .map(|h| h.to_string())
        .chain(["pre-create".to_string(), "post-create".to_string()])
        .collect();

    // Separate schema keys into section keys and hook keys
    let section_keys: Vec<&String> = all_keys
        .iter()
        .filter(|k| !hook_keys.contains(*k))
        .collect();

    // Check non-deprecated sections appear as TOML headers ([key] or [key.something])
    for key in &section_keys {
        if DEPRECATED_SECTION_KEYS
            .iter()
            .any(|d| d.key == key.as_str())
        {
            let header = format!("[{key}]");
            assert!(
                !project_config_content.contains(&header),
                "Deprecated section `{header}` should not appear in project config docs.\n\
                 Use the new section name instead."
            );
        } else {
            let header = format!("[{key}]");
            let nested = format!("[{key}.");
            assert!(
                project_config_content.contains(&header)
                    || project_config_content.contains(&nested),
                "Config section `[{key}]` (from ProjectConfig schema) is missing from project \
                 config docs in src/cli/mod.rs.\nAll config sections must be documented between \
                 PROJECT_CONFIG_START/END markers."
            );
        }
    }

    // Hooks section should exist (individual hook keys are documented in user config
    // and cross-referenced from project config)
    assert!(
        project_config_content.contains("## Hooks"),
        "Hooks section heading missing from project config docs.\n\
         Expected `## Hooks` between PROJECT_CONFIG_START/END markers."
    );
}

/// Verify that LLM tool commands in the Starlight LLM commits page match
/// the examples in config.example.toml (the single source of truth).
#[test]
fn test_llm_docs_commands_match_config_example() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config_example = fs::read_to_string(project_root.join("dev/config.example.toml")).unwrap();
    let llm_docs =
        fs::read_to_string(project_root.join("docs/src/content/docs/llm-commits.md")).unwrap();

    // Extract commands from config example: "# command = ..." lines
    let config_commands: Vec<String> = config_example
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| line.starts_with("command = "))
        .filter_map(|line| {
            let table: toml::Table = toml::from_str(line).ok()?;
            Some(table["command"].as_str()?.to_string())
        })
        .collect();

    // Extract commands from llm-commits.md: "command = ..." lines in TOML code blocks
    let doc_commands: Vec<String> = llm_docs
        .lines()
        .filter(|line| line.starts_with("command = "))
        .filter_map(|line| {
            let table: toml::Table = toml::from_str(line).ok()?;
            Some(table["command"].as_str()?.to_string())
        })
        .collect();

    assert!(
        config_commands.len() >= 2,
        "Expected at least 2 tool commands in config.example.toml, found {}",
        config_commands.len()
    );

    for cmd in &config_commands {
        assert!(
            doc_commands.contains(cmd),
            "Command from config.example.toml not found in docs/src/content/docs/llm-commits.md:\n  {cmd}\n\
             Update llm-commits.md to match the config example (source of truth: dev/config.example.toml, \
             generated from src/cli/mod.rs)."
        );
    }
}

/// Verify that LLM tool commands in Taskfile.yaml bench-llm-commits match
/// the examples in config.example.toml (the single source of truth).
/// Only compares tools present in both files — either side may have tools the other lacks.
#[test]
fn test_taskfile_llm_commands_match_config_example() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config_example = fs::read_to_string(project_root.join("dev/config.example.toml")).unwrap();
    let taskfile = fs::read_to_string(project_root.join("Taskfile.yaml")).unwrap();

    // Extract tool -> command from config example using h3 headings for tool names
    // e.g. "# ### Claude Code" heading followed by '# command = "..."' line
    let mut config_commands = std::collections::HashMap::new();
    let mut current_tool: Option<String> = None;
    for line in config_example.lines() {
        if let Some(heading) = line.strip_prefix("# ### ") {
            current_tool = heading.split_whitespace().next().map(|s| s.to_lowercase());
        } else if let Some(cmd_line) = line.strip_prefix("# ")
            && cmd_line.starts_with("command = ")
            && let Some(ref tool) = current_tool
            && let Ok(table) = toml::from_str::<toml::Table>(cmd_line)
            && let Some(cmd) = table.get("command").and_then(|v| v.as_str())
        {
            config_commands.insert(tool.clone(), cmd.to_string());
        }
    }

    // Extract tool -> command from Taskfile: COMMANDS["tool"]='shell-escaped-value'
    // Unescape bash's '"'"' idiom (literal single quote) then strip outer quotes
    let taskfile_re = Regex::new(r#"COMMANDS\["(\w+)"\]=(.*)"#).unwrap();
    let taskfile_commands: std::collections::HashMap<String, String> = taskfile
        .lines()
        .filter_map(|line| {
            let caps = taskfile_re.captures(line.trim())?;
            let tool = caps[1].to_string();
            let raw = &caps[2];
            let unescaped = raw.replace("'\"'\"'", "'");
            let cmd = unescaped
                .strip_prefix('\'')?
                .strip_suffix('\'')?
                .to_string();
            Some((tool, cmd))
        })
        .collect();

    // Compare only tools present in both
    let mut checked = 0;
    for (tool, taskfile_cmd) in &taskfile_commands {
        if let Some(config_cmd) = config_commands.get(tool.as_str()) {
            assert_eq!(
                config_cmd, taskfile_cmd,
                "Command mismatch for '{tool}'.\n\
                 Config example: {config_cmd}\n\
                 Taskfile:       {taskfile_cmd}\n\
                 Update Taskfile.yaml to match dev/config.example.toml (source of truth)."
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 1,
        "No overlapping tools between config.example.toml and Taskfile.yaml"
    );
}

#[test]
fn test_config_source_templates_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let llm_rs_path = project_root.join("src/llm.rs");
    let cli_mod_path = project_root.join("src/cli/mod.rs");

    let llm_content = fs::read_to_string(&llm_rs_path).unwrap();
    let cli_mod_content = fs::read_to_string(&cli_mod_path).unwrap();

    // Extract templates from llm.rs
    let templates = extract_templates(&llm_content);
    assert!(
        templates.contains_key("DEFAULT_TEMPLATE"),
        "DEFAULT_TEMPLATE not found in src/llm.rs"
    );
    assert!(
        templates.contains_key("DEFAULT_SQUASH_TEMPLATE"),
        "DEFAULT_SQUASH_TEMPLATE not found in src/llm.rs"
    );

    let mut updated_content = cli_mod_content.clone();
    let mut updated_count = 0;

    // Helper to replace a template section in markdown format
    let mut replace_template = |pattern: &Regex, name: &str, key: &str| {
        if let Some(cap) = pattern.captures(&updated_content.clone()) {
            let full_match = cap.get(0).unwrap();
            let prefix = cap.get(1).unwrap().as_str();
            let suffix = cap.get(2).unwrap().as_str();

            let template = templates
                .get(name)
                .unwrap_or_else(|| panic!("{name} not found in src/llm.rs"));

            // Format as markdown code block
            let replacement = format!(
                r#"{prefix}```toml
[commit.generation]
{key} = """
{template}
"""
```
{suffix}"#
            );

            if full_match.as_str() != replacement {
                updated_content = updated_content.replace(full_match.as_str(), &replacement);
                updated_count += 1;
            }
        }
    };

    replace_template(&DEFAULT_TEMPLATE_PATTERN, "DEFAULT_TEMPLATE", "template");
    replace_template(
        &SQUASH_TEMPLATE_PATTERN,
        "DEFAULT_SQUASH_TEMPLATE",
        "squash-template",
    );

    if updated_count > 0 {
        fs::write(&cli_mod_path, &updated_content).unwrap();
        panic!(
            "Templates out of sync: updated {} section(s) in src/cli/mod.rs. \
             Run tests locally and commit the changes.",
            updated_count
        );
    }
}

/// Sync snapshot markers in a docs file as portable console fences.
fn sync_docs_snapshots(doc_path: &Path, project_root: &Path) -> Result<usize, Vec<String>> {
    if !doc_path.exists() {
        return Ok(0);
    }

    let content = fs::read_to_string(doc_path)
        .map_err(|e| vec![format!("Failed to read {}: {}", doc_path.display(), e)])?;

    let project_root_for_snapshots = project_root.to_path_buf();
    match update_section(
        &content,
        &DOCS_SNAPSHOT_MARKER_PATTERN,
        OutputFormat::DocsMarkdown,
        |snap_path, _current_content| {
            let full_path = project_root_for_snapshots.join(snap_path);
            let raw = fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read {}: {}", full_path.display(), e))?;

            // Extract command from snapshot YAML header
            let command = extract_command_from_snapshot(&raw);

            let plain = trim_lines(&parse_snapshot_content(&raw));
            Ok(match command {
                Some(cmd) if plain.is_empty() => format!("$ {cmd}"),
                Some(cmd) => format!("$ {cmd}\n{plain}"),
                None => plain,
            })
        },
    ) {
        Ok((new_content, updated_count, _total_count)) => {
            if updated_count > 0 {
                fs::write(doc_path, &new_content).unwrap();
            }
            Ok(updated_count)
        }
        Err(errs) => Err(errs),
    }
}

/// Update or insert the `description` field in YAML frontmatter.
///
/// Handles three cases:
/// - Description field exists → update it
/// - No description field → insert after title line
/// - No frontmatter → return content unchanged
fn sync_frontmatter_description(content: &str, description: &str) -> String {
    static DESC_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^description:\s*.*$").unwrap());

    let new_field = format!(
        "description: {}",
        serde_json::to_string(description).unwrap()
    );

    if !content.starts_with("---\n") {
        return content.to_string();
    }

    if DESC_PATTERN.is_match(content) {
        // Replace existing description
        DESC_PATTERN
            .replace(content, new_field.as_str())
            .to_string()
    } else {
        // Insert after title line
        static TITLE_PATTERN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(title:\s*.*)\n").unwrap());

        TITLE_PATTERN
            .replace(content, |caps: &regex::Captures| {
                format!("{}\n{}\n", &caps[1], new_field)
            })
            .to_string()
    }
}

#[test]
fn test_starlight_frontmatter_helpers() {
    let source = "---\ntitle: \"wt list\"\nsidebar:\n  order: 11\n---\nBody\n";
    let frontmatter = YAML_FRONTMATTER_PATTERN
        .captures(source)
        .and_then(|captures| captures.get(1))
        .unwrap();
    assert_eq!(
        yaml_scalar(frontmatter.as_str(), "title").as_deref(),
        Some("wt list")
    );

    let updated = sync_frontmatter_description(source, "List worktrees & show \"status\".");
    assert!(updated.contains("description: \"List worktrees & show \\\"status\\\".\""));
    assert!(updated.contains("sidebar:\n  order: 11"));

    let updated_again = sync_frontmatter_description(&updated, "Updated description.");
    assert_eq!(updated_again.matches("description:").count(), 1);
    assert!(updated_again.contains("description: \"Updated description.\""));
}

/// Command pages generated via `wt <cmd> --help-page`
/// Each page preserves its frontmatter and replaces the AUTO-GENERATED marker region.
/// Note: `select` is excluded because it's a deprecated hidden alias for `wt switch`.
const COMMAND_PAGES: &[&str] = &[
    "switch", "list", "merge", "remove", "config", "step", "hook",
];

/// Hand-edited site pages whose snapshot markers are refreshed by this test.
const STANDALONE_DOC_FILES: &[&str] = &[
    "docs/src/content/docs/worktrunk.md",
    "docs/src/content/docs/claude-code.md",
    "docs/src/content/docs/tips-patterns.md",
    "docs/src/content/docs/llm-commits.md",
];

/// Write `expected` to `path` and record `rel_path` in `updated`. Creates
/// parent directories as needed. Panics on I/O failure — these are test-time
/// syncs, so any write error should abort the run.
///
/// Callers are responsible for the "is it different?" check. This lets each
/// site apply its own normalization (e.g., `trim_lines`) before comparing
/// without forcing it into the helper.
fn write_tracked(
    path: &Path,
    expected: &str,
    rel_path: impl Into<String>,
    updated: &mut Vec<String>,
) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("Failed to create {}: {}", parent.display(), e));
    }
    fs::write(path, expected)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", path.display(), e));
    updated.push(rel_path.into());
}

/// Sync command pages from --help-page output to the Starlight content collection.
/// Returns (errors, updated_files)
fn sync_command_pages(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    for cmd in COMMAND_PAGES {
        let doc_path = project_root.join(format!("docs/src/content/docs/{}.md", cmd));

        // Run wt <cmd> --help-page (outputs START marker + content + END marker)
        let output = wt_command()
            .args([cmd, "--help-page"])
            .current_dir(project_root)
            .output()
            .expect("Failed to run wt --help-page");

        if !output.status.success() {
            errors.push(format!(
                "'wt {} --help-page' failed (exit {}): {}",
                cmd,
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }

        // Strip trailing whitespace from each line (pre-commit does this)
        let generated: String = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if generated.trim().is_empty() {
            errors.push(format!(
                "Empty output from 'wt {} --help-page': {}",
                cmd,
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }

        // Expand command placeholders into portable snapshot-backed console fences.
        let snapshots_dir = project_root.join("tests/snapshots");
        let generated = match expand_command_placeholders(&generated, &snapshots_dir) {
            Ok(expanded) => expanded.ansi_strip().into_owned(),
            Err(e) => {
                errors.push(format!(
                    "Failed to expand placeholders for '{}': {}",
                    cmd, e
                ));
                continue;
            }
        };

        // Get meta description from --help-description
        let desc_output = wt_command()
            .args([cmd, "--help-description"])
            .current_dir(project_root)
            .output()
            .expect("Failed to run wt --help-description");
        let description = String::from_utf8_lossy(&desc_output.stdout)
            .trim()
            .to_string();

        let current = fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", doc_path.display(), e));

        // Update frontmatter description field
        let new_content = if !description.is_empty() {
            sync_frontmatter_description(&current, &description)
        } else {
            current.clone()
        };

        // Find the help-page marker region. Non-greedy `.*?` pairs the open
        // with the nearest `MARKER_CLOSE`. Inner `AUTO-GENERATED` markers are
        // not emitted by any sync step (verified via test that ensures no
        // nesting in command pages); if that ever changes, a tempered match
        // would be needed instead of bare non-greedy.
        let id_re = regex::escape(&format!("`wt {cmd} --help-page`"));
        let marker_pattern = Regex::new(&format!(
            r"(?s){open}{id_re}[^>]*-->.*?{close}",
            open = regex::escape(MARKER_OPEN_PREFIX),
            close = regex::escape(MARKER_CLOSE),
        ))
        .unwrap();

        let new_content = if let Some(m) = marker_pattern.find(&new_content) {
            let before = &new_content[..m.start()];
            let after = &new_content[m.end()..];
            format!("{}{}{}", before, generated.trim(), after)
        } else {
            errors.push(format!(
                "No AUTO-GENERATED region found in {}. \
                 Ensure file has marker region for `wt {} --help-page`.",
                doc_path.display(),
                cmd
            ));
            continue;
        };

        if current != new_content {
            write_tracked(
                &doc_path,
                &new_content,
                format!("docs/src/content/docs/{}.md", cmd),
                &mut updated_files,
            );
        }
    }

    (errors, updated_files)
}

// =============================================================================
// Docs to Skill File Sync
// =============================================================================

/// YAML frontmatter used by Astro's Starlight content collection.
static YAML_FRONTMATTER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\n(.*?)\n---\n*").unwrap());

fn yaml_scalar(frontmatter: &str, key: &str) -> Option<String> {
    let value = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}:")))?
        .trim();
    serde_json::from_str(value)
        .ok()
        .or_else(|| (!value.is_empty()).then(|| value.to_string()))
}

/// Regex to strip AUTO-GENERATED marker comments (just the comments, not content).
/// Matches the open prefix (with ⚠️) and the bare close form.
static AUTO_GENERATED_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"{open}[^>]*-->\n*|{close}\n*",
        open = regex::escape(MARKER_OPEN_PREFIX),
        close = regex::escape(MARKER_CLOSE),
    ))
    .unwrap()
});

/// Regex to strip HTML figure/picture elements (demo GIFs)
static HTML_FIGURE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<figure[^>]*>.*?</figure>\n*").unwrap());

/// Transform docs content for skill file consumption
///
/// Transforms:
/// - Extracts title from YAML frontmatter and prepends it as H1
/// - Strips AUTO-GENERATED marker comments (keeps content)
/// - Strips HTML figure elements (demo GIFs not useful for skill)
/// - Converts site-root links to full URLs
/// - Removes "See also" section (just links to other docs pages)
fn transform_docs_for_skill(content: &str) -> String {
    // Extract title from frontmatter
    let title = YAML_FRONTMATTER_PATTERN
        .captures(content)
        .and_then(|caps| caps.get(1))
        .and_then(|fm| yaml_scalar(fm.as_str(), "title"));

    // Strip frontmatter
    let content = YAML_FRONTMATTER_PATTERN.replace(content, "");

    // Strip AUTO-GENERATED marker comments (keep content)
    let content = AUTO_GENERATED_MARKER_PATTERN.replace_all(&content, "");

    // Strip HTML figure elements (demo GIFs)
    let content = HTML_FIGURE_PATTERN.replace_all(&content, "");

    // Replace the HTML badge with its portable text equivalent.
    // Sourcing the badge HTML from `worktrunk::docs` keeps producer (help.rs)
    // and consumer (this strip) in lockstep: a format change there breaks
    // here at compile time rather than silently leaking HTML into skills.
    let content = content.replace(worktrunk::docs::BADGE_EXPERIMENTAL_HTML, "[experimental]");

    // Prepend title as H1 if extracted
    let content = if let Some(title) = title {
        format!("# {}\n\n{}", title, content.trim())
    } else {
        content.trim().to_string()
    };

    // Apply shared finalization: links, See also removal, blank line cleanup
    finalize_skill_content(&content)
}

/// Remove a section from markdown content (from heading to next same-level heading)
fn remove_section(content: &str, heading: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let heading_level = heading.chars().take_while(|&c| c == '#').count();

    if let Some(start_idx) = lines.iter().position(|line| line.starts_with(heading)) {
        // Find end: next heading at same or higher level
        let end_idx = lines
            .iter()
            .skip(start_idx + 1)
            .position(|line| {
                let level = line.chars().take_while(|&c| c == '#').count();
                level > 0 && level <= heading_level
            })
            .map(|i| i + start_idx + 1)
            .unwrap_or(lines.len());

        let mut result: Vec<&str> = lines[..start_idx].to_vec();
        result.extend(&lines[end_idx..]);
        result.join("\n")
    } else {
        content.to_string()
    }
}

/// Sorted Markdown page filenames in the Starlight content collection.
fn docs_content_page_names(docs_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(docs_dir)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", docs_dir.display(), e))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            (name.ends_with(".md") && !name.starts_with('_')).then_some(name)
        })
        .collect();
    names.sort();
    names
}

fn sync_skill_files(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let docs_dir = project_root.join("docs/src/content/docs");
    let skill_dir = project_root.join("skills/worktrunk/reference");

    let entries = docs_content_page_names(&docs_dir);

    for name in &entries {
        let skill_file = skill_dir.join(name);
        let cmd_name = name.trim_end_matches(".md");

        let expected = if COMMAND_PAGES.contains(&cmd_name) {
            // Command pages: generate directly from --help-page --plain (no HTML)
            match generate_skill_from_help(cmd_name, project_root) {
                Ok(content) => content,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            }
        } else {
            // Non-command pages: read portable site Markdown and remove site-only elements.
            let docs_file = docs_dir.join(name);
            let docs_content = fs::read_to_string(&docs_file)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", docs_file.display(), e));
            transform_docs_for_skill(&docs_content)
        };
        let expected = trim_lines(&expected);

        // Treat any read failure (incl. missing) as empty — we'll write `expected` either way.
        let current = trim_lines(&fs::read_to_string(&skill_file).unwrap_or_default());

        if current != expected {
            write_tracked(
                &skill_file,
                &format!("{expected}\n"),
                format!("skills/worktrunk/reference/{name}"),
                &mut updated_files,
            );
        }
    }

    (errors, updated_files)
}

/// Generate a skill reference file directly from `--help-page --plain` output.
///
/// For command pages, this produces clean markdown without HTML. The only
/// post-processing needed is link expansion and section cleanup.
fn generate_skill_from_help(cmd: &str, project_root: &Path) -> Result<String, String> {
    let output = wt_command()
        .args([cmd, "--help-page", "--plain"])
        .current_dir(project_root)
        .output()
        .expect("Failed to run wt --help-page --plain");

    if !output.status.success() {
        return Err(format!(
            "'wt {} --help-page --plain' failed (exit {}): {}",
            cmd,
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let content = String::from_utf8_lossy(&output.stdout).to_string();
    if content.trim().is_empty() {
        return Err(format!(
            "Empty output from 'wt {} --help-page --plain': {}",
            cmd,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Expand command placeholders (e.g., <!-- wt list --> → plain text snapshot output)
    let snapshots_dir = project_root.join("tests/snapshots");
    let content = expand_command_placeholders(&content, &snapshots_dir)?;

    Ok(finalize_skill_content(&content))
}

/// Apply final transforms shared between command and non-command skill files:
/// Site-root links → full URLs, remove "See also", and collapse blank lines.
fn finalize_skill_content(content: &str) -> String {
    let content = SITE_LINK_PATTERN
        .replace_all(content, |caps: &regex::Captures| {
            let text = caps.get(1).unwrap().as_str();
            let page = caps.get(2).unwrap().as_str();
            let anchor = caps.get(3).map_or("", |m| m.as_str());
            format!("[{text}](https://worktrunk.dev/{page}/{anchor})")
        })
        .into_owned();

    // Installed skills don't have the site's root URL as a resolution base.
    assert_no_untransformed_site_links(&content, "skill content");

    // Remove "See also" section (just contains links to other pages)
    let content = remove_section(&content, "## See also");

    // Clean up multiple consecutive blank lines
    content
        .lines()
        .fold((Vec::new(), false), |(mut acc, prev_blank), line| {
            let is_blank = line.trim().is_empty();
            if !(is_blank && prev_blank) {
                acc.push(line);
            }
            (acc, is_blank)
        })
        .0
        .join("\n")
}

/// Mirror the repo-root `skills/` tree into `plugins/worktrunk/skills/` as
/// regular files, dereferencing symlinks, and delete mirror files whose
/// source is gone.
///
/// The mirror is what Claude and Codex installs ship. It must hold real files
/// only: Codex's plugin installer copies the plugin root with a copier that
/// silently skips symlink entries (`copy_dir_recursive` in codex-rs
/// core-plugins), so a symlink anywhere in the tree — a `skills` link at the
/// top or a nested one like `reference/README.md` — ships no content, and a
/// symlink also materializes as a plain text file on Windows checkouts.
/// Repo-root `skills/` stays the authored home: Gemini reads it directly, and
/// the earlier sync stages write into it.
fn sync_plugin_skills_mirror(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let source_root = project_root.join("skills");
    let mirror_root = project_root.join("plugins/worktrunk/skills");

    // `is_dir` and `read` follow symlinks, so linked source content lands in
    // the collected map — and therefore in the mirror — as regular file bytes.
    fn collect_files(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect_files(root, &path, files)?;
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                files.insert(rel, fs::read(&path)?);
            }
        }
        Ok(())
    }

    let mut source_files = BTreeMap::new();
    if let Err(e) = collect_files(&source_root, &source_root, &mut source_files) {
        errors.push(format!("walk {}: {e}", source_root.display()));
        return (errors, updated_files);
    }
    let mut mirror_files = BTreeMap::new();
    if mirror_root.exists()
        && let Err(e) = collect_files(&mirror_root, &mirror_root, &mut mirror_files)
    {
        errors.push(format!("walk {}: {e}", mirror_root.display()));
        return (errors, updated_files);
    }

    for (rel, content) in &source_files {
        if mirror_files
            .get(rel)
            .is_none_or(|mirrored| mirrored != content)
        {
            let dst = mirror_root.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)
                    .unwrap_or_else(|e| panic!("Failed to create {}: {}", parent.display(), e));
            }
            fs::write(&dst, content)
                .unwrap_or_else(|e| panic!("Failed to write {}: {}", dst.display(), e));
            updated_files.push(format!("plugins/worktrunk/skills/{}", rel.display()));
        }
    }
    for rel in mirror_files.keys() {
        if !source_files.contains_key(rel) {
            let stale = mirror_root.join(rel);
            fs::remove_file(&stale)
                .unwrap_or_else(|e| panic!("Failed to remove {}: {}", stale.display(), e));
            updated_files.push(format!(
                "plugins/worktrunk/skills/{} (removed)",
                rel.display()
            ));
        }
    }

    (errors, updated_files)
}

/// Sync .well-known/agent-skills/ index.json and verify symlink.
///
/// The skill files are served via a symlink:
///   docs/public/.well-known/agent-skills/worktrunk → ../../../../skills/worktrunk
///
/// This function verifies the symlink is correct and generates index.json
/// with the correct SHA-256 digest per the Cloudflare agent-skills-discovery RFC.
fn sync_well_known_skills(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let well_known_dir = project_root.join("docs/public/.well-known/agent-skills");
    let symlink_path = well_known_dir.join("worktrunk");

    // Verify the symlink exists and points to the right place
    let expected_target = Path::new("../../../../skills/worktrunk");
    match fs::read_link(&symlink_path) {
        Ok(target) if target == expected_target => {}
        Ok(target) => {
            errors.push(format!(
                "Symlink at {} points to {:?}, expected {:?}",
                symlink_path.display(),
                target,
                expected_target
            ));
            return (errors, updated_files);
        }
        Err(_) => {
            errors.push(format!(
                "Expected symlink at {} → {:?}, but it doesn't exist or isn't a symlink",
                symlink_path.display(),
                expected_target
            ));
            return (errors, updated_files);
        }
    }

    // Read SKILL.md (through the symlink) for digest and description
    let skill_md_path = symlink_path.join("SKILL.md");
    let skill_md_bytes = match fs::read(&skill_md_path) {
        Ok(b) => b,
        Err(e) => {
            errors.push(format!("read {}: {e}", skill_md_path.display()));
            return (errors, updated_files);
        }
    };

    // Generate index.json with SHA-256 digest of SKILL.md
    let digest = {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(&skill_md_bytes);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256:{hex}")
    };

    // Parse the description from SKILL.md frontmatter
    let description = std::str::from_utf8(&skill_md_bytes)
        .ok()
        .and_then(|s| s.strip_prefix("---\n"))
        .and_then(|rest| rest.split_once("\n---"))
        .and_then(|(frontmatter, _)| {
            frontmatter
                .lines()
                .find(|line| line.starts_with("description:"))
                .map(|line| line.trim_start_matches("description:").trim().to_string())
        })
        .unwrap_or_default();

    let index_json = format!(
        "{{\n  \"$schema\": \"https://schemas.agentskills.io/discovery/0.2.0/schema.json\",\n  \"skills\": [\n    {{\n      \"name\": \"worktrunk\",\n      \"type\": \"skill-md\",\n      \"description\": {description},\n      \"url\": \"./worktrunk/SKILL.md\",\n      \"digest\": \"{digest}\"\n    }}\n  ]\n}}\n",
        description = serde_json::to_string(&description).unwrap(),
    );

    let index_dst = well_known_dir.join("index.json");
    let current_index = fs::read_to_string(&index_dst).unwrap_or_default();
    if current_index != index_json {
        write_tracked(
            &index_dst,
            &index_json,
            "docs/public/.well-known/agent-skills/index.json",
            &mut updated_files,
        );
    }

    (errors, updated_files)
}

/// Regex for `<!-- wt <id> -->\n```console\n$ <cmd>\n[body]\n``` ` blocks in
/// `src/cli/mod.rs`. The body is anything between the command line and the
/// closing fence, captured non-greedily so adjacent blocks don't overlap.
///
/// Capture groups:
/// 1. placeholder id
/// 2. display command (the `$ ...` line)
/// 3. body — multiline output lines (may be empty when the placeholder is a
///    freshly added stub with no snapshot filled in yet).
static CLI_MOD_EXAMPLE_BODY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- (wt [^>\n]+) -->\n```console\n\$ (wt [^\n]+)\n(.*?)```").unwrap()
});

/// Fill the body of each `<!-- wt <id> -->`-tagged ```console``` block in
/// `src/cli/mod.rs` with the plain-text output of the snapshot registered for
/// that id. This is the write-back half of the docs-example pipeline: it keeps
/// the terminal `--help` output (which is served verbatim from the source)
/// faithful to real command output without requiring hand maintenance.
///
/// Runs before `sync_command_pages` so `--help-page` sees the fresh bodies.
fn sync_cli_mod_example_bodies(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let cli_mod_path = project_root.join("src/cli/mod.rs");
    let content = match fs::read_to_string(&cli_mod_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!("Failed to read {}: {}", cli_mod_path.display(), e));
            return (errors, updated_files);
        }
    };
    let snapshots_dir = project_root.join("tests/snapshots");

    // Collect matches first to replace in reverse (preserves byte offsets).
    let matches: Vec<_> = CLI_MOD_EXAMPLE_BODY_PATTERN
        .captures_iter(&content)
        .map(|cap| {
            let m = cap.get(0).unwrap();
            (
                m.start(),
                m.end(),
                cap.get(1).unwrap().as_str().to_string(),
                cap.get(2).unwrap().as_str().to_string(),
                cap.get(3).unwrap().as_str().to_string(),
            )
        })
        .collect();

    let mut new_content = content.clone();
    for (start, end, placeholder_id, display_cmd, current_body) in matches.into_iter().rev() {
        let Some(snapshot_name) = command_to_snapshot(&placeholder_id) else {
            continue;
        };

        let snapshot_path = snapshots_dir.join(snapshot_name);
        let snapshot_content = match fs::read_to_string(&snapshot_path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!(
                    "Failed to read {}: {} (for placeholder '{}')",
                    snapshot_path.display(),
                    e,
                    placeholder_id
                ));
                continue;
            }
        };

        let plain = trim_lines(&parse_snapshot_content(&snapshot_content));
        // Body ends with a newline before the closing fence; match the source
        // convention so we don't churn whitespace on subsequent runs.
        let new_body = if plain.is_empty() {
            String::new()
        } else {
            format!("{plain}\n")
        };
        let replacement =
            format!("<!-- {placeholder_id} -->\n```console\n$ {display_cmd}\n{new_body}```",);

        // Compare normalized bodies (trim each line of trailing whitespace) so
        // pre-commit's trailing-whitespace trimmer doesn't create infinite loops.
        if trim_lines(&current_body) != trim_lines(&new_body) {
            new_content.replace_range(start..end, &replacement);
        }
    }

    if new_content != content {
        if let Err(e) = fs::write(&cli_mod_path, &new_content) {
            errors.push(format!("Failed to write {}: {}", cli_mod_path.display(), e));
        } else {
            updated_files.push("src/cli/mod.rs".to_string());
        }
    }

    (errors, updated_files)
}

/// Generate `docs/public/schema/list-v2.json` from `wt list --print-schema`,
/// publishing it at the `$id` the document carries
/// (`https://worktrunk.dev/schema/list-v2.json`).
///
/// Shells out rather than calling `schema_for!` directly: the `JsonEnvelope`
/// it derives from lives in the bin-only `crate::commands` tree, which an
/// integration test can't import. Same reason `sync_command_pages` runs
/// `--help-page`.
///
/// A schemars upgrade rewrites this file. That shows up here as an ordinary
/// out-of-sync failure, which is the intent — a consumer's schema changing
/// under a dependency bump should be a reviewed diff.
fn sync_json_schema(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let output = wt_command()
        .args(["list", "--print-schema"])
        .current_dir(project_root)
        .output()
        .expect("Failed to run wt list --print-schema");

    if !output.status.success() {
        errors.push(format!(
            "'wt list --print-schema' failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        ));
        return (errors, updated_files);
    }

    let generated = String::from_utf8_lossy(&output.stdout).to_string();
    if generated.trim().is_empty() {
        errors.push("Empty output from 'wt list --print-schema'".to_string());
        return (errors, updated_files);
    }

    let rel_path = "docs/public/schema/list-v2.json";
    let dst = project_root.join(rel_path);
    if fs::read_to_string(&dst).unwrap_or_default() != generated {
        write_tracked(&dst, &generated, rel_path, &mut updated_files);
    }

    (errors, updated_files)
}

/// Generate the site-only style manifest from the same ANSI snapshots that
/// populate the portable Markdown examples. Expressive Code receives plain
/// text by design; this sidecar lets it restore the CLI's semantic styling
/// without putting escape sequences or renderer-specific HTML in Markdown.
fn sync_terminal_style_manifest(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();
    let snapshots_dir = project_root.join("tests/snapshots");
    let mut snapshot_paths = BTreeSet::new();
    let mut blocks: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    snapshot_paths.extend(
        COMMAND_SNAPSHOTS
            .iter()
            .map(|(_, snapshot_name)| snapshots_dir.join(snapshot_name)),
    );
    for doc_file in STANDALONE_DOC_FILES {
        let doc_path = project_root.join(doc_file);
        let content = match fs::read_to_string(&doc_path) {
            Ok(content) => content,
            Err(error) => {
                errors.push(format!("Failed to read {}: {error}", doc_path.display()));
                continue;
            }
        };
        snapshot_paths.extend(
            DOCS_SNAPSHOT_MARKER_PATTERN
                .captures_iter(&content)
                .map(|captures| project_root.join(captures.get(1).unwrap().as_str())),
        );
    }

    for snapshot_path in snapshot_paths {
        let raw = match fs::read_to_string(&snapshot_path) {
            Ok(raw) => raw,
            Err(error) => {
                errors.push(format!(
                    "Failed to read {}: {error}",
                    snapshot_path.display()
                ));
                continue;
            }
        };
        let (plain, lines) = match styled_snapshot_lines(&raw) {
            Ok(styled) => styled,
            Err(error) => {
                errors.push(format!("{}: {error}", snapshot_path.display()));
                continue;
            }
        };
        if plain.is_empty() {
            continue;
        }
        if let Some(existing) = blocks.insert(plain.clone(), lines.clone())
            && existing != lines
        {
            errors.push(format!(
                "{}: duplicate portable output carries different ANSI styling",
                snapshot_path.display()
            ));
        }
    }

    if !errors.is_empty() {
        return (errors, updated_files);
    }

    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(
            &blocks
                .into_iter()
                .map(|(plain, lines)| serde_json::json!({ "plain": plain, "lines": lines }))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    let rel_path = "docs/src/generated/terminal-styles.json";
    let dst = project_root.join(rel_path);
    if fs::read_to_string(&dst).unwrap_or_default() != generated {
        write_tracked(&dst, &generated, rel_path, &mut updated_files);
    }

    (errors, updated_files)
}

/// Generate `docs/public/llms.txt` from the Starlight content collection,
/// following the llms.txt spec (https://llmstxt.org/): H1, blockquote summary,
/// optional intro prose, H2 section headings with bulleted link lists.
///
/// Link targets use the `.md` companion URLs (served via symlinks in
/// `docs/public/*.md` → `skills/worktrunk/reference/*.md`).
fn sync_llms_txt(project_root: &Path) -> (Vec<String>, Vec<String>) {
    use std::collections::BTreeMap;

    struct Frontmatter {
        title: String,
        description: Option<String>,
        order: i64,
    }

    let mut errors = Vec::new();
    let mut updated = Vec::new();

    let docs_dir = project_root.join("docs/src/content/docs");
    let mut site_title = None;
    let mut site_description = None;
    let base_url = "https://worktrunk.dev";

    let mut home_intro = String::new();
    // Starlight's sidebar is configured in astro.config.mjs. COMMAND_PAGES is
    // already the sync taxonomy's canonical command-page set; all other listed
    // pages are reference material. Order within each group follows
    // `sidebar.order` from frontmatter.
    let mut groups: BTreeMap<String, Vec<(String, Frontmatter)>> = BTreeMap::new();

    for name in docs_content_page_names(&docs_dir) {
        let path = docs_dir.join(&name);
        let slug = name.trim_end_matches(".md").to_string();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("read {}: {e}", path.display()));
                continue;
            }
        };
        let Some(captures) = YAML_FRONTMATTER_PATTERN.captures(&content) else {
            errors.push(format!("missing YAML frontmatter in {}", path.display()));
            continue;
        };
        let fm_text = captures.get(1).unwrap().as_str();
        let body = &content[captures.get(0).unwrap().end()..];
        let Some(title) = yaml_scalar(fm_text, "title") else {
            errors.push(format!("frontmatter in {} has no title", path.display()));
            continue;
        };
        let order = fm_text
            .lines()
            .find_map(|line| line.trim().strip_prefix("order:"))
            .and_then(|value| value.trim().parse::<i64>().ok());
        let Some(order) = order else {
            errors.push(format!(
                "frontmatter in {} has no numeric sidebar.order",
                path.display()
            ));
            continue;
        };
        let fm = Frontmatter {
            title,
            description: yaml_scalar(fm_text, "description"),
            order,
        };

        if slug == "worktrunk" {
            site_title = Some(fm.title);
            site_description = fm.description;
            home_intro = extract_intro_prose(body);
            continue;
        }

        let group = if COMMAND_PAGES.contains(&slug.as_str()) {
            "Commands"
        } else {
            "Reference"
        };

        groups
            .entry(group.to_string())
            .or_default()
            .push((slug, fm));
    }

    if !errors.is_empty() {
        return (errors, updated);
    }

    let Some(site_title) = site_title else {
        errors.push("docs content has no worktrunk.md homepage".to_string());
        return (errors, updated);
    };
    let Some(site_description) =
        site_description.filter(|description| !description.trim().is_empty())
    else {
        errors.push("worktrunk.md frontmatter has no description".to_string());
        return (errors, updated);
    };

    for pages in groups.values_mut() {
        pages.sort_by_key(|(_, fm)| fm.order);
    }
    let mut ordered: Vec<(String, Vec<(String, Frontmatter)>)> = groups.into_iter().collect();
    ordered.sort_by_key(|(_, pages)| pages.first().map(|(_, fm)| fm.order).unwrap_or(i64::MAX));

    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "# {site_title}\n").unwrap();
    writeln!(out, "> {site_description}\n").unwrap();
    if !home_intro.is_empty() {
        writeln!(out, "{home_intro}\n").unwrap();
    }
    for (group, pages) in ordered {
        writeln!(out, "## {group}\n").unwrap();
        for (slug, fm) in pages {
            let desc = fm
                .description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            writeln!(out, "- [{}]({base_url}/{slug}.md){desc}", fm.title).unwrap();
        }
        writeln!(out).unwrap();
    }

    let out = format!("{}\n", out.trim_end());

    let dst = project_root.join("docs/public/llms.txt");
    let current = fs::read_to_string(&dst).unwrap_or_default();
    if current != out {
        write_tracked(&dst, &out, "docs/public/llms.txt", &mut updated);
    }
    (errors, updated)
}

/// Take the leading prose paragraphs of a page body, stopping at the first
/// section heading or HTML block (figure, comment, etc.). The homepage uses
/// this for the llms.txt intro.
///
/// Trims trailing lines ending with `:` — those typically introduce the
/// content we just cut (a figure, code block, etc.) and dangle without it.
fn extract_intro_prose(body: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("##") || trimmed.starts_with('<') || trimmed.starts_with("<!--") {
            break;
        }
        lines.push(line);
    }
    while lines
        .last()
        .is_some_and(|l| l.trim_end().ends_with(':') || l.trim().is_empty())
    {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

/// Single end-to-end sync test that owns the full pipeline.
///
/// Steps run in dependency order, so a single pass converges and there's no
/// way for nextest parallelism to interleave the stages. The earlier
/// per-stage tests (`test_docs_quickstart_examples_are_in_sync`,
/// `test_readme_examples_are_in_sync`) collapsed into this — they shared
/// state via on-disk docs files, which made test ordering a correctness
/// requirement, not a performance choice.
#[test]
fn test_docs_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Each step's errors and updated-file list are tagged so the failure
    // message tells a developer which stage broke without grepping for code.
    let mut all_errors: Vec<String> = Vec::new();
    let mut all_files: Vec<String> = Vec::new();
    let mut tag = |stage: &str, errors: Vec<String>, files: Vec<String>| {
        all_errors.extend(errors.into_iter().map(|e| format!("[{stage}] {e}")));
        all_files.extend(files.into_iter().map(|f| format!("[{stage}] {f}")));
    };

    // Step 0: Fill docs-example bodies in src/cli/mod.rs from snapshots. Runs
    // before --help-page reads the file so command pages and skill files see
    // the up-to-date content.
    let (mod_errors, mod_files) = sync_cli_mod_example_bodies(project_root);
    tag("cli/mod.rs", mod_errors, mod_files);

    // Step 1: Sync command pages into the Starlight content collection.
    let (cmd_errors, cmd_files) = sync_command_pages(project_root);
    tag("command pages", cmd_errors, cmd_files);

    // Step 2: Sync standalone docs files from snapshots.
    // README extraction in step 5 reads these, so they must be current first.
    let mut docs_errors: Vec<String> = Vec::new();
    let mut docs_files: Vec<String> = Vec::new();
    for doc_file in STANDALONE_DOC_FILES {
        let doc_path = project_root.join(doc_file);
        match sync_docs_snapshots(&doc_path, project_root) {
            Ok(updated) => {
                if updated > 0 {
                    docs_files.push(doc_file.to_string());
                }
            }
            Err(errors) => docs_errors.extend(errors),
        }
    }
    tag("standalone docs", docs_errors, docs_files);

    // Step 2b: Preserve the ANSI semantics of snapshot-backed examples in a
    // site-only manifest while the Markdown remains portable plain text.
    let (terminal_style_errors, terminal_style_files) = sync_terminal_style_manifest(project_root);
    tag(
        "terminal styles",
        terminal_style_errors,
        terminal_style_files,
    );

    // Step 3: Sync skill files (Starlight Markdown → skills/*)
    let (skill_errors, skill_files) = sync_skill_files(project_root);
    tag("skill files", skill_errors, skill_files);

    // Step 3b: Mirror the now-fresh skills/ into plugins/worktrunk/skills/
    // (real files for the plugin payload — Codex's installer drops symlinks)
    let (mirror_errors, mirror_files) = sync_plugin_skills_mirror(project_root);
    tag("plugin skills mirror", mirror_errors, mirror_files);

    // Step 4: Sync .well-known/agent-skills/ (skills/ → docs/public/)
    let (well_known_errors, well_known_files) = sync_well_known_skills(project_root);
    tag(".well-known", well_known_errors, well_known_files);

    // Step 5: Generate docs/public/llms.txt from Starlight frontmatter.
    let (llms_errors, llms_files) = sync_llms_txt(project_root);
    tag("llms.txt", llms_errors, llms_files);

    // Step 5b: Generate docs/public/schema/list-v2.json from the derived
    // schema. Grouped here because it also writes docs/public/, but it reads
    // the binary rather than the markdown pipeline, so it has no ordering
    // dependency on the steps above.
    let (schema_errors, schema_files) = sync_json_schema(project_root);
    tag("json schema", schema_errors, schema_files);

    // Step 6: Sync README from the now-fresh docs files. Runs last because
    // section extraction depends on site Markdown being current.
    let readme_path = project_root.join("README.md");
    let readme_content = fs::read_to_string(&readme_path).unwrap();
    let mut readme_errors: Vec<String> = Vec::new();
    let mut readme_files: Vec<String> = Vec::new();
    match sync_readme_markers(&readme_content, project_root) {
        Ok((updated_content, updated_count, total_count)) => {
            assert!(total_count > 0, "No README markers found in README.md");
            if updated_count > 0 {
                fs::write(&readme_path, &updated_content).unwrap();
                readme_files.push(format!(
                    "README.md ({updated_count} of {total_count} section(s) updated)"
                ));
            }
        }
        Err(errors) => readme_errors.extend(errors),
    }
    tag("README", readme_errors, readme_files);

    if !all_errors.is_empty() {
        panic!("Sync errors:\n\n{}\n", all_errors.join("\n"));
    }

    if !all_files.is_empty() {
        panic!(
            "Files out of sync (updated):\n  {}\n\nRun tests locally and commit the changes.",
            all_files.join("\n  ")
        );
    }
}

/// `AUTO-GENERATED` markers must not nest. The help-page region's close uses
/// the bare `MARKER_CLOSE`, paired with the open via non-greedy `.*?` — a
/// nested inner close would cause the regex to chop the region short, leaving
/// stale content beyond the inner close. This test catches re-introduction of
/// nesting before that subtle failure mode lands.
#[test]
fn test_no_nested_auto_generated_markers() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for entry in fs::read_dir(project_root.join("docs/src/content/docs")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            let content = fs::read_to_string(&path).unwrap();
            let mut depth = 0;
            for (i, line) in content.lines().enumerate() {
                if line.contains(MARKER_OPEN_PREFIX) {
                    depth += 1;
                    if depth > 1 {
                        violations.push(format!(
                            "{}:{}: nested AUTO-GENERATED open (depth {depth})",
                            path.display(),
                            i + 1
                        ));
                    }
                } else if line.contains(MARKER_CLOSE) && depth > 0 {
                    depth -= 1;
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Nested AUTO-GENERATED markers found — outer help-page regex would chop \
         the region at the inner close. Either flatten the nesting or restore a \
         disambiguating close marker.\n\n{}",
        violations.join("\n")
    );
}

/// The hand-authored `## Template variables` table in `src/cli/mod.rs` must
/// match the variable constants in `src/config/expansion.rs`. Drift means the
/// help docs lie about which vars hooks and aliases can reference.
///
/// Checks presence and group placement; descriptions stay free-form prose.
#[test]
fn test_template_variables_table_matches_constants() {
    use std::collections::{BTreeMap, BTreeSet};
    use strum::IntoEnumIterator;
    use worktrunk::config::{
        ACTIVE_VARS, ALIAS_ARGS_KEY, DEPRECATED_TEMPLATE_VARS, EXEC_BASE_VARS, REPO_VARS,
        ValidationScope, vars_available_in,
    };
    use worktrunk::git::HookType;

    let cli_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/mod.rs");
    let content = fs::read_to_string(&cli_path).unwrap();

    // Carve out the `## Template variables` section: from its heading to the
    // next `\n## ` (next level-2 heading). Anchored on the exact heading so an
    // unrelated `## Template …` elsewhere can't be mistaken for it.
    let heading = "\n## Template variables\n";
    let start = content
        .find(heading)
        .expect("`## Template variables` heading missing in src/cli/mod.rs");
    let rest = &content[start + heading.len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];

    // Parse table rows: `| kind | `{{ name }}` | description |`. The kind
    // column only appears on the first row of each group — subsequent rows
    // leave it blank, inheriting the last-seen value.
    let var_re = Regex::new(r"\{\{\s*([a-zA-Z_][a-zA-Z0-9_.<>]*)\s*\}\}").unwrap();
    let mut actual: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current_kind: Option<String> = None;
    for line in section.lines() {
        if !line.starts_with("| ") || line.starts_with("|---") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // cells[0] and cells[last] are empty (leading/trailing `|`).
        if cells.len() < 4 {
            continue;
        }
        let kind_cell = cells[1];
        let var_cell = cells[2];
        // Skip the header row.
        if kind_cell == "Kind" {
            continue;
        }
        if !kind_cell.is_empty() {
            current_kind = Some(kind_cell.to_string());
        }
        let Some(kind) = current_kind.as_ref() else {
            continue;
        };
        if let Some(cap) = var_re.captures(var_cell) {
            let name = cap[1].to_string();
            actual.entry(kind.clone()).or_default().insert(name);
        }
    }

    // Build expected groups from constants.
    let mut expected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    expected.insert(
        "active".into(),
        ACTIVE_VARS.iter().map(|s| s.to_string()).collect(),
    );
    expected.insert(
        "repo".into(),
        REPO_VARS.iter().map(|s| s.to_string()).collect(),
    );
    // `exec` in the docs = runtime infra vars plus `args` (hook+alias body
    // forwarding). The `hook_type`/`hook_name` names aren't exported as a
    // constant, so they're inlined here — anchoring them to the table row
    // they appear in.
    let mut exec: BTreeSet<String> = EXEC_BASE_VARS.iter().map(|s| s.to_string()).collect();
    exec.insert("hook_type".into());
    exec.insert("hook_name".into());
    exec.insert(ALIAS_ARGS_KEY.to_string());
    expected.insert("exec".into(), exec);
    // `user` row has a single entry — the `{{ vars.<key> }}` placeholder.
    expected.insert("user".into(), BTreeSet::from(["vars.<key>".to_string()]));
    // `operation` = union of hook-type-specific extras. Derived through the
    // public `vars_available_in` so this test doesn't depend on the private
    // `hook_extras` helper.
    let base: BTreeSet<&&str> = ACTIVE_VARS
        .iter()
        .chain(REPO_VARS.iter())
        .chain(EXEC_BASE_VARS.iter())
        .chain(DEPRECATED_TEMPLATE_VARS.iter())
        .collect();
    let infra_and_args: BTreeSet<&str> = ["hook_type", "hook_name", ALIAS_ARGS_KEY].into();
    let mut operation: BTreeSet<String> = BTreeSet::new();
    for ht in HookType::iter() {
        for v in vars_available_in(ValidationScope::Hook(ht)) {
            if !base.contains(&v) && !infra_and_args.contains(v) {
                operation.insert(v.to_string());
            }
        }
    }
    expected.insert("operation".into(), operation);

    assert_eq!(
        actual, expected,
        "`## Template variables` table in src/cli/mod.rs drifted from \
         constants in src/config/expansion.rs. Update the table or the constants."
    );
}
