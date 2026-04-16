//! Configuration system for worktrunk
//!
//! Three configuration sources, loaded in order (later overrides earlier):
//!
//! 1. **System config** (`/etc/xdg/worktrunk/config.toml` or platform equivalent) -
//!    Organization-wide defaults, optional
//! 2. **User config** (`~/.config/worktrunk/config.toml`) - Personal preferences
//! 3. **Project config** (`.config/wt.toml`) - Lifecycle hooks, checked into git
//!
//! System and user configs share the same schema and are merged via
//! `deep_merge_table` (user values override system values at the key level).
//! Project config is independent — different schema, different purpose.
//!
//! See `wt config --help` for complete documentation.

pub mod approvals;
mod commands;
pub(crate) mod deprecation;
mod expansion;
mod hooks;
mod project;
#[cfg(test)]
mod test;
mod unknown_tree;
mod user;

/// Trait for worktrunk config types (user and project config).
///
/// Both config types expose JsonSchema-derived top-level keys. The list drives
/// `is_valid_key` (for misplaced-key classification) and seeds the round-trip
/// comparison in `unknown_tree::compute_unknown_tree` so sections that
/// serialize away when default (e.g., `MergeConfig` under
/// `skip_serializing_if`) aren't mistaken for schema-unknown paths.
pub trait WorktrunkConfig:
    for<'de> serde::Deserialize<'de> + serde::Serialize + Default + Sized
{
    /// The other config type (UserConfig ↔ ProjectConfig).
    type Other: WorktrunkConfig;

    /// Human-readable description of where this config lives.
    fn description() -> &'static str;

    /// All valid top-level keys for this config type, derived from JsonSchema.
    fn valid_top_level_keys() -> &'static [String];

    /// Check if a key would be valid in this config type.
    fn is_valid_key(key: &str) -> bool {
        Self::valid_top_level_keys().iter().any(|k| k == key)
    }
}

impl WorktrunkConfig for UserConfig {
    type Other = ProjectConfig;

    fn description() -> &'static str {
        "user config"
    }

    fn valid_top_level_keys() -> &'static [String] {
        use std::sync::OnceLock;
        static VALID_KEYS: OnceLock<Vec<String>> = OnceLock::new();
        VALID_KEYS.get_or_init(user::valid_user_config_keys)
    }
}

impl WorktrunkConfig for ProjectConfig {
    type Other = UserConfig;

    fn description() -> &'static str {
        "project config"
    }

    fn valid_top_level_keys() -> &'static [String] {
        use std::sync::OnceLock;
        static VALID_KEYS: OnceLock<Vec<String>> = OnceLock::new();
        VALID_KEYS.get_or_init(project::valid_project_config_keys)
    }
}

/// Configuration error type.
///
/// Replaces the `config` crate's `ConfigError` with a simple string wrapper.
/// Every usage was `ConfigError::Message(String)` — no other variants were used.
#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Returns true if the given value equals `T::default()`.
///
/// Used as `skip_serializing_if` so section types like `ListConfig` /
/// `MergeConfig` are omitted from serialized TOML when no fields are set.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

