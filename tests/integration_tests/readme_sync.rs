//! README and config synchronization tests
//!
//! Verifies that README.md examples stay in sync with their source snapshots and help output.
//! Also syncs default templates from src/llm.rs to dev/config.example.toml.
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
use ansi_to_html::convert as ansi_to_html;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Unified pattern for all AUTO-GENERATED markers (README and docs)
/// Format: <!-- ⚠️ AUTO-GENERATED from <id> — edit <source> to update -->
/// ID types: path.snap (snapshot), `cmd` (help), path#anchor (section)
/// Content may be wrapped in ```console``` (snapshots) or unwrapped (help/sections)
static MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<!-- ⚠️ AUTO-GENERATED from ([^\n]+?) — edit [^\n]+ to update -->\n+([\s\S]*?)\n*<!-- END AUTO-GENERATED -->",
    )
    .unwrap()
});

/// Regex for literal bracket notation (as stored in snapshots) - used by literal_to_escape
static ANSI_LITERAL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[0-9;]*m").unwrap());

/// Regex to find docs snapshot markers (HTML output)
/// Format: <!-- ⚠️ AUTO-GENERATED-HTML from path.snap — edit source to update -->
static DOCS_SNAPSHOT_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<!-- ⚠️ AUTO-GENERATED-HTML from ([^\s]+\.snap) — edit source to update -->\n+\{% terminal\(\) %\}\n(.*?)\{% end %\}\n+<!-- END AUTO-GENERATED -->",
    )
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

/// Regex to extract user config section from src/cli/mod.rs
/// Matches content between USER_CONFIG_START and USER_CONFIG_END markers
static USER_CONFIG_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- USER_CONFIG_START -->\n(.*?)\n<!-- USER_CONFIG_END -->").unwrap()
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

