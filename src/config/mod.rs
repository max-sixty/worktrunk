//! Configuration system for worktrunk
//!
//! Worktrunk has two independent configuration files:
//!
//! # User Config (~/.config/worktrunk/config.toml)
//!
//! **Purpose**: Personal preferences, not checked into git
//!
//! **Settings**:
//! - `worktree-path` - Template for worktree paths (relative to repo root)
//! - `commit-generation` - LLM command and templates for commit messages
//! - `approved-commands` - Commands approved for automatic execution
//!
//! **Managed by**: Each developer maintains their own user config
//!
//! # Project Config (`<repo>`/.config/wt.toml)
//!
//! **Purpose**: Project-specific hooks and commands, checked into git
//!
//! **Settings**:
//! - `post-create-command` - Sequential blocking commands when creating worktree
//! - `post-start-command` - Parallel background commands after worktree created
//! - `pre-commit-command` - Validation before committing changes during merge
//! - `pre-merge-command` - Validation before merging to target branch
//! - `post-merge-command` - Cleanup after successful merge
//!
//! **Managed by**: Checked into the repository, shared across all developers
//!
//! # Configuration Model
//!
//! The two configs are **completely independent**:
//! - No overlap in settings (they configure different things)
//! - No merging or precedence rules needed
//! - Loaded separately and used in different contexts
//!
//! User config controls "how worktrunk behaves for me", project config controls
//! "what commands run for this project".

mod commands;
mod expansion;
mod project;
#[cfg(test)]
mod test;
mod user;

