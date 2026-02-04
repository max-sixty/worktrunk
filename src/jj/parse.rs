//! Parsing utilities for jj command output.

use std::path::PathBuf;

use super::WorkspaceInfo;

/// Parse the output of `jj workspace list`.
///
/// The output format is:
/// ```text
/// default: /path/to/repo (current)
/// feature: /path/to/repo.feature
/// ```
///
/// Or with --template for machine-readable output:
/// ```text
/// default\0/path/to/repo\0true\0
/// feature\0/path/to/repo.feature\0false\0
/// ```
pub fn parse_workspace_list(output: &str) -> anyhow::Result<Vec<WorkspaceInfo>> {
    let mut workspaces = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse the machine-readable format (null-separated)
        if line.contains('\0') {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 3 {
                workspaces.push(WorkspaceInfo {
                    name: parts[0].to_string(),
                    path: PathBuf::from(parts[1]),
                    working_copy_commit: parts.get(3).unwrap_or(&"").to_string(),
                    bookmark: parts.get(4).filter(|s| !s.is_empty()).map(|s| s.to_string()),
                    is_current: parts[2] == "true",
                });
            }
            continue;
        }

        // Parse the human-readable format: "name: /path (current)"
        if let Some((name, rest)) = line.split_once(": ") {
            let is_current = rest.ends_with("(current)");
            let path_str = if is_current {
                rest.trim_end_matches("(current)").trim()
            } else {
                rest.trim()
            };

            workspaces.push(WorkspaceInfo {
                name: name.to_string(),
                path: PathBuf::from(path_str),
                working_copy_commit: String::new(), // Not available in human-readable format
                bookmark: None,                     // Need to look this up separately
                is_current,
            });
        }
    }

    Ok(workspaces)
}

/// Parse the output of `jj bookmark list`.
///
/// The output format varies, but we use --template for machine-readable output:
/// ```text
/// main\0abc123\0
/// feature\0def456\0
/// ```
pub fn parse_bookmark_list(output: &str) -> Vec<(String, String)> {
    let mut bookmarks = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse null-separated format
        if line.contains('\0') {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 2 {
                bookmarks.push((parts[0].to_string(), parts[1].to_string()));
            }
            continue;
        }

        // Parse human-readable format: "main: abc123"
        if let Some((name, commit)) = line.split_once(": ") {
            bookmarks.push((name.to_string(), commit.trim().to_string()));
        }
    }

    bookmarks
}

/// Parse the output of `jj log` to get commit information.
///
/// Returns (commit_id, change_id, description) tuples.
pub fn parse_log_output(output: &str) -> Vec<(String, String, String)> {
    let mut commits = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse null-separated format: commit_id\0change_id\0description\0
        if line.contains('\0') {
            let parts: Vec<&str> = line.split('\0').collect();
            if parts.len() >= 3 {
                commits.push((
                    parts[0].to_string(),
                    parts[1].to_string(),
                    parts[2].to_string(),
                ));
            }
        }
    }

    commits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_workspace_list_human_readable() {
        let output = r#"
default: /home/user/repo (current)
feature: /home/user/repo.feature
bugfix: /home/user/repo.bugfix
"#;

        let workspaces = parse_workspace_list(output).unwrap();

        assert_eq!(workspaces.len(), 3);

        assert_eq!(workspaces[0].name, "default");
        assert_eq!(workspaces[0].path, PathBuf::from("/home/user/repo"));
        assert!(workspaces[0].is_current);

        assert_eq!(workspaces[1].name, "feature");
        assert_eq!(workspaces[1].path, PathBuf::from("/home/user/repo.feature"));
        assert!(!workspaces[1].is_current);

        assert_eq!(workspaces[2].name, "bugfix");
        assert_eq!(workspaces[2].path, PathBuf::from("/home/user/repo.bugfix"));
        assert!(!workspaces[2].is_current);
    }

    #[test]
    fn test_parse_workspace_list_machine_readable() {
        let output = "default\0/home/user/repo\0true\0abc123\0main\0\n\
                      feature\0/home/user/repo.feature\0false\0def456\0\0\n";

        let workspaces = parse_workspace_list(output).unwrap();

        assert_eq!(workspaces.len(), 2);

        assert_eq!(workspaces[0].name, "default");
        assert_eq!(workspaces[0].path, PathBuf::from("/home/user/repo"));
        assert!(workspaces[0].is_current);
        assert_eq!(workspaces[0].working_copy_commit, "abc123");
        assert_eq!(workspaces[0].bookmark, Some("main".to_string()));

        assert_eq!(workspaces[1].name, "feature");
        assert_eq!(workspaces[1].path, PathBuf::from("/home/user/repo.feature"));
        assert!(!workspaces[1].is_current);
        assert_eq!(workspaces[1].working_copy_commit, "def456");
        assert_eq!(workspaces[1].bookmark, None);
    }

    #[test]
    fn test_parse_bookmark_list() {
        let output = "main\0abc123\0\nfeature\0def456\0\n";

        let bookmarks = parse_bookmark_list(output);

        assert_eq!(bookmarks.len(), 2);
        assert_eq!(bookmarks[0], ("main".to_string(), "abc123".to_string()));
        assert_eq!(bookmarks[1], ("feature".to_string(), "def456".to_string()));
    }
}
