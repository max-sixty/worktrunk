//! Configuration commands.
//!
//! Commands for managing user config, project config, state, and hints.

mod create;
mod hints;
pub mod opencode;
mod plugins;
mod show;
mod state;
mod update;

// Re-export public functions
pub use create::handle_config_create;
pub use hints::{handle_hints_clear, handle_hints_get};
pub use opencode::{handle_opencode_install, handle_opencode_uninstall};
pub use plugins::{
    handle_claude_install, handle_claude_install_statusline, handle_claude_uninstall,
};
pub use show::handle_config_show;
pub use state::{
    handle_logs_list, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show, handle_vars_clear, handle_vars_get, handle_vars_list,
    handle_vars_set,
};
pub use update::handle_config_update;

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use worktrunk::config::{ProjectConfig, UserConfig};

    use super::create::comment_out_config;
    use super::show::{render_ci_tool_status, warn_unknown_keys};
    use super::state::require_user_config_path;

    // ==================== comment_out_config tests ====================

    #[test]
    fn test_comment_out_config() {
        // Basic key-value
        assert_snapshot!(comment_out_config("key = \"value\"\n"), @r#"# key = "value""#);

        // Preserves existing comments
        assert_snapshot!(comment_out_config("# This is a comment\nkey = \"value\"\n"), @r#"
        # This is a comment
        # key = "value"
        "#);

        // Preserves empty lines (not commented)
        assert_snapshot!(comment_out_config("key1 = \"value\"\n\nkey2 = \"value\"\n"), @r#"
        # key1 = "value"

        # key2 = "value"
        "#);

        // Section headers
        assert_snapshot!(comment_out_config("[hooks]\ncommand = \"npm test\"\n"), @r#"
        # [hooks]
        # command = "npm test"
        "#);

        // Empty input
        assert_snapshot!(comment_out_config(""), @"");

        // Only empty lines
        assert_snapshot!(comment_out_config("\n\n\n"), @"");

        // Only comments (unchanged)
        assert_snapshot!(comment_out_config("# comment 1\n# comment 2\n"), @"
        # comment 1
        # comment 2
        ");

        // Mixed content
        assert_snapshot!(
            comment_out_config("# Header comment\n\n[section]\nkey = \"value\"\n\n# Another comment\nkey2 = true\n"),
            @r#"
        # Header comment

        # [section]
        # key = "value"

        # Another comment
        # key2 = true
        "#
        );

        // Inline table
        assert_snapshot!(comment_out_config("point = { x = 1, y = 2 }\n"), @"# point = { x = 1, y = 2 }");

        // Multiline array
        assert_snapshot!(comment_out_config("args = [\n  \"--flag\",\n  \"value\"\n]\n"), @r#"
        # args = [
        #   "--flag",
        #   "value"
        # ]
        "#);

        // Whitespace-only lines are not empty, so they get commented
        assert_snapshot!(comment_out_config("key = 1\n   \nkey2 = 2\n"), @"
        # key = 1
        #    
        # key2 = 2
        ");
    }

    #[test]
    fn test_comment_out_config_preserves_trailing_newline() {
        assert!(comment_out_config("key = \"value\"\n").ends_with('\n'));
        assert!(!comment_out_config("key = \"value\"").ends_with('\n'));
    }

    // ==================== warn_unknown_keys tests ====================

    #[test]
    fn test_warn_unknown_keys_empty() {
        let out = warn_unknown_keys::<UserConfig>("");
        assert!(out.is_empty());
    }

    #[test]
    fn test_warn_unknown_keys() {
        // Single unknown key
        assert_snapshot!(warn_unknown_keys::<UserConfig>("unknown-key = \"value\"\n"), @"[33m▲[39m [33mUnknown key [1munknown-key[22m will be ignored[39m");

        // Multiple unknown keys (output is sorted deterministically)
        assert_snapshot!(warn_unknown_keys::<UserConfig>("key1 = \"v1\"\nkey2 = \"v2\"\n"), @"
        [33m▲[39m [33mUnknown key [1mkey1[22m will be ignored[39m
        [33m▲[39m [33mUnknown key [1mkey2[22m will be ignored[39m
        ");
    }

    #[test]
    fn test_warn_unknown_keys_nested() {
        // Nested typos surface as dotted paths — a UX win from round-trip analysis.
        insta::assert_snapshot!(warn_unknown_keys::<UserConfig>("[merge]\nsquas = true\n"));
    }

    #[test]
    fn test_warn_unknown_keys_suggests_other_config() {
        // skip-shell-integration-prompt in project config should suggest user config
        assert_snapshot!(
            warn_unknown_keys::<ProjectConfig>("skip-shell-integration-prompt = true\n"),
            @"[33m▲[39m [33mKey [1mskip-shell-integration-prompt[22m belongs in user config (will be ignored)[39m");

        // forge in user config should suggest project config
        assert_snapshot!(warn_unknown_keys::<UserConfig>("[forge]\nplatform = \"github\"\n"), @"[33m▲[39m [33mKey [1mforge[22m belongs in project config (will be ignored)[39m");
    }

    #[test]
    fn test_warn_unknown_keys_deprecated_in_wrong_config() {
        // commit-generation in project config should suggest user config with canonical form
        assert_snapshot!(warn_unknown_keys::<ProjectConfig>(
            "[commit-generation]\ncommand = \"llm\"\n"
        ));

        // ci in user config should suggest project config with canonical form
        assert_snapshot!(warn_unknown_keys::<UserConfig>(
            "[ci]\nplatform = \"github\"\n"
        ));
    }

    #[test]
    fn test_warn_unknown_keys_deprecated_in_right_config_is_skipped() {
        // commit-generation in user config should be skipped (deprecation system handles it)
        assert!(
            warn_unknown_keys::<UserConfig>("[commit-generation]\ncommand = \"llm\"\n").is_empty()
        );

        // ci in project config should be skipped (deprecation system handles it)
        assert!(warn_unknown_keys::<ProjectConfig>("[ci]\nplatform = \"github\"\n").is_empty());
    }

    // ==================== render_ci_tool_status tests ====================

    #[test]
    fn test_render_ci_tool_status() {
        // Installed and authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, true).unwrap();
        assert_snapshot!(out, @"[32m✓[39m [32m[1mgh[22m installed & authenticated[39m");

        // Installed but not authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, false).unwrap();
        assert_snapshot!(out, @"[33m▲[39m [33m[1mgh[22m installed but not authenticated; run [1mgh auth login[22m[39m");

        // Not installed
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", false, false).unwrap();
        assert_snapshot!(out, @"[2m↳[22m [2m[1mglab[22m not found (GitLab CI status unavailable)[22m");

        // glab installed and authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", true, true).unwrap();
        assert_snapshot!(out, @"[32m✓[39m [32m[1mglab[22m installed & authenticated[39m");
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

    #[test]
    fn test_require_user_config_path_matches_config_path() {
        // Verify that config create/show path matches config loading path.
        // This was the root cause of #1134: the two paths diverged on Windows
        // when XDG_CONFIG_HOME was set because config create had its own
        // XDG/HOME resolution that differed from config loading.
        let create_path = require_user_config_path().unwrap();
        let load_path = worktrunk::config::config_path().unwrap();
        assert_eq!(
            create_path, load_path,
            "config create path and config loading path must be identical"
        );
    }
}
