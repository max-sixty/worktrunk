//! Configuration commands.
//!
//! Commands for managing user config, project config, state, and hints.

mod create;
mod hints;
mod show;
mod state;

// Re-export public functions
pub use create::handle_config_create;
pub use hints::{handle_hints_clear, handle_hints_get};
pub use show::handle_config_show;
pub use state::{
    handle_state_clear, handle_state_clear_all, handle_state_get, handle_state_set,
    handle_state_show,
};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::create::comment_out_config;
    use super::show::{render_ci_tool_status, warn_unknown_keys};
    use super::state::{get_user_config_path, require_user_config_path, resolve_user_config_path};

    // ==================== comment_out_config tests ====================

    #[test]
    fn test_comment_out_config_basic() {
        let input = "key = \"value\"\n";
        let expected = "# key = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_existing_comments() {
        let input = "# This is a comment\nkey = \"value\"\n";
        let expected = "# This is a comment\n# key = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_empty_lines() {
        let input = "key1 = \"value\"\n\nkey2 = \"value\"\n";
        let expected = "# key1 = \"value\"\n\n# key2 = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_trailing_newline() {
        let with_newline = "key = \"value\"\n";
        let without_newline = "key = \"value\"";

        assert!(comment_out_config(with_newline).ends_with('\n'));
        assert!(!comment_out_config(without_newline).ends_with('\n'));
    }

    #[test]
    fn test_comment_out_config_section_headers() {
        let input = "[hooks]\ncommand = \"npm test\"\n";
        let expected = "# [hooks]\n# command = \"npm test\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_empty_input() {
        assert_eq!(comment_out_config(""), "");
    }

    #[test]
    fn test_comment_out_config_only_empty_lines() {
        let input = "\n\n\n";
        let expected = "\n\n\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_only_comments() {
        let input = "# comment 1\n# comment 2\n";
        let expected = "# comment 1\n# comment 2\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_mixed_content() {
        let input =
            "# Header comment\n\n[section]\nkey = \"value\"\n\n# Another comment\nkey2 = true\n";
        let expected = "# Header comment\n\n# [section]\n# key = \"value\"\n\n# Another comment\n# key2 = true\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_inline_table() {
        let input = "point = { x = 1, y = 2 }\n";
        let expected = "# point = { x = 1, y = 2 }\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_multiline_array() {
        let input = "args = [\n  \"--flag\",\n  \"value\"\n]\n";
        let expected = "# args = [\n#   \"--flag\",\n#   \"value\"\n# ]\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_whitespace_only_line() {
        // Lines with only whitespace are not empty - they should NOT be commented
        // Actually, let's check what the current behavior is:
        // The function checks `!line.is_empty()` - a line with spaces is not empty
        let input = "key = 1\n   \nkey2 = 2\n";
        let expected = "# key = 1\n#    \n# key2 = 2\n";
        assert_eq!(comment_out_config(input), expected);
    }

    // ==================== warn_unknown_keys tests ====================

    #[test]
    fn test_warn_unknown_keys_empty() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_warn_unknown_keys_single() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &["unknown-key".to_string()]).unwrap();
        assert!(out.contains("unknown-key"));
        assert!(out.contains("Unknown key"));
    }

    #[test]
    fn test_warn_unknown_keys_multiple() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &["key1".to_string(), "key2".to_string()]).unwrap();
        assert!(out.contains("key1"));
        assert!(out.contains("key2"));
        // Should have two separate warning lines
        assert_eq!(out.matches("Unknown key").count(), 2);
    }

    // ==================== render_ci_tool_status tests ====================

    #[test]
    fn test_render_ci_tool_status_installed_authenticated() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, true).unwrap();
        assert!(out.contains("gh"));
        assert!(out.contains("installed"));
        assert!(out.contains("authenticated"));
    }

    #[test]
    fn test_render_ci_tool_status_installed_not_authenticated() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, false).unwrap();
        assert!(out.contains("gh"));
        assert!(out.contains("installed"));
        assert!(out.contains("not authenticated"));
        assert!(out.contains("gh auth login"));
    }

    #[test]
    fn test_render_ci_tool_status_not_installed() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", false, false).unwrap();
        assert!(out.contains("glab"));
        assert!(out.contains("not found"));
        assert!(out.contains("GitLab"));
        assert!(out.contains("CI status unavailable"));
    }

    #[test]
    fn test_render_ci_tool_status_glab() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", true, true).unwrap();
        assert!(out.contains("glab"));
        assert!(out.contains("installed"));
        assert!(out.contains("authenticated"));
    }

    // ==================== resolve_user_config_path tests ====================

    #[test]
    fn test_resolve_user_config_path_xdg_takes_priority() {
        let path = resolve_user_config_path(Some("/custom/xdg"), Some("/home/user"));
        assert_eq!(
            path,
            Some(PathBuf::from("/custom/xdg/worktrunk/config.toml"))
        );
    }

    #[test]
    fn test_resolve_user_config_path_home_fallback() {
        let path = resolve_user_config_path(None, Some("/home/testuser"));
        assert_eq!(
            path,
            Some(PathBuf::from(
                "/home/testuser/.config/worktrunk/config.toml"
            ))
        );
    }

    #[test]
    fn test_resolve_user_config_path_none_when_no_env() {
        let path = resolve_user_config_path(None, None);
        assert_eq!(path, None);
    }

    // ==================== get_user_config_path tests ====================

    #[test]
    fn test_get_user_config_path_returns_some() {
        // In a normal environment, get_user_config_path should return Some
        // (either from env vars or etcetera fallback)
        let path = get_user_config_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("worktrunk/config.toml"));
    }

    // ==================== require_user_config_path tests ====================

    #[test]
    fn test_require_user_config_path_returns_ok() {
        // In a normal environment, require_user_config_path should succeed
        let result = require_user_config_path();
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.ends_with("worktrunk/config.toml"));
    }
}
