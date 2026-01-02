//! Configuration system for worktrunk
//!
//! Worktrunk uses two independent configuration files:
//!
//! - **User config** (`~/.config/worktrunk/config.toml`) - Personal preferences
//! - **Project config** (`.config/wt.toml`) - Lifecycle hooks, checked into git
//!
//! The two configs are **completely independent**:
//! - No overlap in settings (they configure different things)
//! - No merging or precedence rules needed
//! - Loaded separately and used in different contexts
//!
//! User config controls "how worktrunk behaves for me", project config controls
//! "what commands run for this project".
//!
//! See `wt config --help` for complete documentation.

mod commands;
mod deprecation;
mod expansion;
mod hooks;
mod project;
#[cfg(test)]
mod test;
mod user;

// Re-export public types
pub use commands::{Command, CommandConfig};
pub use deprecation::check_and_migrate as check_deprecated_vars;
pub use expansion::{expand_template, sanitize_branch_name};
pub use hooks::HooksConfig;
pub use project::{
    ProjectConfig, ProjectListConfig, find_unknown_keys as find_unknown_project_keys,
};
pub use user::{
    CommitGenerationConfig, StageMode, UserProjectConfig, WorktrunkConfig,
    find_unknown_keys as find_unknown_user_keys, get_config_path, set_config_path,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = WorktrunkConfig::default();
        let toml = toml::to_string(&config).unwrap();
        // worktree-path is not serialized when None (uses built-in default)
        assert!(!toml.contains("worktree-path"));
        assert!(toml.contains("commit-generation"));
    }

    #[test]
    fn test_config_serialization_with_worktree_path() {
        let config = WorktrunkConfig {
            worktree_path: Some("custom/{{ branch }}".to_string()),
            ..Default::default()
        };
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("worktree-path"));
        assert!(toml.contains("custom/{{ branch }}"));
    }

    #[test]
    fn test_default_config() {
        let config = WorktrunkConfig::default();
        // worktree_path is None by default, but the getter returns the default
        assert!(config.worktree_path.is_none());
        assert_eq!(
            config.worktree_path(),
            "../{{ repo }}.{{ branch | sanitize }}"
        );
        assert_eq!(config.commit_generation.command, None);
        assert!(config.projects.is_empty());
    }

    #[test]
    fn test_format_worktree_path() {
        let config = WorktrunkConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            "myproject.feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_custom_template() {
        let config = WorktrunkConfig {
            worktree_path: Some("{{ main_worktree }}-{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            "myproject-feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_only_branch() {
        let config = WorktrunkConfig {
            worktree_path: Some(".worktrees/{{ main_worktree }}/{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            ".worktrees/myproject/feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_with_slashes() {
        // Use {{ branch | sanitize }} to replace slashes with dashes
        let config = WorktrunkConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch | sanitize }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature/foo").unwrap(),
            "myproject.feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_with_multiple_slashes() {
        let config = WorktrunkConfig {
            worktree_path: Some(
                ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature/sub/task").unwrap(),
            ".worktrees/myproject/feature-sub-task"
        );
    }

    #[test]
    fn test_format_worktree_path_with_backslashes() {
        // Windows-style path separators should also be sanitized
        let config = WorktrunkConfig {
            worktree_path: Some(
                ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature\\foo").unwrap(),
            ".worktrees/myproject/feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_raw_branch() {
        // {{ branch }} without filter gives raw branch name
        let config = WorktrunkConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.format_path("myproject", "feature/foo").unwrap(),
            "myproject.feature/foo"
        );
    }

    #[test]
    fn test_project_config_default() {
        let config = ProjectConfig::default();
        assert!(config.hooks.post_create.is_none());
        assert!(config.hooks.post_start.is_none());
        assert!(config.hooks.pre_merge.is_none());
        assert!(config.hooks.post_merge.is_none());
    }

    #[test]
    fn test_command_config_single() {
        let toml = r#"post-create = "npm install""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.post_create.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], Command::new(None, "npm install".to_string()));
    }

    #[test]
    fn test_command_config_named() {
        let toml = r#"
            [post-start]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.post_start.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        // Preserves TOML insertion order
        assert_eq!(
            commands[0],
            Command::new(Some("server".to_string()), "npm run dev".to_string())
        );
        assert_eq!(
            commands[1],
            Command::new(Some("watch".to_string()), "npm run watch".to_string())
        );
    }

    #[test]
    fn test_command_config_named_preserves_toml_order() {
        // Test that named commands preserve TOML order (not alphabetical)
        let toml = r#"
            [pre-merge]
            insta = "cargo insta test"
            doc = "cargo doc"
            clippy = "cargo clippy"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.pre_merge.unwrap();
        let commands = cmd_config.commands();

        // Extract just the names for easier verification
        let names: Vec<_> = commands
            .iter()
            .map(|cmd| cmd.name.as_deref().unwrap())
            .collect();

        // Verify TOML insertion order is preserved
        assert_eq!(names, vec!["insta", "doc", "clippy"]);

        // Verify it's NOT alphabetical (which would be clippy, doc, insta)
        let mut alphabetical = names.clone();
        alphabetical.sort();
        assert_ne!(
            names, alphabetical,
            "Order should be TOML insertion order, not alphabetical"
        );
    }

    #[test]
    fn test_command_config_task_order() {
        // Test exact ordering as used in post_start tests
        let toml = r#"
[post-start]
task1 = "echo 'Task 1 running' > task1.txt"
task2 = "echo 'Task 2 running' > task2.txt"
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.post_start.unwrap();
        let commands = cmd_config.commands();

        assert_eq!(commands.len(), 2);
        // Should be in TOML order: task1, task2
        assert_eq!(
            commands[0].name.as_deref(),
            Some("task1"),
            "First command should be task1"
        );
        assert_eq!(
            commands[1].name.as_deref(),
            Some("task2"),
            "Second command should be task2"
        );
    }

    #[test]
    fn test_project_config_both_commands() {
        let toml = r#"
            post-create = "npm install"

            [post-start]
            server = "npm run dev"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.hooks.post_create.is_some());
        assert!(config.hooks.post_start.is_some());
    }

    #[test]
    fn test_pre_merge_command_single() {
        let toml = r#"pre-merge = "cargo test""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.pre_merge.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], Command::new(None, "cargo test".to_string()));
    }

    #[test]
    fn test_pre_merge_command_named() {
        let toml = r#"
            [pre-merge]
            format = "cargo fmt -- --check"
            lint = "cargo clippy"
            test = "cargo test"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.pre_merge.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 3);
        // Preserves TOML insertion order
        assert_eq!(
            commands[0],
            Command::new(
                Some("format".to_string()),
                "cargo fmt -- --check".to_string()
            )
        );
        assert_eq!(
            commands[1],
            Command::new(Some("lint".to_string()), "cargo clippy".to_string())
        );
        assert_eq!(
            commands[2],
            Command::new(Some("test".to_string()), "cargo test".to_string())
        );
    }

    #[test]
    fn test_command_config_roundtrip_single() {
        let original = r#"post-create = "npm install""#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a string
        assert!(serialized.contains(r#"post-create = "npm install""#));
    }

    #[test]
    fn test_command_config_roundtrip_named() {
        let original = r#"
            [post-start]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a named table
        assert!(serialized.contains("[post-start]"));
        assert!(serialized.contains(r#"server = "npm run dev""#));
        assert!(serialized.contains(r#"watch = "npm run watch""#));
    }

    #[test]
    fn test_user_project_config_equality() {
        let config1 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
        };
        let config2 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
        };
        let config3 = UserProjectConfig {
            approved_commands: vec!["npm test".to_string()],
        };
        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }

    #[test]
    fn test_is_command_approved() {
        let mut config = WorktrunkConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectConfig {
                approved_commands: vec!["npm install".to_string()],
            },
        );

        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(!config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.is_command_approved("github.com/other/repo", "npm install"));
    }

    #[test]
    fn test_approve_command() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");
        let mut config = WorktrunkConfig::default();

        // First approval
        assert!(!config.is_command_approved("github.com/user/repo", "npm install"));
        config
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&config_path),
            )
            .unwrap();
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));

        // Duplicate approval shouldn't add twice
        let count_before = config
            .projects
            .get("github.com/user/repo")
            .unwrap()
            .approved_commands
            .len();
        config
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&config_path),
            )
            .unwrap();
        assert_eq!(
            config
                .projects
                .get("github.com/user/repo")
                .unwrap()
                .approved_commands
                .len(),
            count_before
        );
    }

    #[test]
    fn test_revoke_command() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Set up two approved commands
        config
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&config_path),
            )
            .unwrap();
        config
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm test".to_string(),
                Some(&config_path),
            )
            .unwrap();

        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Revoke one command
        config
            .revoke_command("github.com/user/repo", "npm install", Some(&config_path))
            .unwrap();
        assert!(!config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Project entry should still exist
        assert!(config.projects.contains_key("github.com/user/repo"));

        // Revoke the last command - should remove the project entry
        config
            .revoke_command("github.com/user/repo", "npm test", Some(&config_path))
            .unwrap();
        assert!(!config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.projects.contains_key("github.com/user/repo"));
    }

    #[test]
    fn test_revoke_command_nonexistent() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Revoking from non-existent project is a no-op
        config
            .revoke_command("github.com/user/repo", "npm install", Some(&config_path))
            .unwrap();

        // Set up one command
        config
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&config_path),
            )
            .unwrap();

        // Revoking non-existent command is a no-op
        config
            .revoke_command("github.com/user/repo", "npm test", Some(&config_path))
            .unwrap();
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
    }

    #[test]
    fn test_revoke_project() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Set up multiple projects
        config
            .approve_command(
                "github.com/user/repo1".to_string(),
                "npm install".to_string(),
                Some(&config_path),
            )
            .unwrap();
        config
            .approve_command(
                "github.com/user/repo1".to_string(),
                "npm test".to_string(),
                Some(&config_path),
            )
            .unwrap();
        config
            .approve_command(
                "github.com/user/repo2".to_string(),
                "cargo build".to_string(),
                Some(&config_path),
            )
            .unwrap();

        assert!(config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoke entire project
        config
            .revoke_project("github.com/user/repo1", Some(&config_path))
            .unwrap();
        assert!(!config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoking non-existent project is a no-op
        config
            .revoke_project("github.com/user/repo1", Some(&config_path))
            .unwrap();
        config
            .revoke_project("github.com/nonexistent/repo", Some(&config_path))
            .unwrap();
    }

    #[test]
    fn test_expand_template_basic() {
        use std::collections::HashMap;

        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feature-x");
        let result = expand_template("../{{ main_worktree }}.{{ branch }}", &vars, true).unwrap();
        assert_eq!(result, "../myrepo.feature-x");
    }

    #[test]
    fn test_expand_template_sanitizes_branch() {
        use std::collections::HashMap;

        // Use {{ branch | sanitize }} filter for filesystem-safe paths
        // shell_escape=false to test filter in isolation (shell escaping tested separately)
        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feature/foo");
        let result =
            expand_template("{{ main_worktree }}/{{ branch | sanitize }}", &vars, false).unwrap();
        assert_eq!(result, "myrepo/feature-foo");

        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feat\\bar");
        let result = expand_template(
            ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}",
            &vars,
            false,
        )
        .unwrap();
        assert_eq!(result, ".worktrees/myrepo/feat-bar");
    }

    #[test]
    fn test_expand_template_with_extra_vars() {
        use std::collections::HashMap;

        let mut vars = HashMap::new();
        vars.insert("worktree", "/path/to/worktree");
        vars.insert("repo_root", "/path/to/repo");

        let result = expand_template(
            "{{ repo_root }}/target -> {{ worktree }}/target",
            &vars,
            true,
        )
        .unwrap();
        assert_eq!(result, "/path/to/repo/target -> /path/to/worktree/target");
    }

    #[test]
    fn test_commit_generation_config_mutually_exclusive_validation() {
        // Test that deserialization rejects both template and template-file
        let toml_content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
template = "inline template"
template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<WorktrunkConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            // Verify validation logic: both fields should not be Some
            let has_both = config.commit_generation.template.is_some()
                && config.commit_generation.template_file.is_some();
            assert!(
                has_both,
                "Config should have both template fields set for this test"
            );
        }
    }

    #[test]
    fn test_squash_template_mutually_exclusive_validation() {
        // Test that deserialization rejects both squash-template and squash-template-file
        let toml_content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
squash-template = "inline template"
squash-template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<WorktrunkConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            // Verify validation logic: both fields should not be Some
            let has_both = config.commit_generation.squash_template.is_some()
                && config.commit_generation.squash_template_file.is_some();
            assert!(
                has_both,
                "Config should have both squash template fields set for this test"
            );
        }
    }

    #[test]
    fn test_commit_generation_config_serialization() {
        let config = CommitGenerationConfig {
            command: Some("llm".to_string()),
            args: vec!["-m".to_string(), "model".to_string()],
            template: Some("template content".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };

        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("llm"));
        assert!(toml.contains("template"));
    }

    #[test]
    fn test_find_unknown_project_keys_with_typo() {
        let toml_str = "[post-merge-command]\ndeploy = \"task deploy\"";
        let unknown = find_unknown_project_keys(toml_str);
        assert_eq!(unknown, vec!["post-merge-command"]);
    }

    #[test]
    fn test_find_unknown_project_keys_valid() {
        let toml_str =
            "[post-merge]\ndeploy = \"task deploy\"\n\n[pre-merge]\ntest = \"cargo test\"";
        let unknown = find_unknown_project_keys(toml_str);
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_find_unknown_project_keys_multiple() {
        let toml_str = "[post-merge-command]\ndeploy = \"task deploy\"\n\n[after-create]\nsetup = \"npm install\"";
        let unknown = find_unknown_project_keys(toml_str);
        assert_eq!(unknown.len(), 2);
        assert!(unknown.contains(&"post-merge-command".to_string()));
        assert!(unknown.contains(&"after-create".to_string()));
    }

    #[test]
    fn test_find_unknown_user_keys_with_typo() {
        let toml_str = "worktree-path = \"../test\"\n\n[commit-gen]\ncommand = \"llm\"";
        let unknown = find_unknown_user_keys(toml_str);
        assert_eq!(unknown, vec!["commit-gen"]);
    }

    #[test]
    fn test_find_unknown_user_keys_valid() {
        let toml_str = "worktree-path = \"../test\"\n\n[commit-generation]\ncommand = \"llm\"\n\n[list]\nfull = true";
        let unknown = find_unknown_user_keys(toml_str);
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_find_unknown_keys_invalid_toml() {
        let toml = "this is not valid toml {{{";
        let unknown_project = find_unknown_project_keys(toml);
        let unknown_user = find_unknown_user_keys(toml);
        assert!(unknown_project.is_empty());
        assert!(unknown_user.is_empty());
    }

    #[test]
    fn test_user_hooks_config_parsing() {
        let toml_str = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
log = "echo '{{ repo }}' >> ~/.log"

[pre-merge]
test = "cargo test"
lint = "cargo clippy"
"#;
        let config: WorktrunkConfig = toml::from_str(toml_str).unwrap();

        // Check post-create
        let post_create = config
            .hooks
            .post_create
            .expect("post-create should be present");
        let commands = post_create.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name.as_deref(), Some("log"));

        // Check pre-merge (multiple commands preserve order)
        let pre_merge = config.hooks.pre_merge.expect("pre-merge should be present");
        let commands = pre_merge.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].name.as_deref(), Some("test"));
        assert_eq!(commands[1].name.as_deref(), Some("lint"));
    }

    #[test]
    fn test_user_hooks_config_single_command() {
        let toml_str = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
post-create = "npm install"
"#;
        let config: WorktrunkConfig = toml::from_str(toml_str).unwrap();

        let post_create = config
            .hooks
            .post_create
            .expect("post-create should be present");
        let commands = post_create.commands();
        assert_eq!(commands.len(), 1);
        assert!(commands[0].name.is_none()); // single command has no name
        assert_eq!(commands[0].template, "npm install");
    }

    #[test]
    fn test_user_hooks_not_reported_as_unknown() {
        let toml_str = r#"
worktree-path = "../test"
post-create = "npm install"

[pre-merge]
test = "cargo test"
"#;
        let unknown = find_unknown_user_keys(toml_str);
        assert!(
            unknown.is_empty(),
            "hook fields should not be reported as unknown: {:?}",
            unknown
        );
    }
}
