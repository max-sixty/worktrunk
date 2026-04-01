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
/// Matches both old `{% terminal() %}` and new `{% terminal(cmd="...") %}` forms
static DOCS_SNAPSHOT_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<!-- ⚠️ AUTO-GENERATED-HTML from ([^\s]+\.snap) — edit source to update -->\n+\{% terminal\([^)]*\) %\}\n(.*?)\{% end %\}\n+<!-- END AUTO-GENERATED -->"#,
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
/// - `_REPO_` → `repo` (just the repo name, no path)
/// - `_REPO_.branch` → `repo.branch`
fn replace_placeholders(content: &str) -> String {
    let content = HASH_REGEX.replace_all(content, "a1b2c3d");
    let content = TMPDIR_BRANCH_REGEX.replace_all(&content, "../repo.$1");
    let content = TMPDIR_MAIN_REGEX.replace_all(&content, "../repo$1");
    let content = REPO_REGEX.replace_all(&content, "../repo");
    // Handle _REPO_.branch -> repo.branch and _REPO_ -> repo
    REPO_UNDERSCORE_REGEX
        .replace_all(&content, |caps: &regex::Captures| {
            if let Some(branch) = caps.get(2) {
                format!("repo.{}", branch.as_str())
            } else {
                "repo".to_string()
            }
        })
        .into_owned()
}