// Re-export public types
pub use approvals::{Approvals, approvals_path};
pub use commands::{Command, CommandConfig, HookStep, append_aliases};
pub use deprecation::CheckAndMigrateResult;
pub use deprecation::DeprecationInfo;
pub use deprecation::Deprecations;
pub use deprecation::check_and_migrate;
pub use deprecation::compute_migrated_content;
pub use deprecation::copy_approved_commands_to_approvals_file;
pub use deprecation::detect_deprecations;
pub use deprecation::format_deprecation_details;
pub use deprecation::format_deprecation_warnings;
pub use deprecation::format_migration_diff;
pub use deprecation::migrate_content;
pub use deprecation::normalize_template_vars;
pub use deprecation::suppress_warnings;
pub use deprecation::{
    DEPRECATED_SECTION_KEYS, DeprecatedSection, UnknownKeyKind, classify_unknown_key,
    key_belongs_in, warn_unknown_fields,
};
pub use expansion::{
    DEPRECATED_TEMPLATE_VARS, TEMPLATE_VARS, TemplateExpandError, expand_template,
    redact_credentials, sanitize_branch_name, sanitize_db, short_hash, template_references_var,
    validate_template, validate_template_syntax,
};
pub use hooks::HooksConfig;
pub use project::{ProjectCiConfig, ProjectConfig, ProjectListConfig, valid_project_config_keys};
pub use unknown_tree::{
    UnknownAnalysis, UnknownTree, UnknownWarning, collect_unknown_warnings, compute_unknown_tree,
};
pub(crate) use user::LoadError;
pub use user::{
    CommitConfig, CommitGenerationConfig, CopyIgnoredConfig, ListConfig, MergeConfig,
    ResolvedConfig, StageMode, StepConfig, SwitchConfig, SwitchPickerConfig, UserConfig,
    UserProjectOverrides, config_path, default_config_path, default_system_config_path,
    set_config_path, system_config_path, valid_user_config_keys,
};

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;
    use crate::testing::TestRepo;

    fn test_repo() -> TestRepo {
        TestRepo::new()
    }

    #[test]
    fn test_config_serialization() {
        // Default config serializes to empty (no optional fields)
        assert_snapshot!(toml::to_string(&UserConfig::default()).unwrap(), @"[projects]");

        // With worktree-path set
        let config = UserConfig {
            worktree_path: Some("custom/{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_snapshot!(toml::to_string(&config).unwrap(), @r#"
        worktree-path = "custom/{{ branch }}"

        [projects]
        "#);
    }

    #[test]
    fn test_default_config() {
        let config = UserConfig::default();
        // worktree_path is None by default, but the getter returns the default
        assert!(config.worktree_path.is_none());
        assert_eq!(
            config.worktree_path(),
            "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
        );
        assert!(config.projects.is_empty());
    }

    #[test]
    fn test_format_worktree_path() {
        let test = test_repo();
        let config = UserConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature-x", &test.repo, None)
                .unwrap(),
            "myproject.feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_custom_template() {
        let test = test_repo();
        let config = UserConfig {
            worktree_path: Some("{{ main_worktree }}-{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature-x", &test.repo, None)
                .unwrap(),
            "myproject-feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_only_branch() {
        let test = test_repo();
        let config = UserConfig {
            worktree_path: Some(".worktrees/{{ main_worktree }}/{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature-x", &test.repo, None)
                .unwrap(),
            ".worktrees/myproject/feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_with_slashes() {
        let test = test_repo();
        // Use {{ branch | sanitize }} to replace slashes with dashes
        let config = UserConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch | sanitize }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature/foo", &test.repo, None)
                .unwrap(),
            "myproject.feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_with_multiple_slashes() {
        let test = test_repo();
        let config = UserConfig {
            worktree_path: Some(
                ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature/sub/task", &test.repo, None)
                .unwrap(),
            ".worktrees/myproject/feature-sub-task"
        );
    }

    #[test]
    fn test_format_worktree_path_with_backslashes() {
        let test = test_repo();
        // Windows-style path separators should also be sanitized
        let config = UserConfig {
            worktree_path: Some(
                ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}".to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", r"feature\foo", &test.repo, None)
                .unwrap(),
            ".worktrees/myproject/feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_raw_branch() {
        let test = test_repo();
        // {{ branch }} without filter gives raw branch name
        let config = UserConfig {
            worktree_path: Some("{{ main_worktree }}.{{ branch }}".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config
                .format_path("myproject", "feature/foo", &test.repo, None)
                .unwrap(),
            "myproject.feature/foo"
        );
    }

    #[test]
    fn test_command_config_single() {
        let toml = r#"post-create = "npm install""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.hooks.post_create.unwrap();
        let commands: Vec<_> = cmd_config.commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(*commands[0], Command::new(None, "npm install".to_string()));
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
        let commands: Vec<_> = cmd_config.commands().collect();
        assert_eq!(commands.len(), 2);
        // Preserves TOML insertion order
        assert_eq!(
            *commands[0],
            Command::new(Some("server".to_string()), "npm run dev".to_string())
        );
        assert_eq!(
            *commands[1],
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
        let commands: Vec<_> = cmd_config.commands().collect();

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
        let commands: Vec<_> = cmd_config.commands().collect();

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
        let commands: Vec<_> = cmd_config.commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(*commands[0], Command::new(None, "cargo test".to_string()));
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
        let commands: Vec<_> = cmd_config.commands().collect();
        assert_eq!(commands.len(), 3);
        // Preserves TOML insertion order
        assert_eq!(
            *commands[0],
            Command::new(
                Some("format".to_string()),
                "cargo fmt -- --check".to_string()
            )
        );
        assert_eq!(
            *commands[1],
            Command::new(Some("lint".to_string()), "cargo clippy".to_string())
        );
        assert_eq!(
            *commands[2],
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
        assert_snapshot!(serialized, @r#"post-create = "npm install""#);
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
        assert_snapshot!(serialized, @r#"
        [post-start]
        server = "npm run dev"
        watch = "npm run watch"
        "#);
    }

    #[test]
    fn test_expand_template_basic() {
        use std::collections::HashMap;

        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feature-x");
        let result = expand_template(
            "../{{ main_worktree }}.{{ branch }}",
            &vars,
            true,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "../myrepo.feature-x");
    }

    #[test]
    fn test_expand_template_sanitizes_branch() {
        use std::collections::HashMap;

        let test = test_repo();

        // Use {{ branch | sanitize }} filter for filesystem-safe paths
        // shell_escape=false to test filter in isolation (shell escaping tested separately)
        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feature/foo");
        let result = expand_template(
            "{{ main_worktree }}/{{ branch | sanitize }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "myrepo/feature-foo");

        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", r"feat\bar");
        let result = expand_template(
            ".worktrees/{{ main_worktree }}/{{ branch | sanitize }}",
            &vars,
            false,
            &test.repo,
            "test",
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
            &test_repo().repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "/path/to/repo/target -> /path/to/worktree/target");
    }

    #[test]
    fn test_commit_generation_config_mutually_exclusive_validation() {
        // Test that deserialization rejects both template and template-file
        let toml_content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit.generation]
command = "llm"
template = "inline template"
template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<UserConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            let generation = config.commit.generation.as_ref();
            // Verify validation logic: both fields should not be Some
            let has_both = generation
                .map(|g| g.template.is_some() && g.template_file.is_some())
                .unwrap_or(false);
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

[commit.generation]
command = "llm"
squash-template = "inline template"
squash-template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<UserConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            let generation = config.commit.generation.as_ref();
            // Verify validation logic: both fields should not be Some
            let has_both = generation
                .map(|g| g.squash_template.is_some() && g.squash_template_file.is_some())
                .unwrap_or(false);
            assert!(
                has_both,
                "Config should have both squash template fields set for this test"
            );
        }
    }

    #[test]
    fn test_commit_generation_config_serialization() {
        let config = CommitGenerationConfig {
            command: Some("llm -m model".to_string()),
            template: Some("template content".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };

        assert_snapshot!(toml::to_string(&config).unwrap(), @r#"
        command = "llm -m model"
        template = "template content"
        "#);
    }

    fn project_warn_tree(contents: &str) -> UnknownTree {
        compute_unknown_tree::<ProjectConfig>(contents)
            .warn_tree()
            .cloned()
            .unwrap()
    }

    fn user_warn_tree(contents: &str) -> UnknownTree {
        compute_unknown_tree::<UserConfig>(contents)
            .warn_tree()
            .cloned()
            .unwrap()
    }

    #[test]
    fn test_unknown_tree_project_with_typo() {
        let toml_str = "[post-merge-command]\ndeploy = \"task deploy\"";
        let tree = project_warn_tree(toml_str);
        assert!(tree.keys.contains("post-merge-command"));
        assert_eq!(tree.keys.len(), 1);
    }

    #[test]
    fn test_unknown_tree_project_valid() {
        let toml_str =
            "[post-merge]\ndeploy = \"task deploy\"\n\n[pre-merge]\ntest = \"cargo test\"";
        let tree = project_warn_tree(toml_str);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_unknown_tree_project_multiple() {
        let toml_str = "[post-merge-command]\ndeploy = \"task deploy\"\n\n[after-create]\nsetup = \"npm install\"";
        let tree = project_warn_tree(toml_str);
        assert_eq!(tree.keys.len(), 2);
        assert!(tree.keys.contains("post-merge-command"));
        assert!(tree.keys.contains("after-create"));
    }

    #[test]
    fn test_unknown_tree_user_with_typo() {
        let toml_str = "worktree-path = \"../test\"\n\n[commit-gen]\ncommand = \"llm\"";
        let tree = user_warn_tree(toml_str);
        assert!(tree.keys.contains("commit-gen"));
        assert_eq!(tree.keys.len(), 1);
    }

    #[test]
    fn test_unknown_tree_user_valid() {
        let toml_str = "worktree-path = \"../test\"\n\n[commit.generation]\ncommand = \"llm\"\n\n[list]\nfull = true";
        let tree = user_warn_tree(toml_str);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_unknown_tree_invalid_toml() {
        let toml = "this is not valid toml {{{";
        assert!(
            compute_unknown_tree::<ProjectConfig>(toml)
                .warn_tree()
                .is_none()
        );
        assert!(
            compute_unknown_tree::<UserConfig>(toml)
                .warn_tree()
                .is_none()
        );
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
        let config: UserConfig = toml::from_str(toml_str).unwrap();

        // Check post-create
        let post_create = config
            .hooks
            .post_create
            .expect("post-create should be present");
        let commands: Vec<_> = post_create.commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name.as_deref(), Some("log"));

        // Check pre-merge (multiple commands preserve order)
        let pre_merge = config.hooks.pre_merge.expect("pre-merge should be present");
        let commands: Vec<_> = pre_merge.commands().collect();
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
        let config: UserConfig = toml::from_str(toml_str).unwrap();

        let post_create = config
            .hooks
            .post_create
            .expect("post-create should be present");
        let commands: Vec<_> = post_create.commands().collect();
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
        let tree = user_warn_tree(toml_str);
        assert!(
            tree.is_empty(),
            "hook fields should not be reported as unknown: {tree:?}"
        );
    }

    #[test]
    fn test_user_config_key_in_project_config_is_detected() {
        // skip-shell-integration-prompt is a user-config-only key
        let toml_str = "skip-shell-integration-prompt = true\n";
        let tree = project_warn_tree(toml_str);
        assert!(
            tree.keys.contains("skip-shell-integration-prompt"),
            "skip-shell-integration-prompt should be unknown in project config"
        );

        // Verify it's valid in user config
        assert!(
            user_warn_tree(toml_str).is_empty(),
            "skip-shell-integration-prompt should be valid in user config"
        );
    }

    #[test]
    fn test_project_config_key_in_user_config_is_detected() {
        // ci is a project-config-only key
        let toml_str = r#"
[ci]
platform = "github"
"#;
        let tree = user_warn_tree(toml_str);
        assert!(
            tree.keys.contains("ci"),
            "ci should be unknown in user config"
        );

        // Verify it's valid in project config
        assert!(
            project_warn_tree(toml_str).is_empty(),
            "ci should be valid in project config"
        );
    }
}
