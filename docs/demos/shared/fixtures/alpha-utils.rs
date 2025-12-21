//! Utility functions for the Acme application.
//!
//! This module provides common utilities used throughout the codebase,
//! including string manipulation, path handling, and configuration helpers.

use std::path::{Path, PathBuf};

/// Normalize a path by resolving `.` and `..` components.
///
/// Unlike `std::fs::canonicalize`, this function does not require the path
/// to exist on the filesystem.
///
/// # Examples
///
/// ```
/// use acme::utils::normalize_path;
///
/// let path = normalize_path("/foo/bar/../baz");
/// assert_eq!(path, PathBuf::from("/foo/baz"));
/// ```
pub fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if !components.is_empty() {
                    components.pop();
                }
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }

    components.iter().collect()
}

/// Find the project root by searching for a marker file.
///
/// Walks up from the given directory looking for common project markers
/// like `Cargo.toml`, `.git`, or `package.json`.
///
/// # Arguments
///
/// * `start` - The directory to start searching from
/// * `markers` - A list of filenames to look for
///
/// # Returns
///
/// The path to the directory containing the marker, or `None` if not found.
pub fn find_project_root(start: &Path, markers: &[&str]) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        for marker in markers {
            if current.join(marker).exists() {
                return Some(current);
            }
        }

        if !current.pop() {
            return None;
        }
    }
}

/// Configuration for string formatting options.
#[derive(Debug, Clone, Default)]
pub struct FormatConfig {
    /// Maximum line width before wrapping
    pub max_width: Option<usize>,
    /// Indentation string (spaces or tabs)
    pub indent: String,
    /// Whether to trim trailing whitespace
    pub trim_trailing: bool,
    /// Whether to normalize line endings to LF
    pub normalize_newlines: bool,
}

impl FormatConfig {
    /// Create a new configuration with default values.
    pub fn new() -> Self {
        Self {
            max_width: Some(80),
            indent: "    ".to_string(),
            trim_trailing: true,
            normalize_newlines: true,
        }
    }

    /// Set the maximum line width.
    pub fn with_max_width(mut self, width: usize) -> Self {
        self.max_width = Some(width);
        self
    }

    /// Set the indentation string.
    pub fn with_indent(mut self, indent: impl Into<String>) -> Self {
        self.indent = indent.into();
        self
    }
}

/// Format a string according to the given configuration.
///
/// # Arguments
///
/// * `input` - The string to format
/// * `config` - The formatting configuration
///
/// # Returns
///
/// The formatted string.
pub fn format_string(input: &str, config: &FormatConfig) -> String {
    let mut output = input.to_string();

    // Normalize line endings
    if config.normalize_newlines {
        output = output.replace("\r\n", "\n").replace('\r', "\n");
    }

    // Trim trailing whitespace from each line
    if config.trim_trailing {
        output = output
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Wrap long lines if max_width is set
    if let Some(max_width) = config.max_width {
        output = wrap_lines(&output, max_width);
    }

    output
}

/// Wrap lines that exceed the maximum width.
fn wrap_lines(input: &str, max_width: usize) -> String {
    let mut result = Vec::new();

    for line in input.lines() {
        if line.len() <= max_width {
            result.push(line.to_string());
        } else {
            // Simple word wrapping
            let mut current_line = String::new();
            for word in line.split_whitespace() {
                if current_line.is_empty() {
                    current_line = word.to_string();
                } else if current_line.len() + 1 + word.len() <= max_width {
                    current_line.push(' ');
                    current_line.push_str(word);
                } else {
                    result.push(current_line);
                    current_line = word.to_string();
                }
            }
            if !current_line.is_empty() {
                result.push(current_line);
            }
        }
    }

    result.join("\n")
}

/// Parse a duration string like "5m", "2h", "1d" into seconds.
///
/// Supported units:
/// - `s` - seconds
/// - `m` - minutes
/// - `h` - hours
/// - `d` - days
///
/// # Examples
///
/// ```
/// use acme::utils::parse_duration;
///
/// assert_eq!(parse_duration("5m"), Ok(300));
/// assert_eq!(parse_duration("2h"), Ok(7200));
/// ```
pub fn parse_duration(input: &str) -> Result<u64, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty duration string".to_string());
    }

    let (num_str, unit) = input.split_at(input.len() - 1);
    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: {}", num_str))?;

    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(format!("unknown unit: {}", unit)),
    };

    Ok(num * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("/foo/bar/../baz"), PathBuf::from("/foo/baz"));
        assert_eq!(normalize_path("/foo/./bar"), PathBuf::from("/foo/bar"));
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("5m"), Ok(300));
        assert_eq!(parse_duration("2h"), Ok(7200));
        assert_eq!(parse_duration("1d"), Ok(86400));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("5x").is_err());
    }

    #[test]
    fn test_format_config() {
        let config = FormatConfig::new().with_max_width(100).with_indent("  ");
        assert_eq!(config.max_width, Some(100));
        assert_eq!(config.indent, "  ");
    }

    #[test]
    fn test_format_string_trailing_whitespace() {
        let config = FormatConfig::new();
        let input = "hello   \nworld  ";
        let output = format_string(input, &config);
        assert!(!output.lines().any(|l| l.ends_with(' ')));
    }
}
