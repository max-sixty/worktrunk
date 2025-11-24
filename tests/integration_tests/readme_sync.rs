//! README and config synchronization tests
//!
//! Verifies that README.md examples stay in sync with their source snapshots and help output.
//! Also syncs default templates from src/llm.rs to dev/config.example.toml.
//! Automatically updates sections when out of sync.
//!
//! Run with: `cargo test --test integration readme_sync`

use regex::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

/// Regex to find README snapshot markers
static SNAPSHOT_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<!-- README:snapshot:([^\s]+) -->\n```\w*\n(?:\$ [^\n]+\n)?(.*?)```\n<!-- README:end -->",
    )
    .unwrap()
});

/// Regex to find README help markers (no wrapper - content is rendered markdown)
static HELP_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- README:help:(.+?) -->\n(.*?)\n<!-- README:end -->").unwrap()
});

/// Regex to strip ANSI escape codes (actual escape sequences)
static ANSI_ESCAPE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());

/// Regex to strip literal bracket notation (as stored in snapshots)
static ANSI_LITERAL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[0-9;]*m").unwrap());

/// Regex for SHA placeholder
static SHA_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[SHA\]").unwrap());

/// Regex for HASH placeholder (used by shell_wrapper tests)
static HASH_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[HASH\]").unwrap());

/// Regex for TMPDIR paths
static TMPDIR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[TMPDIR\]/test-repo\.([^\s/]+)").unwrap());

/// Regex for REPO placeholder
static REPO_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[REPO\]").unwrap());

/// Regex to find DEFAULT_TEMPLATE marker
static DEFAULT_TEMPLATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(# <!-- DEFAULT_TEMPLATE_START -->\n).*?(# <!-- DEFAULT_TEMPLATE_END -->)")
        .unwrap()
});

/// Regex to find DEFAULT_SQUASH_TEMPLATE marker
static SQUASH_TEMPLATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)(# <!-- DEFAULT_SQUASH_TEMPLATE_START -->\n).*?(# <!-- DEFAULT_SQUASH_TEMPLATE_END -->)",
    )
    .unwrap()
});

/// Regex to extract Rust raw string constants (single pound)
static RUST_RAW_STRING_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##"(?s)const (DEFAULT_TEMPLATE|DEFAULT_SQUASH_TEMPLATE): &str = r#"(.*?)"#;"##)
        .unwrap()
});

/// Strip ANSI escape codes from text
fn strip_ansi(text: &str) -> String {
    let text = ANSI_ESCAPE_REGEX.replace_all(text, "");
    ANSI_LITERAL_REGEX.replace_all(&text, "").to_string()
}

/// Parse content from an insta snapshot file
fn parse_snapshot(path: &Path) -> Result<String, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    // Remove YAML front matter
    let content = if content.starts_with("---") {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() >= 3 {
            parts[2].trim().to_string()
        } else {
            content
        }
    } else {
        content
    };

    // Handle insta_cmd format with stdout/stderr sections
    let content = if content.contains("----- stdout -----") {
        // Extract stdout section
        let stdout = if let Some(start) = content.find("----- stdout -----\n") {
            let after_header = &content[start + "----- stdout -----\n".len()..];
            if let Some(end) = after_header.find("----- stderr -----") {
                after_header[..end].trim_end().to_string()
            } else {
                after_header.trim_end().to_string()
            }
        } else {
            String::new()
        };

        // Extract stderr section
        let stderr = if let Some(start) = content.find("----- stderr -----\n") {
            let after_header = &content[start + "----- stderr -----\n".len()..];
            if let Some(end) = after_header.find("----- ") {
                after_header[..end].trim_end().to_string()
            } else {
                after_header.trim_end().to_string()
            }
        } else {
            String::new()
        };

        // Use stdout if it has content, otherwise stderr
        if !stdout.is_empty() { stdout } else { stderr }
    } else {
        content
    };

    // Strip ANSI codes
    Ok(strip_ansi(&content))
}