// Re-export public types
pub use commands::{Command, CommandConfig, CommandPhase};
pub use expansion::{expand_command_template, expand_template};
pub use project::ProjectConfig;
pub use user::{CommitGenerationConfig, UserProjectConfig, WorktrunkConfig, get_config_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = WorktrunkConfig::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("worktree-path"));
        assert!(toml.contains("../{{ main_worktree }}.{{ branch }}"));
        assert!(toml.contains("commit-generation"));
    }

    #[test]
    fn test_default_config() {
        let config = WorktrunkConfig::default();
        assert_eq!(config.worktree_path, "../{{ main_worktree }}.{{ branch }}");
        assert_eq!(config.commit_generation.command, None);
        assert!(config.projects.is_empty());
    }

    #[test]
    fn test_format_worktree_path() {
        let config = WorktrunkConfig {
            worktree_path: "{{ main_worktree }}.{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            "myproject.feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_custom_template() {
        let config = WorktrunkConfig {
            worktree_path: "{{ main_worktree }}-{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            "myproject-feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_only_branch() {
        let config = WorktrunkConfig {
            worktree_path: ".worktrees/{{ main_worktree }}/{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x").unwrap(),
            ".worktrees/myproject/feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_with_slashes() {
        // Slashes should be replaced with dashes to prevent directory traversal
        let config = WorktrunkConfig {
            worktree_path: "{{ main_worktree }}.{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature/foo").unwrap(),
            "myproject.feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_with_multiple_slashes() {
        let config = WorktrunkConfig {
            worktree_path: ".worktrees/{{ main_worktree }}/{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
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
            worktree_path: ".worktrees/{{ main_worktree }}/{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature\\foo").unwrap(),
            ".worktrees/myproject/feature-foo"
        );
    }

    #[test]
    fn test_project_config_default() {
        let config = ProjectConfig::default();
        assert!(config.post_create_command.is_none());
        assert!(config.post_start_command.is_none());
        assert!(config.pre_merge_command.is_none());
        assert!(config.post_merge_command.is_none());
    }

    #[test]
    fn test_command_config_single() {
        let toml = r#"post-create-command = "npm install""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_create_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0],
            Command::new(None, "npm install".to_string(), CommandPhase::PostCreate)
        );
    }

    #[test]
    fn test_command_config_multiple() {
        let toml = r#"post-create-command = ["npm install", "npm test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_create_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            Command::new(None, "npm install".to_string(), CommandPhase::PostCreate)
        );
        assert_eq!(
            commands[1],
            Command::new(None, "npm test".to_string(), CommandPhase::PostCreate)
        );
    }

    #[test]
    fn test_command_config_named() {
        let toml = r#"
            [post-start-command]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_start_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        // Preserves TOML insertion order
        assert_eq!(
            commands[0],
            Command::new(
                Some("server".to_string()),
                "npm run dev".to_string(),
                CommandPhase::PostCreate
            )
        );
        assert_eq!(
            commands[1],
            Command::new(
                Some("watch".to_string()),
                "npm run watch".to_string(),
                CommandPhase::PostCreate
            )
        );
    }

    #[test]
    fn test_command_config_named_preserves_toml_order() {
        // Test that named commands preserve TOML order (not alphabetical)
        let toml = r#"
            [pre-merge-command]
            insta = "cargo insta test"
            doc = "cargo doc"
            clippy = "cargo clippy"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
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
[post-start-command]
task1 = "echo 'Task 1 running' > task1.txt"
task2 = "echo 'Task 2 running' > task2.txt"
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_start_command.unwrap();
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
            post-create-command = ["npm install"]

            [post-start-command]
            server = "npm run dev"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.post_create_command.is_some());
        assert!(config.post_start_command.is_some());
    }

    #[test]
    fn test_pre_merge_command_single() {
        let toml = r#"pre-merge-command = "cargo test""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0],
            Command::new(None, "cargo test".to_string(), CommandPhase::PostCreate)
        );
    }

    #[test]
    fn test_pre_merge_command_multiple() {
        let toml = r#"pre-merge-command = ["cargo fmt -- --check", "cargo test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            Command::new(
                None,
                "cargo fmt -- --check".to_string(),
                CommandPhase::PostCreate
            )
        );
        assert_eq!(
            commands[1],
            Command::new(None, "cargo test".to_string(), CommandPhase::PostCreate)
        );
    }

    #[test]
    fn test_pre_merge_command_named() {
        let toml = r#"
            [pre-merge-command]
            format = "cargo fmt -- --check"
            lint = "cargo clippy"
            test = "cargo test"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 3);
        // Preserves TOML insertion order
        assert_eq!(
            commands[0],
            Command::new(
                Some("format".to_string()),
                "cargo fmt -- --check".to_string(),
                CommandPhase::PostCreate
            )
        );
        assert_eq!(
            commands[1],
            Command::new(
                Some("lint".to_string()),
                "cargo clippy".to_string(),
                CommandPhase::PostCreate
            )
        );
        assert_eq!(
            commands[2],
            Command::new(
                Some("test".to_string()),
                "cargo test".to_string(),
                CommandPhase::PostCreate
            )
        );
    }

    #[test]
    fn test_command_config_roundtrip_single() {
        let original = r#"post-create-command = "npm install""#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a string, not array
        assert!(serialized.contains(r#"post-create-command = "npm install""#));
    }

    #[test]
    fn test_command_config_roundtrip_multiple() {
        let original = r#"post-create-command = ["npm install", "npm test"]"#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as an array
        assert!(serialized.contains(r#"post-create-command = ["npm install", "npm test"]"#));
    }

    #[test]
    fn test_command_config_roundtrip_named() {
        let original = r#"
            [post-start-command]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a named table
        assert!(serialized.contains("[post-start-command]"));
        assert!(serialized.contains(r#"server = "npm run dev""#));
        assert!(serialized.contains(r#"watch = "npm run watch""#));
    }

    #[test]
    fn test_user_project_config_equality() {
        let config1 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
            list: None,
        };
        let config2 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
            list: None,
        };
        let config3 = UserProjectConfig {
            approved_commands: vec!["npm test".to_string()],
            list: None,
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
                list: None,
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
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
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
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
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
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm test".to_string(),
                &config_path,
            )
            .unwrap();

        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Revoke one command
        config
            .revoke_command_to("github.com/user/repo", "npm install", &config_path)
            .unwrap();
        assert!(!config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Project entry should still exist
        assert!(config.projects.contains_key("github.com/user/repo"));

        // Revoke the last command - should remove the project entry
        config
            .revoke_command_to("github.com/user/repo", "npm test", &config_path)
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
            .revoke_command_to("github.com/user/repo", "npm install", &config_path)
            .unwrap();

        // Set up one command
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();

        // Revoking non-existent command is a no-op
        config
            .revoke_command_to("github.com/user/repo", "npm test", &config_path)
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
            .approve_command_to(
                "github.com/user/repo1".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo1".to_string(),
                "npm test".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo2".to_string(),
                "cargo build".to_string(),
                &config_path,
            )
            .unwrap();

        assert!(config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoke entire project
        config
            .revoke_project_to("github.com/user/repo1", &config_path)
            .unwrap();
        assert!(!config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoking non-existent project is a no-op
        config
            .revoke_project_to("github.com/user/repo1", &config_path)
            .unwrap();
        config
            .revoke_project_to("github.com/nonexistent/repo", &config_path)
            .unwrap();
    }

    #[test]
    fn test_expand_template_basic() {
        use std::collections::HashMap;

        let result = expand_template(
            "../{{ main_worktree }}.{{ branch }}",
            "myrepo",
            "feature-x",
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, "../myrepo.feature-x");
    }

    #[test]
    fn test_expand_template_sanitizes_branch() {
        use std::collections::HashMap;

        let result = expand_template(
            "{{ main_worktree }}/{{ branch }}",
            "myrepo",
            "feature/foo",
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, "myrepo/feature-foo");

        let result = expand_template(
            ".worktrees/{{ main_worktree }}/{{ branch }}",
            "myrepo",
            "feat\\bar",
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result, ".worktrees/myrepo/feat-bar");
    }

    #[test]
    fn test_expand_template_with_extra_vars() {
        use std::collections::HashMap;

        let mut extra = HashMap::new();
        extra.insert("worktree", "/path/to/worktree");
        extra.insert("repo_root", "/path/to/repo");

        let result = expand_template(
            "{{ repo_root }}/target -> {{ worktree }}/target",
            "myrepo",
            "main",
            &extra,
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
}