/// Format replacement content based on output format
fn format_replacement(id: &str, content: &str, format: &OutputFormat) -> String {
    match format {
        OutputFormat::DocsHtml => {
            // Extract command from <span class="cmd"> in body to also emit as cmd= parameter
            // The cmd= parameter enables giallo syntax highlighting in the shortcode
            // The span is kept in body for stable sync comparisons
            let cmd_re = Regex::new(r#"^<span class="cmd">([^<]+)</span>"#).unwrap();
            let cmd_attr = cmd_re
                .captures(content)
                .map(|c| format!(r#"cmd="{}""#, c.get(1).unwrap().as_str()))
                .unwrap_or_default();
            format!(
                "<!-- ⚠️ AUTO-GENERATED-HTML from {} — edit source to update -->\n\n{{% terminal({}) %}}\n{}\n{{% end %}}\n\n<!-- END AUTO-GENERATED -->",
                id, cmd_attr, content
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
/// Matches: <!-- wt <args> -->\n```bash\nwt <args>\n```
/// The HTML comment triggers expansion, the code block shows in terminal help
/// Note: Pattern expects ```bash``` because --help-page converts ```console``` first
static COMMAND_PLACEHOLDER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!-- (wt [^>]+) -->\n```bash\nwt [^\n]+\n```").unwrap());

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
        let normalized = encode_leading_spaces(&trim_lines(&html));

        // Build the terminal shortcode with standard template markers
        // cmd= parameter enables giallo syntax highlighting on the command line
        // Prompt ($) is added via CSS ::before, so not included in HTML
        let replacement = format!(
            "<!-- ⚠️ AUTO-GENERATED from tests/snapshots/{} — edit source to update -->\n\n\
             {{% terminal(cmd=\"{}\") %}}\n\
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

/// Encode leading spaces on the first line as `&#32;` HTML entities.
/// Zola trims leading whitespace from shortcode bodies, stripping the
/// two-space gutter that aligns table headers with data rows in `wt list`.
/// HTML entities survive the trim and render as spaces in `<pre>` blocks.
fn encode_leading_spaces(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("");
    let leading = first_line.len() - first_line.trim_start().len();
    if leading == 0 {
        return content.to_string();
    }
    format!("{}{}", "&#32;".repeat(leading), &content[leading..])
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
    ensure_line_resets_impl(ansi, false)
}

/// Like `ensure_line_resets`, but also carries active styles to the next line
///
/// Clap resets styles at line breaks when wrapping, so a bold span that wraps across
/// lines loses its bold on the continuation. This variant tracks active SGR styles
/// and re-opens them at the start of each continuation line, producing clean per-line
/// HTML like `<b>first part</b>\n<b>second part</b>` instead of `<b>first part</b>\nsecond part`.
fn ensure_line_resets_with_carry(ansi: &str) -> String {
    ensure_line_resets_impl(ansi, true)
}

fn ensure_line_resets_impl(ansi: &str, carry_styles: bool) -> String {
    const RESET: &str = "\x1b[0m";
    // Match SGR sequences: ESC [ <params> m
    static SGR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[([0-9;]*)m").unwrap());

    let lines: Vec<&str> = ansi.lines().collect();
    let mut result = Vec::with_capacity(lines.len());
    let mut active_styles: Vec<String> = Vec::new();

    for line in lines {
        // Prepend active styles from previous line (only when carrying)
        let line = if !carry_styles || active_styles.is_empty() {
            line.to_string()
        } else {
            let prefix: String = active_styles.iter().map(|s| s.as_str()).collect();
            format!("{prefix}{line}")
        };

        // Track which styles are active at end of this line
        active_styles.clear();
        for cap in SGR_RE.captures_iter(&line) {
            let params = &cap[1];
            if params.is_empty() || params == "0" {
                active_styles.clear();
            } else {
                active_styles.push(format!("\x1b[{params}m"));
            }
        }

        // Ensure line ends with reset
        if line.ends_with(RESET) {
            result.push(line);
        } else {
            result.push(format!("{line}{RESET}"));
        }
    }

    result.join("\n")
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
        let with_resets = ensure_line_resets_with_carry(code_content);
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

/// Regex to match terminal shortcodes with AUTO-GENERATED-HTML markers
/// Optionally captures a preceding bash code block (which becomes redundant)
/// These need to be converted to plain code blocks for README
/// Matches both `{% terminal() %}` and `{% terminal(cmd="...") %}` forms
static TERMINAL_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)(?:```bash\n[^\n]+\n```\n+)?<!-- ⚠️ AUTO-GENERATED-HTML from [^\n]+ -->\n+\{% terminal\([^)]*\) %\}\n(.*?)\{% end %\}\n+<!-- END AUTO-GENERATED -->"#,
    )
    .unwrap()
});

/// Strip HTML tags from content, converting .cmd spans to `$ ` prefixed commands
fn strip_html(content: &str) -> String {
    // First, convert <span class="cmd">...</span> to "$ ..." (add prompt)
    let cmd_pattern = Regex::new(r#"<span class="cmd">([^<]*)</span>"#).unwrap();
    let result = cmd_pattern.replace_all(content, "$ $1");

    // Strip remaining HTML tags
    let tag_pattern = Regex::new(r"<[^>]+>").unwrap();
    let result = tag_pattern.replace_all(&result, "");

    // Decode HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Transform Zola-flavored markdown to GitHub-flavored markdown
///
/// Converts:
/// - `[text](@/page.md)` → `[text](https://worktrunk.dev/page/)`
/// - `[text](@/page.md#anchor)` → `[text](https://worktrunk.dev/page/#anchor)`
/// - `{% rawcode() %}...{% end %}` → `<pre>...</pre>`
/// - `<figure class="demo">...<img src="/assets/X.gif"...>...</figure>` → `![alt](raw.githubusercontent.com/.../X.gif)`
/// - AUTO-GENERATED-HTML terminal markers → plain code blocks
/// - `{{ terminal(cmd="...") }}` → ```bash code blocks
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

    // Transform terminal markers to console code blocks for README
    let content = TERMINAL_MARKER_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            let inner = caps.get(1).unwrap().as_str();
            // Strip HTML, converting .cmd spans to "$ ..." (adds prompt)
            let plain = strip_html(inner);
            format!("```console\n{}\n```", plain)
        })
        .into_owned();

    // Transform self-closing terminal shortcodes to bash code blocks for README
    // These are `{{ terminal(cmd="...") }}` shortcodes without body content
    let content = ZOLA_TERMINAL_SELF_CLOSING_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            cmd_to_bash_block(caps.get(1).map_or("", |m| m.as_str()), "")
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
fn docs_section_for_readme(id: &str, project_root: &Path) -> Result<String, String> {
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

    // Process in reverse order to preserve positions
    for (start, end, id, current_with_wrapper) in matches.into_iter().rev() {
        let marker_type = MarkerType::from_id(&id);

        // Strip wrapper from current content (snapshots have ```console```, others are raw)
        let current_inner = marker_type.extract_inner(&current_with_wrapper);

        let expected = match generate_readme_content(&id, &current_with_wrapper, project_root) {
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
                // Handle anchors: @/config.md#section → config/#section
                let (page, anchor) = match path.split_once('#') {
                    Some((p, a)) => (p.trim_end_matches(".md"), Some(a)),
                    None => (path.trim_end_matches(".md"), None),
                };
                match anchor {
                    Some(a) => format!("https://worktrunk.dev/{page}/#{a}"),
                    None => format!("https://worktrunk.dev/{page}/"),
                }
            } else {
                url.to_string()
            };

            format!("{text} ({url})")
        })
        .to_string()
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

    // Hook keys from HookType enum + deprecated post-create (not in enum)
    let hook_keys: HashSet<String> = HookType::iter()
        .map(|h| h.to_string())
        .chain(std::iter::once("post-create".to_string()))
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
        if DEPRECATED_SECTION_KEYS.contains(&key.as_str()) {
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

    // Hook keys from HookType enum + deprecated post-create (not in enum)
    let hook_keys: HashSet<String> = HookType::iter()
        .map(|h| h.to_string())
        .chain(std::iter::once("post-create".to_string()))
        .collect();

    // Separate schema keys into section keys and hook keys
    let section_keys: Vec<&String> = all_keys
        .iter()
        .filter(|k| !hook_keys.contains(*k))
        .collect();

    // Check non-deprecated sections appear as TOML headers ([key] or [key.something])
    for key in &section_keys {
        if DEPRECATED_SECTION_KEYS.contains(&key.as_str()) {
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

/// Verify that LLM tool commands in docs/content/llm-commits.md match
/// the examples in config.example.toml (the single source of truth).
#[test]
fn test_llm_docs_commands_match_config_example() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config_example = fs::read_to_string(project_root.join("dev/config.example.toml")).unwrap();
    let llm_docs = fs::read_to_string(project_root.join("docs/content/llm-commits.md")).unwrap();

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
            "Command from config.example.toml not found in docs/content/llm-commits.md:\n  {cmd}\n\
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
        let expected = match help_output(&id, project_root) {
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
        |snap_path, _current_content| {
            let full_path = project_root_for_snapshots.join(snap_path);
            let raw = fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read {}: {}", full_path.display(), e))?;

            // Extract command from snapshot YAML header
            let command = extract_command_from_snapshot(&raw);

            let html_content = parse_snapshot_content_for_docs(&raw)?;
            let normalized = trim_lines(&html_content);

            // Prepend command line with styling if present
            // Prompt ($) is added via CSS ::before, so not included in HTML
            Ok(match command {
                Some(cmd) => format!("<span class=\"cmd\">{}</span>\n{}", cmd, normalized),
                None => normalized,
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

/// Update or insert the `description` field in TOML frontmatter.
///
/// Handles three cases:
/// - Description field exists → update it
/// - No description field → insert after title line
/// - No frontmatter → return content unchanged
fn sync_frontmatter_description(content: &str, description: &str) -> String {
    static DESC_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^description\s*=\s*"[^"]*""#).unwrap());

    let new_field = format!("description = \"{}\"", description.replace('"', "\\\""));

    // Check if we're in a TOML frontmatter block
    if !content.starts_with("+++\n") {
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
            LazyLock::new(|| Regex::new(r#"(?m)^(title\s*=\s*"[^"]*")\n"#).unwrap());

        TITLE_PATTERN
            .replace(content, |caps: &regex::Captures| {
                format!("{}\n{}\n", &caps[1], new_field)
            })
            .to_string()
    }
}

/// Command pages generated via `wt <cmd> --help-page`
/// Each page preserves its frontmatter and replaces the AUTO-GENERATED marker region.
/// Note: `select` is excluded because it's a deprecated hidden alias for `wt switch`.
const COMMAND_PAGES: &[&str] = &[
    "switch", "list", "merge", "remove", "config", "step", "hook",
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

        // Expand command placeholders (wt list -> terminal shortcode with snapshot output)
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

        // Find the help-page marker region using mirrored END tag
        // Pattern: <!-- ⚠️ AUTO-GENERATED from `wt cmd --help-page` ... --> ... <!-- END AUTO-GENERATED from `wt cmd --help-page` -->
        let marker_pattern = Regex::new(&format!(
            r"(?s)<!-- ⚠️ AUTO-GENERATED from `wt {} --help-page`[^>]*-->.*?<!-- END AUTO-GENERATED from `wt {} --help-page` -->",
            cmd, cmd
        )).unwrap();

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
            fs::write(&doc_path, &new_content)
                .unwrap_or_else(|e| panic!("Failed to write {}: {}", doc_path.display(), e));
            updated_files.push(format!("docs/content/{}.md", cmd));
        }
    }

    (errors, updated_files)
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

/// Regex to strip body-form terminal shortcodes ({% terminal(...) %}...{% end %}).
/// Optionally captures the cmd parameter value (group 1) and body (group 2).
static ZOLA_TERMINAL_BODY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)\{%\s*terminal\((?:cmd="([^"]*)"\s*)?\)\s*%\}\n?(.*?)\{%\s*end\s*%\}"#)
        .unwrap()
});

/// Regex to strip self-closing terminal shortcodes ({{ terminal(cmd="...") }}).
static ZOLA_TERMINAL_SELF_CLOSING_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{\{ terminal\(cmd="([^"]*)"\) \}\}"#).unwrap());

/// Regex to replace Zola experimental shortcode with plain text for skill files
static ZOLA_EXPERIMENTAL_SHORTCODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*experimental\(\)\s*\}\}").unwrap());

/// Regex to strip AUTO-GENERATED marker comments (just the comments, not content)
static AUTO_GENERATED_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<!-- ⚠️ AUTO-GENERATED[^>]*-->\n*|<!-- END AUTO-GENERATED[^>]*-->\n*").unwrap()
});

/// Regex to strip HTML figure/picture elements (demo GIFs)
static HTML_FIGURE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<figure[^>]*>.*?</figure>\n*").unwrap());

/// Regex to strip `<span class="cmd">...</span>` lines from shortcode bodies.
/// These duplicate the cmd parameter content (the template uses `clean_body` for this).
static SPAN_CMD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<span class="cmd">[^<]*</span>\n?"#).unwrap());

/// Regex to convert `<span class="cmd">X</span>` → `$ X` for no-cmd body shortcodes.
static SPAN_CMD_TO_DOLLAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<span class="cmd">([^<]*)</span>"#).unwrap());

/// Convert a `|||`-delimited cmd string (and optional body) into a ```bash block.
/// Each cmd line gets `$ ` prefix; comment lines (`#`) and blank lines pass through.
fn cmd_to_bash_block(cmd: &str, body: &str) -> String {
    let mut result = String::from("```bash\n");
    for line in cmd.split("|||") {
        if line.is_empty() {
            result.push('\n');
        } else if line.starts_with('#') {
            result.push_str(line);
            result.push('\n');
        } else {
            result.push_str("$ ");
            result.push_str(line);
            result.push('\n');
        }
    }
    // Strip <span class="cmd"> lines that duplicate the cmd parameter
    let clean_body = SPAN_CMD_PATTERN.replace_all(body, "");
    if !clean_body.is_empty() {
        result.push_str(&clean_body);
        if !clean_body.ends_with('\n') {
            result.push('\n');
        }
    }
    result.push_str("```");
    result
}

/// Transform docs content for skill file consumption
///
/// Transforms:
/// - Extracts title from Zola frontmatter and prepends as H1
/// - Strips Zola terminal shortcodes ({% terminal() %}...{% end %}) - keeps inner content
/// - Strips AUTO-GENERATED marker comments (keeps content)
/// - Strips HTML figure elements (demo GIFs not useful for skill)
/// - Replaces Zola shortcodes with plain text equivalents
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

    // Strip terminal shortcodes, converting cmd parameters back to `$ command` blocks.
    // Commands joined by `|||` are split into separate lines.
    let content = ZOLA_TERMINAL_BODY_PATTERN.replace_all(&content, |caps: &regex::Captures| {
        let body = caps.get(2).map_or("", |m| m.as_str());
        match caps.get(1) {
            Some(cmd) => cmd_to_bash_block(cmd.as_str(), body),
            None if body.contains(r#"<span class="cmd">"#) => {
                // Old-style body shortcode with <span class="cmd"> — convert to $ lines
                let converted = SPAN_CMD_TO_DOLLAR.replace_all(body, "$$ $1");
                format!("```bash\n{converted}```")
            }
            // Terminal bodies without cmd are styled CLI output demos (colored
            // spans, bold text). Strip HTML to plain text for skill files.
            None => strip_html(body),
        }
    });
    let content = ZOLA_TERMINAL_SELF_CLOSING_PATTERN
        .replace_all(&content, |caps: &regex::Captures| {
            cmd_to_bash_block(caps.get(1).map_or("", |m| m.as_str()), "")
        });

    // Strip rawcode shortcodes (keep content)
    let content = ZOLA_RAWCODE_PATTERN.replace_all(&content, "$1");

    // Replace placeholders used to escape Tera template syntax in cmd parameters
    let content = content
        .replace("__WT_OPEN2__", "{{")
        .replace("__WT_CLOSE2__", "}}")
        .replace("__WT_QUOT__", "\"");

    // Strip AUTO-GENERATED marker comments (keep content)
    let content = AUTO_GENERATED_MARKER_PATTERN.replace_all(&content, "");

    // Strip HTML figure elements (demo GIFs)
    let content = HTML_FIGURE_PATTERN.replace_all(&content, "");

    // Replace experimental markers (shortcode and HTML badge) with plain text
    let content = ZOLA_EXPERIMENTAL_SHORTCODE.replace_all(&content, "[experimental]");
    let content = content.replace(
        "<span class=\"badge-experimental\"></span>",
        "[experimental]",
    );

    // Prepend title as H1 if extracted
    let content = if let Some(title) = title {
        format!("# {}\n\n{}", title, content.trim())
    } else {
        content.trim().to_string()
    };

    // Apply shared finalization: Zola links, See also removal, blank line cleanup
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

/// Convert ```console blocks with $ to terminal shortcodes in all docs files.
///
/// Command pages already have this conversion via --help-page, but hand-written
/// docs (faq.md, llm-commits.md, claude-code.md) can also use ```console with $
/// and get the same treatment.
fn convert_console_blocks_in_docs(project_root: &Path) -> Vec<String> {
    let docs_dir = project_root.join("docs/content");
    let mut updated_files = Vec::new();

    for entry in fs::read_dir(&docs_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            let content = fs::read_to_string(&path).unwrap();
            let converted = worktrunk::docs::convert_dollar_console_to_terminal(&content);
            if converted != content {
                fs::write(&path, &converted).unwrap();
                let rel = path.strip_prefix(project_root).unwrap_or(&path);
                updated_files.push(rel.display().to_string());
            }
        }
    }

    updated_files
}

/// Sync all docs/content/*.md files to skills/worktrunk/reference/*.md
/// (excluding _index.md which is a Zola template)
/// Returns (errors, updated_files)
fn sync_skill_files(project_root: &Path) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut updated_files = Vec::new();

    let docs_dir = project_root.join("docs/content");
    let skill_dir = project_root.join("skills/worktrunk/reference");

    let mut entries: Vec<_> = fs::read_dir(&docs_dir)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", docs_dir.display(), e))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !name.starts_with('_') {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    entries.sort();

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
            // Non-command pages: read from docs, transform Zola syntax, strip residual HTML
            let docs_file = docs_dir.join(name);
            if !docs_file.exists() {
                errors.push(format!("Missing docs file: {}", docs_file.display()));
                continue;
            }
            let docs_content = fs::read_to_string(&docs_file)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", docs_file.display(), e));
            transform_docs_for_skill(&docs_content)
        };
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
            updated_files.push(format!("skills/worktrunk/reference/{name}"));
        }
    }

    (errors, updated_files)
}

/// Generate a skill reference file directly from `--help-page --plain` output.
///
/// For command pages, this produces clean markdown without HTML. The only
/// post-processing needed is Zola link transformation and section cleanup.
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

    Ok(finalize_skill_content(&content))
}

/// Apply final transforms shared between command and non-command skill files:
/// Zola internal links → full URLs, remove "See also" section, collapse blank lines.
fn finalize_skill_content(content: &str) -> String {
    // Transform Zola internal links to full URLs
    let content = ZOLA_LINK_PATTERN
        .replace_all(content, |caps: &regex::Captures| {
            let text = caps.get(1).unwrap().as_str();
            let page = caps.get(2).unwrap().as_str();
            let anchor = caps.get(3).map_or("", |m| m.as_str());
            format!("[{text}](https://worktrunk.dev/{page}/{anchor})")
        })
        .into_owned();

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

/// Sync .well-known/agent-skills/ index.json and verify symlink.
///
/// The skill files are served via a symlink:
///   docs/static/.well-known/agent-skills/worktrunk → ../../../../skills/worktrunk
///
/// This function verifies the symlink is correct and generates index.json
/// with the correct SHA-256 digest per the Cloudflare agent-skills-discovery RFC.
fn sync_well_known_skills(project_root: &Path) -> Vec<String> {
    let mut updated_files = Vec::new();

    let well_known_dir = project_root.join("docs/static/.well-known/agent-skills");
    let symlink_path = well_known_dir.join("worktrunk");

    // Verify the symlink exists and points to the right place
    let expected_target = Path::new("../../../../skills/worktrunk");
    match fs::read_link(&symlink_path) {
        Ok(target) => {
            assert_eq!(
                target,
                expected_target,
                "Symlink at {} points to {:?}, expected {:?}",
                symlink_path.display(),
                target,
                expected_target
            );
        }
        Err(_) => {
            panic!(
                "Expected symlink at {} → {:?}, but it doesn't exist or isn't a symlink",
                symlink_path.display(),
                expected_target
            );
        }
    }

    // Read SKILL.md (through the symlink) for digest and description
    let skill_md_path = symlink_path.join("SKILL.md");
    let skill_md_content = fs::read_to_string(&skill_md_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", skill_md_path.display(), e));

    // Generate index.json with SHA-256 digest of SKILL.md
    let digest = {
        use sha2::{Digest, Sha256};
        let file_bytes = fs::read(&skill_md_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", skill_md_path.display(), e));
        let hash = Sha256::digest(&file_bytes);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256:{hex}")
    };

    // Parse the description from SKILL.md frontmatter
    let description = skill_md_content
        .strip_prefix("---\n")
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
        fs::write(&index_dst, &index_json)
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", index_dst.display(), e));
        updated_files.push("docs/static/.well-known/agent-skills/index.json".to_string());
    }

    updated_files
}

/// Combined test: sync command pages (mod.rs → docs) then skill files (docs → skills)
/// then .well-known/agent-skills/ (skills → docs/static).
/// This ensures a single test run handles the full chain when mod.rs changes.
#[test]
fn test_command_pages_and_skill_files_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Step 1: Sync command pages (mod.rs → docs/content/*.md)
    let (cmd_errors, cmd_files) = sync_command_pages(project_root);

    // Step 1b: Convert $ console blocks to terminal shortcodes in ALL docs
    // (command pages already converted via --help-page; this catches hand-written docs)
    let console_files = convert_console_blocks_in_docs(project_root);

    // Step 2: Sync skill files (docs/content/*.md → skills/*)
    // This reads the freshly-written docs from step 1
    let (skill_errors, skill_files) = sync_skill_files(project_root);

    // Step 3: Sync .well-known/agent-skills/ (skills/ → docs/static/)
    // This reads the freshly-written skills from step 2
    let well_known_files = sync_well_known_skills(project_root);

    // Aggregate results
    let all_errors: Vec<_> = cmd_errors.into_iter().chain(skill_errors).collect();
    let all_files: Vec<_> = cmd_files
        .into_iter()
        .chain(console_files)
        .chain(skill_files)
        .chain(well_known_files)
        .collect();

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

/// Verify that post_process_for_html() transforms the approval prompt code block
/// into a styled terminal shortcode. If the source text in cli/mod.rs changes
/// without updating the replacement in help.rs, the .replace() silently stops
/// matching and the web docs fall back to a plain code block.
#[test]
fn test_approval_prompt_styled_in_hook_page() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = wt_command()
        .args(["hook", "--help-page"])
        .current_dir(project_root)
        .output()
        .expect("Failed to run wt hook --help-page");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#"class="y""#),
        "hook --help-page should contain styled approval prompt (class=\"y\" for yellow ▲). \
         If cli/mod.rs approval example changed, update the replacement in help.rs post_process_for_html()."
    );
}