/// Normalize snapshot output for README display
fn normalize_for_readme(content: &str) -> String {
    let content = SHA_REGEX.replace_all(content, "a1b2c3d");
    let content = HASH_REGEX.replace_all(&content, "a1b2c3d");
    let content = TMPDIR_REGEX.replace_all(&content, "../repo.$1");
    let content = REPO_REGEX.replace_all(&content, "../repo");

    // Trim trailing whitespace from each line and overall (matches pre-commit behavior)
    content
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Normalize README content for comparison
fn normalize_readme_content(content: &str) -> String {
    content
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Get help output for a command
///
/// Expected format: `wt <subcommand> --help-md`
fn get_help_output(command: &str, project_root: &Path) -> Result<String, String> {
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

    // Build the cargo run command
    let output = Command::new("cargo")
        .args(["run", "-q", "--"])
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

    // Strip ANSI codes
    let help_output = strip_ansi(&help_output);

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
    let result = if let Some(first_newline) = help_output.find('\n') {
        let (first_line, rest) = help_output.split_at(first_newline);
        // Replace hyphen-minus with em dash in command description
        let first_line = first_line.replacen(" - ", " — ", 1);

        if let Some(header_pos) = rest.find("\n## ") {
            // Split at first H2 header
            let (synopsis, docs) = rest.split_at(header_pos);
            let docs = docs.trim_start_matches('\n');
            format!("```text\n{}{}\n```\n\n{}", first_line, synopsis, docs)
        } else {
            // No documentation section, wrap everything in code block
            format!("```text\n{}{}\n```", first_line, rest)
        }
    } else {
        // Single line output
        help_output.replacen(" - ", " — ", 1)
    };

    Ok(result)
}

/// Update a section in the README content, returning (new content, updated count, total count)
fn update_readme_section(
    content: &str,
    pattern: &Regex,
    get_replacement: impl Fn(&str) -> Result<String, String>,
    wrapper: (&str, &str),
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
            let current = normalize_readme_content(cap.get(2).unwrap().as_str());
            (full_match.start(), full_match.end(), id, current)
        })
        .collect();

    let total = matches.len();

    // Process in reverse order to preserve positions
    for (start, end, id, current) in matches.into_iter().rev() {
        let expected = match get_replacement(&id) {
            Ok(content) => content,
            Err(e) => {
                errors.push(format!("❌ {}: {}", id, e));
                continue;
            }
        };

        if current != expected {
            // Build replacement
            let replacement = if wrapper.0.is_empty() {
                // No wrapper (help sections - rendered markdown)
                format!(
                    "<!-- README:help:{} -->\n{}\n<!-- README:end -->",
                    id, expected
                )
            } else {
                // With wrapper (snapshot sections)
                format!(
                    "<!-- README:snapshot:{} -->\n{}\n{}\n{}\n<!-- README:end -->",
                    id, wrapper.0, expected, wrapper.1
                )
            };
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

/// Convert a template to commented TOML format
fn comment_template(template: &str) -> String {
    template
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::from("#")
            } else {
                format!("# {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

#[test]
fn test_config_example_templates_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let llm_rs_path = project_root.join("src/llm.rs");
    let config_path = project_root.join("dev/config.example.toml");

    let llm_content = fs::read_to_string(&llm_rs_path).unwrap();
    let config_content = fs::read_to_string(&config_path).unwrap();

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

    let mut updated_content = config_content.clone();
    let mut updated_count = 0;

    // Helper to replace a template section
    let mut replace_template = |pattern: &Regex, name: &str, key: &str| {
        if let Some(cap) = pattern.captures(&updated_content.clone()) {
            let full_match = cap.get(0).unwrap();
            let prefix = cap.get(1).unwrap().as_str();
            let suffix = cap.get(2).unwrap().as_str();

            let template = templates
                .get(name)
                .unwrap_or_else(|| panic!("{name} not found in src/llm.rs"));
            let commented = comment_template(template);

            let replacement = format!(
                r#"{prefix}# {key} = """
{commented}
# """
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
        fs::write(&config_path, &updated_content).unwrap();
        println!(
            "✅ Updated {} template sections in config.example.toml",
            updated_count
        );
    }
}

#[test]
fn test_readme_examples_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_path = project_root.join("README.md");

    let readme_content = fs::read_to_string(&readme_path).unwrap();

    let mut errors = Vec::new();
    let mut checked = 0;
    let mut updated_content = readme_content.clone();
    let mut total_updated = 0;

    // Check snapshot markers
    for cap in SNAPSHOT_MARKER_PATTERN.captures_iter(&readme_content) {
        let snap_path = cap.get(1).unwrap().as_str();
        let current_content = normalize_readme_content(cap.get(2).unwrap().as_str());
        checked += 1;

        let full_path = project_root.join(snap_path);

        // Parse and normalize the snapshot
        let expected = match parse_snapshot(&full_path) {
            Ok(content) => normalize_for_readme(&content),
            Err(e) => {
                errors.push(format!("❌ {}: {}", snap_path, e));
                continue;
            }
        };

        // Compare
        if current_content != expected {
            errors.push(format!(
                "❌ {} is out of sync\n\n--- Current (in README) ---\n{}\n\n--- Expected (from snapshot) ---\n{}\n",
                snap_path, current_content, expected
            ));
        }
    }

    // Update help markers (no wrapper - content is rendered markdown)
    let project_root_clone = project_root.to_path_buf();
    match update_readme_section(
        &updated_content,
        &HELP_MARKER_PATTERN,
        |cmd| get_help_output(cmd, &project_root_clone),
        ("", ""),
    ) {
        Ok((new_content, updated_count, total_count)) => {
            updated_content = new_content;
            total_updated += updated_count;
            checked += total_count;
        }
        Err(errs) => errors.extend(errs),
    }

    if checked == 0 {
        panic!("No README markers found in README.md");
    }

    // Write updates
    if total_updated > 0 {
        fs::write(&readme_path, &updated_content).unwrap();
        println!("✅ Updated {} sections in README.md", total_updated);
    }

    if !errors.is_empty() {
        panic!(
            "README examples are out of sync:\n\n{}\n\n\
            Checked {} markers, {} errors.",
            errors.join("\n"),
            checked,
            errors.len()
        );
    }
}