/// Regex to convert Zola internal links to full URLs
/// Matches: [text](@/page.md) or [text](@/page.md#anchor)
static ZOLA_LINK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(@/([^)#]+)\.md(#[^)]*)?\)").unwrap());

/// Regex to convert Zola rawcode shortcode to HTML pre tags
/// Matches: {% rawcode() %}...{% end %}
static ZOLA_RAWCODE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{% rawcode\(\) %\}(.*?)\{% end %\}").unwrap());

/// Regex to convert Zola figure/picture elements to simple markdown images
/// Matches: <figure class="demo">...<img src="/assets/X.gif" alt="Y"...>...</figure>
/// Extracts: src path and alt text from the <img> tag
/// Note: Maps /assets/X to assets/X in the worktrunk-assets repo
static ZOLA_FIGURE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
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
    /// Docs: HTML with ANSI colors in {% terminal() %} shortcode
    DocsHtml,
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

    /// Get the OutputFormat for this marker type
    fn output_format(&self) -> OutputFormat {
        match self {
            Self::Snapshot => unreachable!("README has no snapshot markers"),
            Self::Help | Self::Section => OutputFormat::Unwrapped,
        }
    }

    /// Extract inner content (help/sections are unwrapped)
    fn extract_inner(&self, content: &str) -> String {
        match self {
            Self::Snapshot => unreachable!("README has no snapshot markers"),
            Self::Help | Self::Section => content.to_string(),
        }
    }
}

/// Parse a snapshot file, returning the user-facing output content
///
/// Handles:
/// - YAML front matter removal
/// - insta_cmd stdout/stderr section extraction (prefers stderr where user messages go)
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

    // Handle insta_cmd format with stdout/stderr sections
    if content.contains("----- stdout -----") {
        let stderr = extract_section(&content, "----- stderr -----\n", "----- ");
        if !stderr.is_empty() {
            return stderr;
        }
        let stdout = extract_section(&content, "----- stdout -----\n", "----- stderr -----");
        return stdout; // May be empty if both sections are empty
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

/// Replace test placeholders with display-friendly values
///
/// Transforms:
/// - `[HASH]` → `a1b2c3d`
/// - `[TMPDIR]/repo.branch` → `../repo.branch`
/// - `[TMPDIR]/repo` → `../repo`
/// - `_REPO_` → `../repo`
fn replace_placeholders(content: &str) -> String {
    let content = HASH_REGEX.replace_all(content, "a1b2c3d");
    let content = TMPDIR_BRANCH_REGEX.replace_all(&content, "../repo.$1");
    let content = TMPDIR_MAIN_REGEX.replace_all(&content, "../repo$1");
    REPO_REGEX.replace_all(&content, "../repo").into_owned()
}

/// Format replacement content based on output format
fn format_replacement(id: &str, content: &str, format: &OutputFormat) -> String {
    match format {
        OutputFormat::DocsHtml => {
            format!(
                "<!-- ⚠️ AUTO-GENERATED-HTML from {} — edit source to update -->\n\n{{% terminal() %}}\n{}\n{{% end %}}\n\n<!-- END AUTO-GENERATED -->",
                id, content
            )
        }
        OutputFormat::Unwrapped => {
            format!(
                "<!-- ⚠️ AUTO-GENERATED from {} — edit source to update -->\n\n{}\n\n<!-- END AUTO-GENERATED -->",
                id, content
            )
        }
    }
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
            let current = trim_lines(cap.get(2).unwrap().as_str());
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

        if current != expected {
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

// =============================================================================
// End Unified Infrastructure
// =============================================================================

/// Regex to find command placeholder comments in help pages
/// Matches: <!-- wt <args> -->\n```bash\n$ wt <args>\n```
/// The HTML comment triggers expansion, the code block shows in terminal help
/// Note: Pattern expects ```bash``` because --help-page converts ```console``` first
static COMMAND_PLACEHOLDER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!-- (wt [^>]+) -->\n```bash\n\$ wt [^\n]+\n```").unwrap());

/// Map commands to their snapshot files for help page expansion
fn command_to_snapshot(command: &str) -> Option<&'static str> {
    match command {
        "wt list" => Some("integration__integration_tests__list__readme_example_list.snap"),
        "wt list --full" => {
            Some("integration__integration_tests__list__readme_example_list_full.snap")
        }
        "wt list --branches --full" => {
            Some("integration__integration_tests__list__readme_example_list_branches.snap")
        }
        _ => None,
    }
}

/// Expand command placeholders in help page content to terminal shortcodes
///
/// Finds ```bash\nwt <cmd>\n``` blocks (```console``` is already converted
/// to ```bash``` by --help-page) and replaces them with {% terminal() %}
/// shortcodes containing snapshot output.
///
/// Commands without a snapshot mapping are left as plain code blocks.
fn expand_command_placeholders(content: &str, snapshots_dir: &Path) -> Result<String, String> {
    let mut result = content.to_string();
    let mut errors = Vec::new();

    // Find all placeholder blocks
    for cap in COMMAND_PLACEHOLDER_PATTERN.captures_iter(content) {
        let full_match = cap.get(0).unwrap().as_str();
        let command = cap.get(1).unwrap().as_str();

        // Skip commands without snapshot mappings - leave as plain code blocks
        let Some(snapshot_name) = command_to_snapshot(command) else {
            continue;
        };

        let snapshot_path = snapshots_dir.join(snapshot_name);
        if !snapshot_path.exists() {
            errors.push(format!(
                "Snapshot file not found: {} (for command '{}')",
                snapshot_path.display(),
                command
            ));
            continue;
        }

        let snapshot_content = fs::read_to_string(&snapshot_path)
            .map_err(|e| format!("Failed to read {}: {}", snapshot_path.display(), e))?;

        let html = parse_snapshot_content_for_docs(&snapshot_content)?;
        let normalized = trim_lines(&html);

        // Build the terminal shortcode with standard template markers
        let replacement = format!(
            "<!-- ⚠️ AUTO-GENERATED from tests/snapshots/{} — edit source to update -->\n\n\
             {{% terminal() %}}\n\
             <span class=\"prompt\">$</span> <span class=\"cmd\">{}</span>\n\
             {}\n\
             {{% end %}}\n\n\
             <!-- END AUTO-GENERATED -->",
            snapshot_name, command, normalized
        );

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

/// Parse snapshot content for docs (with ANSI to HTML conversion)
fn parse_snapshot_content_for_docs(content: &str) -> Result<String, String> {
    let content = parse_snapshot_raw(content);
    let content = replace_placeholders(&content);
    let content = literal_to_escape(&content);
    // Ensure each line ends with a reset so ansi-to-html produces clean per-line HTML.
    // This handles snapshots where trailing resets were stripped for cross-platform consistency.
    let content = ensure_line_resets(&content);
    let html = ansi_to_html(&content).map_err(|e| format!("ANSI conversion failed: {e}"))?;
    Ok(clean_ansi_html(&html))
}

/// Ensure each line ends with a reset code so ansi-to-html produces clean per-line HTML
///
/// When trailing ANSI resets are stripped from snapshots for cross-platform consistency,
/// the ansi-to-html library will carry styles across lines (e.g., `<b>text\nmore</b>`).
/// By adding a reset at the end of each line, we ensure proper HTML tag closure.
fn ensure_line_resets(ansi: &str) -> String {
    const RESET: &str = "\x1b[0m";

    ansi.lines()
        .map(|line| {
            // Add reset at end of line if it doesn't already end with one
            if line.ends_with(RESET) {
                line.to_string()
            } else {
                format!("{}{}", line, RESET)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clean up HTML output from ansi-to-html conversion
fn clean_ansi_html(html: &str) -> String {
    // Regex to remove empty HTML spans (e.g., <span style='opacity:0.67'></span>)
    static EMPTY_SPAN_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<span[^>]*></span>").unwrap());

    // Strip bare ESC characters left by the library
    let html = html.replace('\x1b', "");

    // Clean up empty tags generated by reset codes
    let html = html.replace("<b></b>", "");
    let html = EMPTY_SPAN_REGEX.replace_all(&html, "").to_string();

    // Replace verbose inline styles with CSS classes for cleaner output
    html.replace("<span style='opacity:0.67'>", "<span class=d>")
        .replace("<span style='color:var(--green,#0a0)'>", "<span class=g>")
        .replace("<span style='color:var(--red,#a00)'>", "<span class=r>")
        .replace("<span style='color:var(--cyan,#0aa)'>", "<span class=c>")
}

/// Regex to find command reference code blocks with ANSI content
/// Matches: ## Command reference\n\n```\n<content with ANSI>\n```
/// or: ### Command reference\n\n```\n<content with ANSI>\n```
static COMMAND_REF_BLOCK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)(###? Command reference\n\n)```\n(.*?)\n```").unwrap());

/// Convert command reference code blocks to terminal shortcodes with HTML
///
/// Finds code blocks after "## Command reference" or "### Command reference" headers
/// and converts ANSI escape codes to HTML, wrapping in {% terminal() %} shortcode.
fn convert_command_reference_to_html(content: &str) -> Result<String, String> {
    let mut result = content.to_string();

    // Find all command reference blocks and convert them
    // Process in reverse order to preserve positions
    let matches: Vec<_> = COMMAND_REF_BLOCK_PATTERN
        .captures_iter(content)
        .map(|cap| {
            let full_match = cap.get(0).unwrap();
            let header = cap.get(1).unwrap().as_str();
            let code_content = cap.get(2).unwrap().as_str();
            (full_match.start(), full_match.end(), header, code_content)
        })
        .collect();

    for (start, end, header, code_content) in matches.into_iter().rev() {
        // Convert ANSI to HTML
        let with_resets = ensure_line_resets(code_content);
        let html =
            ansi_to_html(&with_resets).map_err(|e| format!("ANSI conversion failed: {e}"))?;
        let clean_html = clean_ansi_html(&html);
        let trimmed_html = trim_lines(&clean_html);

        // Build terminal shortcode
        let replacement = format!("{header}{{% terminal() %}}\n{trimmed_html}\n{{% end %}}");
        result.replace_range(start..end, &replacement);
    }

    Ok(result)
}

/// Get help output for a command
///
/// Expected format: `wt <subcommand> --help-md` (ID includes backticks from marker)
fn get_help_output(id: &str, project_root: &Path) -> Result<String, String> {
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

/// Transform Zola-flavored markdown to GitHub-flavored markdown
///
/// Converts:
/// - `[text](@/page.md)` → `[text](https://worktrunk.dev/page/)`
/// - `[text](@/page.md#anchor)` → `[text](https://worktrunk.dev/page/#anchor)`
/// - `{% rawcode() %}...{% end %}` → `<pre>...</pre>`
/// - `<figure class="demo">...<img src="/assets/X.gif"...>...</figure>` → `![alt](raw.githubusercontent.com/.../X.gif)`
fn transform_zola_to_github(content: &str) -> String {
    // Transform internal links
    let content = ZOLA_LINK_PATTERN
        .replace_all(content, |caps: &regex::Captures| {
            let text = caps.get(1).unwrap().as_str();
            let page = caps.get(2).unwrap().as_str();
            let anchor = caps.get(3).map_or("", |m| m.as_str());
            format!("[{text}](https://worktrunk.dev/{page}/{anchor})")
        })
        .into_owned();

    // Transform rawcode shortcodes to pre tags
    let content = ZOLA_RAWCODE_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            let inner = caps.get(1).unwrap().as_str();
            format!("<pre>{}</pre>", inner)
        })
        .into_owned();

    // Transform figure/picture elements to markdown images with GitHub raw URLs
    ZOLA_FIGURE_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            let filename = caps.get(1).unwrap().as_str();
            let alt = caps.get(2).unwrap().as_str();
            format!(
                "![{alt}](https://raw.githubusercontent.com/max-sixty/worktrunk-assets/main/assets/{filename})"
            )
        })
        .into_owned()
}

/// Get section content from docs file, transformed for README
///
/// Parses `path#anchor` ID format, extracts section(s) by anchor
/// (supports ranges like `start..end`), and transforms Zola links to GitHub URLs.
fn get_docs_section_for_readme(id: &str, project_root: &Path) -> Result<String, String> {
    let (path, anchor) = id
        .split_once('#')
        .ok_or_else(|| format!("Invalid section ID (missing #): {}", id))?;

    let docs_path = project_root.join(path);
    let content = fs::read_to_string(&docs_path)
        .map_err(|e| format!("Failed to read {}: {}", docs_path.display(), e))?;

    let section = extract_section_by_anchor(&content, anchor)
        .ok_or_else(|| format!("Section '{}' not found in {}", anchor, docs_path.display()))?;

    // Transform Zola links to GitHub URLs
    Ok(transform_zola_to_github(&section))
}

/// Get content for a README marker based on its type
///
/// Handles help (`cmd`) and section (#anchor) markers.
fn get_readme_content(
    id: &str,
    _current_content: &str,
    project_root: &Path,
) -> Result<String, String> {
    match MarkerType::from_id(id) {
        MarkerType::Snapshot => unreachable!("README has no snapshot markers"),
        MarkerType::Help => get_help_output(id, project_root),
        MarkerType::Section => {
            get_docs_section_for_readme(id, project_root).map(|c| trim_lines(&c))
        }
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

    // Process in reverse order to preserve positions
    for (start, end, id, current_with_wrapper) in matches.into_iter().rev() {
        let marker_type = MarkerType::from_id(&id);

        // Strip wrapper from current content (snapshots have ```console```, others are raw)
        let current_inner = marker_type.extract_inner(&current_with_wrapper);

        let expected = match get_readme_content(&id, &current_with_wrapper, project_root) {
            Ok(content) => content,
            Err(e) => {
                errors.push(format!("❌ {}: {}", id, e));
                continue;
            }
        };

        // Compare with trim_lines normalization applied once to each side
        if trim_lines(&current_inner) != trim_lines(&expected) {
            let replacement = format_replacement(&id, &expected, &marker_type.output_format());
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
/// file that serves as inline documentation — users read through, find what they want,
/// and uncomment the relevant `key = value` line.
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
        // [Link text](@/page.md) → Link text (https://worktrunk.dev/page/)
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

    result.join("\n")
}

/// Convert markdown links to plain text with URL in parentheses.
///
/// Config files aren't rendered as markdown, so links need to be readable as plain text.
/// - `[Link text](@/page.md)` → `Link text (https://worktrunk.dev/page/)`
/// - `[Link text](https://example.com)` → `Link text (https://example.com)`
fn convert_markdown_links_for_config(line: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static MARKDOWN_LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());

    MARKDOWN_LINK
        .replace_all(line, |caps: &regex::Captures| {
            let text = &caps[1];
            let url = &caps[2];

            // Convert Zola @/ links to full URLs
            let url = if let Some(path) = url.strip_prefix("@/") {
                let page = path.trim_end_matches(".md");
                format!("https://worktrunk.dev/{page}/")
            } else {
                url.to_string()
            };

            format!("{text} ({url})")
        })
        .to_string()
}

/// Extract user config documentation from src/cli/mod.rs
///
/// The user config section is embedded in mod.rs between USER_CONFIG_START
/// and USER_CONFIG_END markers. This function extracts that content for
/// transforming into config.example.toml.
fn extract_user_config_from_cli(cli_mod_content: &str) -> String {
    USER_CONFIG_PATTERN
        .captures(cli_mod_content)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .expect("USER_CONFIG_START/END markers not found in src/cli/mod.rs")
}

#[test]
fn test_config_source_generates_example_toml() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_mod_path = project_root.join("src/cli/mod.rs");
    let config_path = project_root.join("dev/config.example.toml");

    let cli_mod_content = fs::read_to_string(&cli_mod_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", cli_mod_path.display(), e));

    let user_config_content = extract_user_config_from_cli(&cli_mod_content);
    let expected = transform_config_source_to_toml(&user_config_content);
    let expected = trim_lines(&expected);

    let current = fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", config_path.display(), e));
    let current = trim_lines(&current);

    if current != expected {
        fs::write(&config_path, format!("{}\n", expected)).unwrap();
        panic!(
            "config.example.toml out of sync with user config section in src/cli/mod.rs. \
             Run tests locally and commit the changes."
        );
    }
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
[commit-generation]
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

/// Update help markers in a docs file
/// Uses unified MARKER_PATTERN, processes only help commands (backtick IDs)
fn sync_help_markers(file_path: &Path, project_root: &Path) -> Result<usize, Vec<String>> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| vec![format!("Failed to read {}: {}", file_path.display(), e)])?;

    let mut result = content.clone();
    let mut errors = Vec::new();
    let mut updated = 0;

    // Collect all matches and filter to help commands only
    let matches: Vec<_> = MARKER_PATTERN
        .captures_iter(&content)
        .filter_map(|cap| {
            let id = cap.get(1).unwrap().as_str().trim();
            // Only process help commands (backtick IDs)
            if matches!(MarkerType::from_id(id), MarkerType::Help) {
                let full_match = cap.get(0).unwrap();
                let current = cap.get(2).unwrap().as_str();
                Some((
                    full_match.start(),
                    full_match.end(),
                    id.to_string(),
                    current.to_string(),
                ))
            } else {
                None
            }
        })
        .collect();

    // Process in reverse order
    for (start, end, id, current) in matches.into_iter().rev() {
        let expected = match get_help_output(&id, project_root) {
            Ok(content) => content,
            Err(e) => {
                errors.push(format!("❌ {}: {}", id, e));
                continue;
            }
        };

        if trim_lines(&current) != trim_lines(&expected) {
            let replacement = format_replacement(&id, &expected, &OutputFormat::Unwrapped);
            result.replace_range(start..end, &replacement);
            updated += 1;
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    if updated > 0 {
        fs::write(file_path, &result).unwrap();
    }
    Ok(updated)
}

#[test]
fn test_readme_examples_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_path = project_root.join("README.md");

    let readme_content = fs::read_to_string(&readme_path).unwrap();

    // Single pass handles all marker types (snapshots, help, sections)
    match sync_readme_markers(&readme_content, project_root) {
        Ok((updated_content, updated_count, total_count)) => {
            if total_count == 0 {
                panic!("No README markers found in README.md");
            }

            if updated_count > 0 {
                fs::write(&readme_path, &updated_content).unwrap();
                panic!(
                    "README out of sync: updated {} of {} section(s). \
                     Run tests locally and commit the changes.",
                    updated_count, total_count
                );
            }
        }
        Err(errors) => {
            panic!(
                "README examples are out of sync:\n\n{}\n",
                errors.join("\n")
            );
        }
    }
}

#[test]
fn test_docs_commands_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let commands_path = project_root.join("docs/content/commands.md");

    if !commands_path.exists() {
        // Skip if docs directory doesn't exist
        return;
    }

    match sync_help_markers(&commands_path, project_root) {
        Ok(updated_count) => {
            if updated_count > 0 {
                panic!(
                    "Docs commands out of sync: updated {} section(s) in {}. \
                     Run tests locally and commit the changes.",
                    updated_count,
                    commands_path.display()
                );
            }
        }
        Err(errors) => {
            panic!("Docs commands are out of sync:\n\n{}\n", errors.join("\n"));
        }
    }
}

/// Sync docs snapshot markers in a single file (with ANSI to HTML conversion)
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
        OutputFormat::DocsHtml,
        |snap_path, current_content| {
            // Extract existing command line from docs if present
            let existing_command = current_content
                .lines()
                .find(|line| line.contains("class=\"prompt\""))
                .map(|line| {
                    // Convert HTML command back to plain text for matching
                    // <span class="prompt">$</span> wt switch ... -> $ wt switch ...
                    let plain = strip_html_tags(line);
                    plain.trim().to_string()
                });

            let full_path = project_root_for_snapshots.join(snap_path);
            let raw = fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read {}: {}", full_path.display(), e))?;
            let html_content = parse_snapshot_content_for_docs(&raw)?;
            let normalized = trim_lines(&html_content);

            // Prepend command line with prompt styling if present
            Ok(match existing_command {
                Some(cmd) if cmd.starts_with("$ ") => {
                    let cmd_text = cmd.strip_prefix("$ ").unwrap();
                    format!(
                        "<span class=\"prompt\">$</span> <span class=\"cmd\">{}</span>\n{}",
                        cmd_text, normalized
                    )
                }
                _ => normalized,
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

#[test]
fn test_docs_quickstart_examples_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Process all docs files with AUTO-GENERATED-HTML markers
    let doc_files = [
        "docs/content/worktrunk.md",
        "docs/content/claude-code.md",
        "docs/content/tips-patterns.md",
    ];

    let mut all_errors = Vec::new();
    let mut total_updated = 0;

    for doc_file in doc_files {
        let doc_path = project_root.join(doc_file);
        match sync_docs_snapshots(&doc_path, project_root) {
            Ok(updated) => total_updated += updated,
            Err(errors) => all_errors.extend(errors),
        }
    }

    if !all_errors.is_empty() {
        panic!(
            "Docs examples are out of sync:\n\n{}\n",
            all_errors.join("\n")
        );
    }

    if total_updated > 0 {
        panic!(
            "Docs examples out of sync: updated {} section(s). \
             Run tests locally and commit the changes.",
            total_updated
        );
    }
}

/// Command pages generated via `wt <cmd> --help-page`
/// Each page preserves its frontmatter and replaces the AUTO-GENERATED marker region.
const COMMAND_PAGES: &[&str] = &[
    "switch", "list", "merge", "remove", "select", "config", "step", "hook",
];

/// Sync command pages from --help-page output to docs/content/*.md
/// Returns (errors, updated_files)
fn sync_command_pages(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    for cmd in COMMAND_PAGES {
        let doc_path = project_root.join(format!("docs/content/{}.md", cmd));
        if !doc_path.exists() {
            errors.push(format!("Missing command page: {}", doc_path.display()));
            continue;
        }

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

        // Expand command placeholders ($ wt list -> terminal shortcode with snapshot output)
        let snapshots_dir = project_root.join("tests/snapshots");
        let generated = match expand_command_placeholders(&generated, &snapshots_dir) {
            Ok(expanded) => expanded,
            Err(e) => {
                errors.push(format!(
                    "Failed to expand placeholders for '{}': {}",
                    cmd, e
                ));
                continue;
            }
        };

        // Convert command reference code blocks to terminal shortcodes with HTML
        let generated = match convert_command_reference_to_html(&generated) {
            Ok(converted) => converted,
            Err(e) => {
                errors.push(format!(
                    "Failed to convert command reference for '{}': {}",
                    cmd, e
                ));
                continue;
            }
        };

        let current = fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", doc_path.display(), e));

        // Find the help-page marker region using mirrored END tag
        // Pattern: <!-- ⚠️ AUTO-GENERATED from `wt cmd --help-page` ... --> ... <!-- END AUTO-GENERATED from `wt cmd --help-page` -->
        let marker_pattern = Regex::new(&format!(
            r"(?s)<!-- ⚠️ AUTO-GENERATED from `wt {} --help-page`[^>]*-->.*?<!-- END AUTO-GENERATED from `wt {} --help-page` -->",
            cmd, cmd
        )).unwrap();

        let new_content = if let Some(m) = marker_pattern.find(&current) {
            let before = &current[..m.start()];
            let after = &current[m.end()..];
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
            fs::write(&doc_path, &new_content)
                .unwrap_or_else(|e| panic!("Failed to write {}: {}", doc_path.display(), e));
            updated_files.push(format!("docs/content/{}.md", cmd));
        }
    }

    (errors, updated_files)
}

/// Strip HTML tags from a string (simple implementation for command extraction)
fn strip_html_tags(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

// =============================================================================
// Docs to Skill File Sync
// =============================================================================

/// Regex to match Zola frontmatter and extract title
static ZOLA_FRONTMATTER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\+\+\+\n(.*?)\n\+\+\+\n*").unwrap());

/// Regex to extract title from frontmatter
static ZOLA_TITLE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"title\s*=\s*"([^"]+)""#).unwrap());

/// Regex to strip Zola terminal shortcodes ({% terminal() %}...{% end %})
static ZOLA_TERMINAL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{% terminal\(\) %\}\n?(.*?)\{% end %\}").unwrap());

/// Regex to strip AUTO-GENERATED marker comments (just the comments, not content)
static AUTO_GENERATED_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<!-- ⚠️ AUTO-GENERATED[^>]*-->\n*|<!-- END AUTO-GENERATED[^>]*-->\n*").unwrap()
});

/// Regex to strip HTML figure/picture elements (demo GIFs)
static HTML_FIGURE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<figure[^>]*>.*?</figure>\n*").unwrap());

/// Transform docs content for skill file consumption
///
/// Transforms:
/// - Extracts title from Zola frontmatter and prepends as H1
/// - Strips Zola terminal shortcodes ({% terminal() %}...{% end %}) - keeps inner content
/// - Strips AUTO-GENERATED marker comments (keeps content)
/// - Strips HTML figure elements (demo GIFs not useful for skill)
/// - Converts Zola internal links (@/page.md) -> full URLs
/// - Removes "See also" section (just links to other docs pages)
fn transform_docs_for_skill(content: &str) -> String {
    // Extract title from frontmatter
    let title = ZOLA_FRONTMATTER_PATTERN
        .captures(content)
        .and_then(|caps| caps.get(1))
        .and_then(|fm| ZOLA_TITLE_PATTERN.captures(fm.as_str()))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string());

    // Strip frontmatter
    let content = ZOLA_FRONTMATTER_PATTERN.replace(content, "");

    // Strip terminal shortcodes, keeping inner content
    let content = ZOLA_TERMINAL_PATTERN.replace_all(&content, "$1");

    // Strip AUTO-GENERATED marker comments (keep content)
    let content = AUTO_GENERATED_MARKER_PATTERN.replace_all(&content, "");

    // Strip HTML figure elements (demo GIFs)
    let content = HTML_FIGURE_PATTERN.replace_all(&content, "");

    // Transform Zola internal links to full URLs
    let content = ZOLA_LINK_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            let text = caps.get(1).unwrap().as_str();
            let page = caps.get(2).unwrap().as_str();
            let anchor = caps.get(3).map_or("", |m| m.as_str());
            format!("[{text}](https://worktrunk.dev/{page}/{anchor})")
        })
        .into_owned();

    // Remove "See also" section (just contains links to other pages)
    let content = remove_section(&content, "## See also");

    // Clean up multiple consecutive blank lines
    let content = content
        .lines()
        .fold((Vec::new(), false), |(mut acc, prev_blank), line| {
            let is_blank = line.trim().is_empty();
            if !(is_blank && prev_blank) {
                acc.push(line);
            }
            (acc, is_blank)
        })
        .0
        .join("\n");

    // Prepend title as H1 if extracted
    if let Some(title) = title {
        format!("# {}\n\n{}", title, content.trim())
    } else {
        content.trim().to_string()
    }
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

/// Skill files to sync from docs
/// Format: (docs_path, skill_path)
/// Files are synced with content transformed for skill consumption
const SKILL_SYNC_FILES: &[(&str, &str)] = &[
    (
        "docs/content/hook.md",
        ".claude-plugin/skills/worktrunk/reference/hook.md",
    ),
    (
        "docs/content/config.md",
        ".claude-plugin/skills/worktrunk/reference/config.md",
    ),
    (
        "docs/content/switch.md",
        ".claude-plugin/skills/worktrunk/reference/switch.md",
    ),
    (
        "docs/content/merge.md",
        ".claude-plugin/skills/worktrunk/reference/merge.md",
    ),
    (
        "docs/content/list.md",
        ".claude-plugin/skills/worktrunk/reference/list.md",
    ),
    (
        "docs/content/select.md",
        ".claude-plugin/skills/worktrunk/reference/select.md",
    ),
    (
        "docs/content/step.md",
        ".claude-plugin/skills/worktrunk/reference/step.md",
    ),
    (
        "docs/content/remove.md",
        ".claude-plugin/skills/worktrunk/reference/remove.md",
    ),
    (
        "docs/content/llm-commits.md",
        ".claude-plugin/skills/worktrunk/reference/llm-commits.md",
    ),
    (
        "docs/content/tips-patterns.md",
        ".claude-plugin/skills/worktrunk/reference/tips-patterns.md",
    ),
    (
        "docs/content/worktrunk.md",
        ".claude-plugin/skills/worktrunk/reference/worktrunk.md",
    ),
    (
        "docs/content/faq.md",
        ".claude-plugin/skills/worktrunk/reference/faq.md",
    ),
    (
        "docs/content/claude-code.md",
        ".claude-plugin/skills/worktrunk/reference/claude-code.md",
    ),
];

/// Sync skill files from docs/content/*.md to .claude-plugin/skills/worktrunk/reference/*.md
/// Returns (errors, updated_files)
fn sync_skill_files(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    for (docs_path, skill_path) in SKILL_SYNC_FILES {
        let docs_file = project_root.join(docs_path);
        let skill_file = project_root.join(skill_path);

        if !docs_file.exists() {
            errors.push(format!("Missing docs file: {}", docs_file.display()));
            continue;
        }

        let docs_content = fs::read_to_string(&docs_file)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", docs_file.display(), e));

        // Transform and use content directly (docs already have proper H1 titles)
        let expected = transform_docs_for_skill(&docs_content);
        let expected = trim_lines(&expected);

        let current = if skill_file.exists() {
            fs::read_to_string(&skill_file)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", skill_file.display(), e))
        } else {
            String::new()
        };
        let current = trim_lines(&current);

        if current != expected {
            // Ensure parent directory exists
            if let Some(parent) = skill_file.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|e| {
                    panic!("Failed to create directory {}: {}", parent.display(), e)
                });
            }
            fs::write(&skill_file, format!("{}\n", expected))
                .unwrap_or_else(|e| panic!("Failed to write {}: {}", skill_file.display(), e));
            updated_files.push(skill_path.to_string());
        }
    }

    (errors, updated_files)
}

/// Combined test: sync command pages (mod.rs → docs) then skill files (docs → skills)
/// This ensures a single test run handles the full chain when mod.rs changes.
#[test]
fn test_command_pages_and_skill_files_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Step 1: Sync command pages (mod.rs → docs/content/*.md)
    let (cmd_errors, cmd_files) = sync_command_pages(project_root);

    // Step 2: Sync skill files (docs/content/*.md → .claude-plugin/skills/*)
    // This reads the freshly-written docs from step 1
    let (skill_errors, skill_files) = sync_skill_files(project_root);

    // Aggregate results
    let all_errors: Vec<_> = cmd_errors.into_iter().chain(skill_errors).collect();
    let all_files: Vec<_> = cmd_files.into_iter().chain(skill_files).collect();

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
