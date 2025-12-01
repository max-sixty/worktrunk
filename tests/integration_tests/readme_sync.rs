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
/// Format: <!-- ⚠️ AUTO-GENERATED from path.snap — edit source to update -->
static SNAPSHOT_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<!-- ⚠️ AUTO-GENERATED from ([^\s]+\.snap) — edit source to update -->\n+```\w*\n(.*?)```\n+<!-- END AUTO-GENERATED -->",
    )
    .unwrap()
});

/// Regex to find README help markers (no wrapper - content is rendered markdown)
/// Format: <!-- ⚠️ AUTO-GENERATED from `wt command --help-md` — edit source to update -->
static HELP_MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!-- ⚠️ AUTO-GENERATED from `([^`]+)` — edit source to update -->\n+(.*?)\n+<!-- END AUTO-GENERATED -->").unwrap()
});

/// Regex to strip ANSI escape codes (actual escape sequences)
static ANSI_ESCAPE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());

/// Regex to strip literal bracket notation (as stored in snapshots)
static ANSI_LITERAL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[0-9;]*m").unwrap());

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

/// Check if a line is gutter-formatted content (has the white background ANSI code)
fn is_gutter_line(line: &str) -> bool {
    line.starts_with("[107m") || line.starts_with("\x1b[107m")
}

/// Check if a line is a command gutter (gutter line with blue command name)
fn is_command_gutter(line: &str) -> bool {
    is_gutter_line(line) && line.contains("[34m")
}

/// Split stderr into logical chunks for interleaving with stdout
///
/// Chunks are split at command gutter boundaries and at transitions between
/// command output and regular gutter content.
fn split_stderr_chunks(stderr: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = Vec::new();
    let mut in_command_section = false;

    for line in stderr.lines() {
        if is_command_gutter(line) {
            // New command section - save previous chunk if any
            if !current_chunk.is_empty() {
                chunks.push(current_chunk.join("\n"));
                current_chunk = Vec::new();
            }
            current_chunk.push(line.to_string());
            in_command_section = true;
        } else if is_gutter_line(line) && in_command_section {
            // Non-command gutter after command section - new content section
            if !current_chunk.is_empty() {
                chunks.push(current_chunk.join("\n"));
                current_chunk = Vec::new();
            }
            current_chunk.push(line.to_string());
            in_command_section = false;
        } else {
            // Continue current chunk
            current_chunk.push(line.to_string());
        }
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk.join("\n"));
    }

    chunks
}

/// Combine stdout and stderr by inserting stderr content after trigger lines
///
/// The pattern is:
/// - Certain stdout lines trigger stderr content (commit messages, commands, etc.)
/// - stderr is split into chunks that correspond to these triggers
/// - All stderr content is included (gutter + child process output)
fn combine_stdout_stderr(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        return stdout.to_string();
    }

    let stdout_lines: Vec<&str> = stdout.lines().collect();
    let mut stderr_chunks: std::collections::VecDeque<String> = split_stderr_chunks(stderr).into();
    let mut result = Vec::new();

    for stdout_line in stdout_lines {
        result.push(stdout_line.to_string());

        // Check if this line triggers stderr content
        let triggers_stderr = stdout_line.contains("Generating")
            || stdout_line.contains("Running pre-")
            || stdout_line.contains("Running post-")
            || (stdout_line.contains("Merging") && stdout_line.contains("commit"));

        if triggers_stderr && let Some(chunk) = stderr_chunks.pop_front() {
            result.push(chunk);
        }
    }

    // Append any remaining stderr chunks (shouldn't normally happen)
    for chunk in stderr_chunks {
        result.push(chunk);
    }

    result.join("\n")
}

/// Parse content from an insta snapshot file, optionally including command line.
/// Command line comes from the README (preserved during update), not the snapshot.
fn parse_snapshot_with_command(
    path: &Path,
    readme_command: Option<&str>,
) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let content = parse_snapshot_content(&raw)?;

    Ok(match readme_command {
        Some(cmd) => format!("{}\n{}", cmd, content),
        None => content,
    })
}

/// Parse content from snapshot file content
fn parse_snapshot_content(content: &str) -> Result<String, String> {
    let content = content.to_string();

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

        // Combine stdout and stderr by inserting gutter content after trigger lines
        combine_stdout_stderr(&stdout, &stderr)
    } else {
        content
    };

    // Strip ANSI codes
    Ok(strip_ansi(&content))
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

/// Normalize snapshot output for README display (replace placeholders, trim whitespace)
fn normalize_for_readme(content: &str) -> String {
    // Real SHAs flow through from deterministic tests (fixed dates + git identity)
    let content = HASH_REGEX.replace_all(content, "a1b2c3d");
    // Replace branch worktree paths first (e.g., [TMPDIR]/repo.fix-auth -> ../repo.fix-auth)
    let content = TMPDIR_BRANCH_REGEX.replace_all(&content, "../repo.$1");
    // Replace main worktree paths (e.g., [TMPDIR]/repo -> ../repo), preserving trailing whitespace
    let content = TMPDIR_MAIN_REGEX.replace_all(&content, "../repo$1");
    let content = REPO_REGEX.replace_all(&content, "../repo");
    trim_lines(&content)
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

    // Use the already-built binary from cargo test
    let output = Command::new(env!("CARGO_BIN_EXE_wt"))
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
/// The replacement function receives (id, current_content) to allow preserving existing values.
fn update_readme_section(
    content: &str,
    pattern: &Regex,
    get_replacement: impl Fn(&str, &str) -> Result<String, String>,
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
            // Build replacement with new AUTO-GENERATED format
            // Include blank lines after opening marker and before closing marker
            // to match markdown formatter expectations
            let replacement = if wrapper.0.is_empty() {
                // No wrapper (help sections - rendered markdown)
                format!(
                    "<!-- ⚠️ AUTO-GENERATED from `{}` — edit source to update -->\n\n{}\n\n<!-- END AUTO-GENERATED -->",
                    id, expected
                )
            } else {
                // With wrapper (snapshot sections)
                // wrapper.0 includes trailing \n, so no extra newline between wrapper and content
                format!(
                    "<!-- ⚠️ AUTO-GENERATED from {} — edit source to update -->\n\n{}{}\n{}\n\n<!-- END AUTO-GENERATED -->",
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

/// Update help markers in a file (for both README.md and docs pages)
fn sync_help_markers(file_path: &Path, project_root: &Path) -> Result<usize, Vec<String>> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| vec![format!("Failed to read {}: {}", file_path.display(), e)])?;

    let project_root_clone = project_root.to_path_buf();
    match update_readme_section(
        &content,
        &HELP_MARKER_PATTERN,
        |cmd, _current| get_help_output(cmd, &project_root_clone),
        ("", ""),
    ) {
        Ok((new_content, updated_count, _total_count)) => {
            if updated_count > 0 {
                fs::write(file_path, &new_content).unwrap();
                println!(
                    "✅ Updated {} help sections in {}",
                    updated_count,
                    file_path.display()
                );
            }
            Ok(updated_count)
        }
        Err(errs) => Err(errs),
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

    // Update snapshot markers (with command line from YAML, or preserved from README)
    let project_root_for_snapshots = project_root.to_path_buf();
    match update_readme_section(
        &updated_content,
        &SNAPSHOT_MARKER_PATTERN,
        |snap_path, current_content| {
            // Extract existing command line from README if present (e.g., "$ wt switch --create fix-auth")
            let existing_command = current_content
                .lines()
                .next()
                .filter(|line| line.starts_with("$ "));
            let full_path = project_root_for_snapshots.join(snap_path);
            parse_snapshot_with_command(&full_path, existing_command)
                .map(|content| normalize_for_readme(&content))
        },
        ("```console\n", "```"),
    ) {
        Ok((new_content, updated_count, total_count)) => {
            updated_content = new_content;
            total_updated += updated_count;
            checked += total_count;
        }
        Err(errs) => errors.extend(errs),
    }

    // Update help markers (no wrapper - content is rendered markdown)
    let project_root_clone = project_root.to_path_buf();
    match update_readme_section(
        &updated_content,
        &HELP_MARKER_PATTERN,
        |cmd, _current| get_help_output(cmd, &project_root_clone),
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

#[test]
fn test_docs_commands_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let commands_path = project_root.join("docs/content/commands.md");

    if !commands_path.exists() {
        // Skip if docs directory doesn't exist
        return;
    }

    match sync_help_markers(&commands_path, project_root) {
        Ok(_) => {}
        Err(errors) => {
            panic!("Docs commands are out of sync:\n\n{}\n", errors.join("\n"));
        }
    }
}
