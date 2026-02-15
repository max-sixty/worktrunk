use super::*;
use crate::config::HooksConfig;
use crate::git::Repository;

/// Test fixture that creates a real temporary git repository.
struct TestRepo {
    _dir: tempfile::TempDir,
    repo: Repository,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = Repository::at(dir.path()).unwrap();
        Self { _dir: dir, repo }
    }
}

fn test_repo() -> TestRepo {
    TestRepo::new()
}

#[test]
fn test_find_unknown_keys_empty() {
    // Valid config with no unknown keys
    let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
    let keys = find_unknown_keys(content);
    assert!(
        keys.is_empty(),
        "Expected no unknown keys, found: {:?}",
        keys
    );
}

#[test]
fn test_find_unknown_keys_with_unknown() {
    // Config with unknown top-level keys
    let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
unknown-key = "value"
another-unknown = 42
"#;
    let keys = find_unknown_keys(content);
    assert!(keys.contains_key("unknown-key"));
    assert!(keys.contains_key("another-unknown"));
}

#[test]
fn test_find_unknown_keys_known_sections() {
    // All known sections should not be reported
    let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"

[list]
full = true

[commit]
stage = "all"

[merge]
squash = true

[post-create]
run = "npm install"

[post-start]
run = "npm run build"

[post-switch]
rename-tab = "echo 'switched'"
"#;
    let keys = find_unknown_keys(content);
    assert!(keys.is_empty());
}

#[test]
fn test_commit_generation_config_is_configured_empty() {
    let config = CommitGenerationConfig::default();
    assert!(!config.is_configured());
}

#[test]
fn test_commit_generation_config_is_configured_with_command() {
    let config = CommitGenerationConfig {
        command: Some("llm".to_string()),
        ..Default::default()
    };
    assert!(config.is_configured());
}

#[test]
fn test_commit_generation_config_is_configured_with_whitespace_only() {
    let config = CommitGenerationConfig {
        command: Some("   ".to_string()),
        ..Default::default()
    };
    assert!(!config.is_configured());
}

#[test]
fn test_commit_generation_config_is_configured_with_empty_string() {
    let config = CommitGenerationConfig {
        command: Some("".to_string()),
        ..Default::default()
    };
    assert!(!config.is_configured());
}

#[test]
fn test_stage_mode_default() {
    assert_eq!(StageMode::default(), StageMode::All);
}

#[test]
fn test_stage_mode_serde() {
    // Test serialization
    let all_json = serde_json::to_string(&StageMode::All).unwrap();
    assert_eq!(all_json, "\"all\"");

    let tracked_json = serde_json::to_string(&StageMode::Tracked).unwrap();
    assert_eq!(tracked_json, "\"tracked\"");

    let none_json = serde_json::to_string(&StageMode::None).unwrap();
    assert_eq!(none_json, "\"none\"");

    // Test deserialization
    let all: StageMode = serde_json::from_str("\"all\"").unwrap();
    assert_eq!(all, StageMode::All);

    let tracked: StageMode = serde_json::from_str("\"tracked\"").unwrap();
    assert_eq!(tracked, StageMode::Tracked);

    let none: StageMode = serde_json::from_str("\"none\"").unwrap();
    assert_eq!(none, StageMode::None);
}

#[test]
fn test_user_project_config_default() {
    let config = UserProjectOverrides::default();
    assert!(config.overrides.worktree_path.is_none());
    assert!(config.approved_commands.is_empty());
}

#[test]
fn test_user_project_config_with_worktree_path_serde() {
    let config = UserProjectOverrides {
        overrides: OverridableConfig {
            worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
            ..Default::default()
        },
        approved_commands: vec!["npm install".to_string()],
        ..Default::default()
    };
    let toml = toml::to_string(&config).unwrap();
    assert!(toml.contains("worktree-path"));
    assert!(toml.contains(".worktrees/{{ branch | sanitize }}"));

    let parsed: UserProjectOverrides = toml::from_str(&toml).unwrap();
    assert_eq!(
        parsed.overrides.worktree_path,
        Some(".worktrees/{{ branch | sanitize }}".to_string())
    );
    assert_eq!(parsed.approved_commands, vec!["npm install".to_string()]);
}

#[test]
fn test_worktree_path_for_project_uses_project_specific() {
    let mut config = UserConfig::default();
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
                ..Default::default()
            },
            approved_commands: vec![],
            ..Default::default()
        },
    );

    // Project-specific path should be used
    assert_eq!(
        config.worktree_path_for_project("github.com/user/repo"),
        ".worktrees/{{ branch | sanitize }}"
    );
}

#[test]
fn test_worktree_path_for_project_falls_back_to_global() {
    let mut config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("../{{ repo }}-{{ branch | sanitize }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                worktree_path: None, // No project-specific path
                ..Default::default()
            },
            approved_commands: vec!["npm install".to_string()],
            ..Default::default()
        },
    );

    // Should fall back to global worktree-path
    assert_eq!(
        config.worktree_path_for_project("github.com/user/repo"),
        "../{{ repo }}-{{ branch | sanitize }}"
    );
}

#[test]
fn test_worktree_path_for_project_falls_back_to_default() {
    let config = UserConfig::default();

    // Unknown project should fall back to default template
    assert_eq!(
        config.worktree_path_for_project("github.com/unknown/project"),
        "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
    );
}

#[test]
fn test_format_path_with_project_override() {
    let test = test_repo();
    let mut config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("../{{ repo }}.{{ branch | sanitize }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
                ..Default::default()
            },
            approved_commands: vec![],
            ..Default::default()
        },
    );

    // With project identifier, should use project-specific template
    let path = config
        .format_path(
            "myrepo",
            "feature/branch",
            &test.repo,
            Some("github.com/user/repo"),
        )
        .unwrap();
    assert_eq!(path, ".worktrees/feature-branch");

    // Without project identifier, should use global template
    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();
    assert_eq!(path, "../myrepo.feature-branch");
}

#[test]
fn test_list_config_serde() {
    let config = ListConfig {
        full: Some(true),
        branches: Some(false),
        remotes: None,
        timeout_ms: Some(500),
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: ListConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.full, Some(true));
    assert_eq!(parsed.branches, Some(false));
    assert_eq!(parsed.remotes, None);
    assert_eq!(parsed.timeout_ms, Some(500));
}

#[test]
fn test_commit_config_default() {
    let config = CommitConfig::default();
    assert!(config.stage.is_none());
}

#[test]
fn test_worktrunk_config_default() {
    let config = UserConfig::default();
    // worktree_path is None by default, but the getter returns the default
    assert!(config.configs.worktree_path.is_none());
    assert_eq!(
        config.worktree_path(),
        "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
    );
    assert!(config.projects.is_empty());
    assert!(config.configs.list.is_none());
    assert!(config.configs.commit.is_none());
    assert!(config.configs.merge.is_none());
    assert!(config.commit_generation.is_none());
    assert!(!config.skip_shell_integration_prompt);
}

#[test]
fn test_worktrunk_config_is_command_approved_empty() {
    let config = UserConfig::default();
    assert!(!config.is_command_approved("some/project", "npm install"));
}

#[test]
fn test_worktrunk_config_is_command_approved_with_commands() {
    let mut config = UserConfig::default();
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            approved_commands: vec!["npm install".to_string(), "npm test".to_string()],
            ..Default::default()
        },
    );
    assert!(config.is_command_approved("github.com/user/repo", "npm install"));
    assert!(config.is_command_approved("github.com/user/repo", "npm test"));
    assert!(!config.is_command_approved("github.com/user/repo", "rm -rf /"));
    assert!(!config.is_command_approved("other/project", "npm install"));
}

#[test]
fn test_is_command_approved_normalizes_deprecated_vars() {
    // Approval saved with deprecated variable should match command with new variable
    let mut config = UserConfig::default();
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            approved_commands: vec![
                "ln -sf {{ repo_root }}/node_modules".to_string(), // old var
            ],
            ..Default::default()
        },
    );

    // Should match when checking with new variable name
    assert!(config.is_command_approved(
        "github.com/user/repo",
        "ln -sf {{ repo_path }}/node_modules" // new var
    ));

    // Should still match exact old name too
    assert!(config.is_command_approved(
        "github.com/user/repo",
        "ln -sf {{ repo_root }}/node_modules" // old var
    ));
}

#[test]
fn test_is_command_approved_normalizes_new_approval_matches_old_command() {
    // Approval saved with new variable should match command with deprecated variable
    let mut config = UserConfig::default();
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            approved_commands: vec![
                "cd {{ worktree_path }} && npm install".to_string(), // new var
            ],
            ..Default::default()
        },
    );

    // Should match when checking with old variable name
    assert!(config.is_command_approved(
        "github.com/user/repo",
        "cd {{ worktree }} && npm install" // old var
    ));
}

#[test]
fn test_is_command_approved_normalizes_multiple_vars() {
    let mut config = UserConfig::default();
    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            approved_commands: vec![
                "ln -sf {{ repo_root }}/modules {{ worktree }}/modules".to_string(),
            ],
            ..Default::default()
        },
    );

    // Should match with all new variable names
    assert!(config.is_command_approved(
        "github.com/user/repo",
        "ln -sf {{ repo_path }}/modules {{ worktree_path }}/modules"
    ));

    // Should match with mixed old/new (both normalize to same canonical form)
    assert!(config.is_command_approved(
        "github.com/user/repo",
        "ln -sf {{ repo_path }}/modules {{ worktree }}/modules"
    ));
}

#[test]
fn test_worktrunk_config_format_path() {
    let test = test_repo();
    let config = UserConfig::default();
    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();
    // Default path is now absolute: {{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}
    // The template uses forward slashes which work on all platforms
    // Check that the path contains the expected components
    assert!(
        path.contains("myrepo.feature-branch"),
        "Expected path containing 'myrepo.feature-branch', got: {path}"
    );
    // Verify it contains parent directory navigation
    assert!(
        path.contains("/..") || path.contains("\\.."),
        "Expected path containing parent navigation, got: {path}"
    );
    // The path should start with the repo path (absolute)
    let repo_path = test.repo.repo_path().to_string_lossy();
    assert!(
        path.starts_with(repo_path.as_ref()),
        "Expected path starting with repo path '{repo_path}', got: {path}"
    );
}

#[test]
fn test_worktrunk_config_format_path_custom_template() {
    let test = test_repo();
    let config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some(".worktrees/{{ branch }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let path = config
        .format_path("myrepo", "feature", &test.repo, None)
        .unwrap();
    assert_eq!(path, ".worktrees/feature");
}

#[test]
fn test_worktrunk_config_format_path_repo_path_variable() {
    let test = test_repo();
    let config = UserConfig {
        configs: OverridableConfig {
            // Use forward slashes in template (works on all platforms)
            worktree_path: Some("{{ repo_path }}/worktrees/{{ branch | sanitize }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();
    // Path should contain the expected components
    assert!(
        path.contains("worktrees") && path.contains("feature-branch"),
        "Expected path containing 'worktrees' and 'feature-branch', got: {path}"
    );
    // The path should start with the repo path
    let repo_path = test.repo.repo_path().to_string_lossy();
    assert!(
        path.starts_with(repo_path.as_ref()),
        "Expected path starting with repo path '{repo_path}', got: {path}"
    );
    // The path should be absolute since repo_path is absolute
    assert!(
        std::path::Path::new(&path).is_absolute() || path.starts_with('/'),
        "Expected absolute path, got: {path}"
    );
}

#[test]
fn test_worktrunk_config_format_path_tilde_expansion() {
    let test = test_repo();
    let config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("~/worktrees/{{ repo }}/{{ branch | sanitize }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();
    // Tilde should be expanded to home directory
    assert!(
        !path.starts_with('~'),
        "Tilde should be expanded, got: {path}"
    );
    // Path should contain expected components
    assert!(
        path.contains("worktrees") && path.contains("myrepo") && path.contains("feature-branch"),
        "Expected path containing 'worktrees/myrepo/feature-branch', got: {path}"
    );
    // Path should be absolute after tilde expansion
    assert!(
        std::path::Path::new(&path).is_absolute(),
        "Expected absolute path after tilde expansion, got: {path}"
    );
}

#[test]
fn test_merge_config_serde() {
    let config = MergeConfig {
        squash: Some(true),
        commit: Some(true),
        rebase: Some(false),
        remove: Some(true),
        verify: Some(true),
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: MergeConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.squash, Some(true));
    assert_eq!(parsed.rebase, Some(false));
}

#[test]
fn test_skip_shell_integration_prompt_default_false() {
    let config = UserConfig::default();
    assert!(!config.skip_shell_integration_prompt);
}

#[test]
fn test_skip_shell_integration_prompt_serde_roundtrip() {
    // Test serialization when true
    let config = UserConfig {
        skip_shell_integration_prompt: true,
        ..UserConfig::default()
    };
    let toml = toml::to_string(&config).unwrap();
    assert!(toml.contains("skip-shell-integration-prompt = true"));

    // Test deserialization
    let parsed: UserConfig = toml::from_str(&toml).unwrap();
    assert!(parsed.skip_shell_integration_prompt);
}

#[test]
fn test_skip_shell_integration_prompt_skipped_when_false() {
    // When false, the field should not appear in serialized output
    let config = UserConfig::default();
    let toml = toml::to_string(&config).unwrap();
    assert!(!toml.contains("skip-shell-integration-prompt"));
}

#[test]
fn test_skip_shell_integration_prompt_parsed_from_toml() {
    let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
skip-shell-integration-prompt = true
"#;
    let config: UserConfig = toml::from_str(content).unwrap();
    assert!(config.skip_shell_integration_prompt);
}

#[test]
fn test_skip_shell_integration_prompt_defaults_when_missing() {
    let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
    let config: UserConfig = toml::from_str(content).unwrap();
    assert!(!config.skip_shell_integration_prompt);
}

// =========================================================================
// Merge trait tests
// =========================================================================

#[test]
fn test_merge_list_config() {
    let base = ListConfig {
        full: Some(true),
        branches: Some(false),
        remotes: None,
        timeout_ms: Some(1000),
    };
    let override_config = ListConfig {
        full: None,           // Should fall back to base
        branches: Some(true), // Should override
        remotes: Some(true),  // Should override (base was None)
        timeout_ms: None,     // Should fall back to base
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.full, Some(true)); // From base
    assert_eq!(merged.branches, Some(true)); // From override
    assert_eq!(merged.remotes, Some(true)); // From override
    assert_eq!(merged.timeout_ms, Some(1000)); // From base
}

#[test]
fn test_merge_commit_config() {
    let base = CommitConfig {
        stage: Some(StageMode::All),
        generation: None,
    };
    let override_config = CommitConfig {
        stage: Some(StageMode::Tracked),
        generation: None,
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.stage, Some(StageMode::Tracked));
}

#[test]
fn test_merge_commit_config_generation_base_only() {
    // Base has generation, override doesn't - use base
    let base = CommitConfig {
        stage: None,
        generation: Some(CommitGenerationConfig {
            command: Some("base-llm".to_string()),
            ..Default::default()
        }),
    };
    let override_config = CommitConfig {
        stage: None,
        generation: None,
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(
        merged.generation.as_ref().unwrap().command,
        Some("base-llm".to_string())
    );
}

#[test]
fn test_merge_commit_config_generation_override_only() {
    // Override has generation, base doesn't - use override
    let base = CommitConfig {
        stage: None,
        generation: None,
    };
    let override_config = CommitConfig {
        stage: None,
        generation: Some(CommitGenerationConfig {
            command: Some("override-llm".to_string()),
            ..Default::default()
        }),
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(
        merged.generation.as_ref().unwrap().command,
        Some("override-llm".to_string())
    );
}

#[test]
fn test_merge_commit_config_generation_both() {
    // Both have generation - merge them
    let base = CommitConfig {
        stage: Some(StageMode::All),
        generation: Some(CommitGenerationConfig {
            command: Some("base-llm".to_string()),
            template: Some("base-template".to_string()),
            ..Default::default()
        }),
    };
    let override_config = CommitConfig {
        stage: None, // Will use base's stage
        generation: Some(CommitGenerationConfig {
            command: Some("override-llm".to_string()), // Override command
            template: None,                            // Use base's template
            ..Default::default()
        }),
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.stage, Some(StageMode::All));
    let generation = merged.generation.as_ref().unwrap();
    assert_eq!(generation.command, Some("override-llm".to_string()));
    assert_eq!(generation.template, Some("base-template".to_string()));
}

#[test]
fn test_merge_merge_config() {
    let base = MergeConfig {
        squash: Some(true),
        commit: Some(true),
        rebase: Some(true),
        remove: Some(true),
        verify: Some(true),
    };
    let override_config = MergeConfig {
        squash: Some(false), // Override
        commit: None,        // Fall back to base
        rebase: None,        // Fall back to base
        remove: Some(false), // Override
        verify: None,        // Fall back to base
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.squash, Some(false));
    assert_eq!(merged.commit, Some(true));
    assert_eq!(merged.rebase, Some(true));
    assert_eq!(merged.remove, Some(false));
    assert_eq!(merged.verify, Some(true));
}

#[test]
fn test_merge_commit_generation_config() {
    let base = CommitGenerationConfig {
        command: Some("llm -m claude-haiku-4.5".to_string()),
        template: None,
        template_file: Some("~/.config/template.txt".to_string()),
        squash_template: None,
        squash_template_file: None,
    };
    let override_config = CommitGenerationConfig {
        command: Some("claude -p --model=haiku".to_string()), // Override
        template: Some("custom".to_string()),                 // Override (was None)
        template_file: None,                                  // Fall back to base
        squash_template: None,
        squash_template_file: None,
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.command, Some("claude -p --model=haiku".to_string()));
    assert_eq!(merged.template, Some("custom".to_string()));
    // When project sets template, template_file is cleared to maintain mutual exclusivity
    assert_eq!(merged.template_file, None);
}

#[test]
fn test_commit_generation_merge_mutual_exclusivity() {
    // Global has template_file, project has template
    // Merged result should only have template (project wins, clears template_file)
    let global = CommitGenerationConfig {
        template_file: Some("~/.config/template.txt".to_string()),
        ..Default::default()
    };
    let project = CommitGenerationConfig {
        template: Some("inline template".to_string()),
        ..Default::default()
    };

    let merged = global.merge_with(&project);
    assert_eq!(merged.template, Some("inline template".to_string()));
    assert_eq!(merged.template_file, None); // Cleared because project set template

    // Reverse: global has template, project has template_file
    let global = CommitGenerationConfig {
        template: Some("global template".to_string()),
        ..Default::default()
    };
    let project = CommitGenerationConfig {
        template_file: Some("project-file.txt".to_string()),
        ..Default::default()
    };

    let merged = global.merge_with(&project);
    assert_eq!(merged.template, None); // Cleared because project set template_file
    assert_eq!(merged.template_file, Some("project-file.txt".to_string()));

    // Neither set in project: inherit both from global
    let global = CommitGenerationConfig {
        template: Some("global template".to_string()),
        ..Default::default()
    };
    let project = CommitGenerationConfig::default();

    let merged = global.merge_with(&project);
    assert_eq!(merged.template, Some("global template".to_string()));
    assert_eq!(merged.template_file, None);
}

#[test]
fn test_commit_generation_merge_squash_template_mutual_exclusivity() {
    // Global has squash_template_file, project has squash_template
    // Merged result should only have squash_template (project wins)
    let global = CommitGenerationConfig {
        squash_template_file: Some("~/.config/squash.txt".to_string()),
        ..Default::default()
    };
    let project = CommitGenerationConfig {
        squash_template: Some("inline squash".to_string()),
        ..Default::default()
    };

    let merged = global.merge_with(&project);
    assert_eq!(merged.squash_template, Some("inline squash".to_string()));
    assert_eq!(merged.squash_template_file, None);

    // Reverse: global has squash_template, project has squash_template_file
    let global = CommitGenerationConfig {
        squash_template: Some("global squash".to_string()),
        ..Default::default()
    };
    let project = CommitGenerationConfig {
        squash_template_file: Some("project-squash.txt".to_string()),
        ..Default::default()
    };

    let merged = global.merge_with(&project);
    assert_eq!(merged.squash_template, None);
    assert_eq!(
        merged.squash_template_file,
        Some("project-squash.txt".to_string())
    );
}

// =========================================================================
// Effective config methods tests
// =========================================================================

#[test]
fn test_effective_commit_generation_no_project() {
    let config = UserConfig {
        configs: OverridableConfig {
            commit: Some(CommitConfig {
                stage: None,
                generation: Some(CommitGenerationConfig {
                    command: Some("global-llm".to_string()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let effective = config.commit_generation(None);
    assert_eq!(effective.command, Some("global-llm".to_string()));
}

#[test]
fn test_effective_commit_generation_with_project_override() {
    let mut config = UserConfig {
        configs: OverridableConfig {
            commit: Some(CommitConfig {
                stage: None,
                generation: Some(CommitGenerationConfig {
                    command: Some("global-llm".to_string()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                commit: Some(CommitConfig {
                    stage: None,
                    generation: Some(CommitGenerationConfig {
                        command: Some("project-llm".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    // With project identifier, should merge project config
    let effective = config.commit_generation(Some("github.com/user/repo"));
    assert_eq!(effective.command, Some("project-llm".to_string()));

    // Without project or unknown project, should use global
    let effective = config.commit_generation(None);
    assert_eq!(effective.command, Some("global-llm".to_string()));

    let effective = config.commit_generation(Some("github.com/other/repo"));
    assert_eq!(effective.command, Some("global-llm".to_string()));
}

#[test]
fn test_effective_merge_with_partial_override() {
    let mut config = UserConfig {
        configs: OverridableConfig {
            merge: Some(MergeConfig {
                squash: Some(true),
                commit: Some(true),
                rebase: Some(true),
                remove: Some(true),
                verify: Some(true),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                merge: Some(MergeConfig {
                    squash: Some(false), // Only override squash
                    commit: None,
                    rebase: None,
                    remove: None,
                    verify: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let effective = config.merge(Some("github.com/user/repo")).unwrap();
    assert_eq!(effective.squash, Some(false)); // From project
    assert_eq!(effective.commit, Some(true)); // From global
    assert_eq!(effective.rebase, Some(true)); // From global
}

#[test]
fn test_effective_list_project_only() {
    // No global list config, only project config
    let mut config = UserConfig::default();
    assert!(config.configs.list.is_none());

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                list: Some(ListConfig {
                    full: Some(true),
                    branches: None,
                    remotes: None,
                    timeout_ms: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let effective = config.list(Some("github.com/user/repo")).unwrap();
    assert_eq!(effective.full, Some(true));
    assert!(effective.branches.is_none());

    // No global, no matching project = None
    assert!(config.list(Some("github.com/other/repo")).is_none());
}

#[test]
fn test_effective_select_with_project_override() {
    // Test that OverridableConfig merge works correctly for select
    let mut config = UserConfig {
        configs: OverridableConfig {
            select: Some(SelectConfig {
                pager: Some("delta".to_string()),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                select: Some(SelectConfig {
                    pager: Some("bat".to_string()),
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    // Project override takes precedence
    let effective = config.select(Some("github.com/user/repo")).unwrap();
    assert_eq!(effective.pager, Some("bat".to_string()));

    // No project override = use global
    let effective = config.select(Some("github.com/other/repo")).unwrap();
    assert_eq!(effective.pager, Some("delta".to_string()));

    // No project = use global
    let effective = config.select(None).unwrap();
    assert_eq!(effective.pager, Some("delta".to_string()));
}

#[test]
fn test_effective_commit_global_only() {
    // Only global config, no project config
    let config = UserConfig {
        configs: OverridableConfig {
            commit: Some(CommitConfig {
                stage: Some(StageMode::Tracked),
                generation: None,
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let effective = config.commit(Some("github.com/any/project")).unwrap();
    assert_eq!(effective.stage, Some(StageMode::Tracked));
}

// =========================================================================
// Config accessor methods and ResolvedConfig tests
// =========================================================================

#[test]
fn test_list_config_accessor_methods_defaults() {
    let config = ListConfig::default();
    assert!(!config.full());
    assert!(!config.branches());
    assert!(!config.remotes());
    assert!(config.timeout_ms().is_none());
}

#[test]
fn test_list_config_accessor_methods_with_values() {
    let config = ListConfig {
        full: Some(true),
        branches: Some(true),
        remotes: Some(false),
        timeout_ms: Some(5000),
    };
    assert!(config.full());
    assert!(config.branches());
    assert!(!config.remotes());
    assert_eq!(config.timeout_ms(), Some(5000));
}

#[test]
fn test_merge_config_accessor_methods_defaults() {
    let config = MergeConfig::default();
    // MergeConfig defaults are all true
    assert!(config.squash());
    assert!(config.commit());
    assert!(config.rebase());
    assert!(config.remove());
    assert!(config.verify());
}

#[test]
fn test_merge_config_accessor_methods_with_values() {
    let config = MergeConfig {
        squash: Some(false),
        commit: Some(false),
        rebase: Some(false),
        remove: Some(false),
        verify: Some(false),
    };
    assert!(!config.squash());
    assert!(!config.commit());
    assert!(!config.rebase());
    assert!(!config.remove());
    assert!(!config.verify());
}

#[test]
fn test_commit_config_accessor_methods() {
    let config = CommitConfig::default();
    assert_eq!(config.stage(), StageMode::All);

    let config = CommitConfig {
        stage: Some(StageMode::Tracked),
        generation: None,
    };
    assert_eq!(config.stage(), StageMode::Tracked);
}

#[test]
fn test_select_config_accessor_methods() {
    let config = SelectConfig::default();
    assert!(config.pager().is_none());

    let config = SelectConfig {
        pager: Some("delta --paging=never".to_string()),
    };
    assert_eq!(config.pager(), Some("delta --paging=never"));
}

#[test]
fn test_resolved_config_for_project() {
    let config = UserConfig {
        configs: OverridableConfig {
            list: Some(ListConfig {
                full: Some(true),
                ..Default::default()
            }),
            merge: Some(MergeConfig {
                squash: Some(false),
                ..Default::default()
            }),
            commit: Some(CommitConfig {
                stage: Some(StageMode::None),
                ..Default::default()
            }),
            select: Some(SelectConfig {
                pager: Some("less".to_string()),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let resolved = config.resolved(None);

    // Test that accessor methods work through ResolvedConfig
    assert!(resolved.list.full());
    assert!(!resolved.list.branches()); // Default
    assert!(!resolved.merge.squash()); // Overridden to false
    assert!(resolved.merge.commit()); // Default true
    assert_eq!(resolved.commit.stage(), StageMode::None);
    assert_eq!(resolved.select.pager(), Some("less"));
}

// =========================================================================
// Per-project config serde tests
// =========================================================================

#[test]
fn test_user_project_config_with_nested_configs_serde() {
    let config = UserProjectOverrides {
        approved_commands: vec!["npm install".to_string()],
        commit_generation: None, // Deprecated field, use commit.generation instead
        overrides: OverridableConfig {
            worktree_path: Some(".worktrees/{{ branch }}".to_string()),
            list: Some(ListConfig {
                full: Some(true),
                ..Default::default()
            }),
            commit: Some(CommitConfig {
                stage: Some(StageMode::Tracked),
                generation: Some(CommitGenerationConfig {
                    command: Some("llm -m gpt-4".to_string()),
                    ..Default::default()
                }),
            }),
            merge: Some(MergeConfig {
                squash: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
    };

    let toml = toml::to_string(&config).unwrap();
    let parsed: UserProjectOverrides = toml::from_str(&toml).unwrap();

    assert_eq!(
        parsed.overrides.worktree_path,
        Some(".worktrees/{{ branch }}".to_string())
    );
    assert_eq!(
        parsed
            .overrides
            .commit
            .as_ref()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .command,
        Some("llm -m gpt-4".to_string())
    );
    assert_eq!(parsed.overrides.list.as_ref().unwrap().full, Some(true));
    assert_eq!(
        parsed.overrides.commit.as_ref().unwrap().stage,
        Some(StageMode::Tracked)
    );
    assert_eq!(parsed.overrides.merge.as_ref().unwrap().squash, Some(false));
}

#[test]
fn test_full_config_with_per_project_sections_serde() {
    // Test new format: [commit.generation] instead of [commit-generation]
    let content = r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[commit.generation]
command = "llm -m claude-haiku-4.5"

[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
approved-commands = ["npm install"]

[projects."github.com/user/repo".commit.generation]
command = "claude -p --model opus"

[projects."github.com/user/repo".list]
full = true

[projects."github.com/user/repo".merge]
squash = false
"#;

    let config: UserConfig = toml::from_str(content).unwrap();

    // Global config
    assert_eq!(
        config.configs.worktree_path,
        Some("../{{ repo }}.{{ branch | sanitize }}".to_string())
    );
    assert_eq!(
        config
            .configs
            .commit
            .as_ref()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .command,
        Some("llm -m claude-haiku-4.5".to_string())
    );

    // Project config
    let project = config.projects.get("github.com/user/repo").unwrap();
    assert_eq!(
        project.overrides.worktree_path,
        Some(".worktrees/{{ branch | sanitize }}".to_string())
    );
    assert_eq!(
        project
            .overrides
            .commit
            .as_ref()
            .unwrap()
            .generation
            .as_ref()
            .unwrap()
            .command,
        Some("claude -p --model opus".to_string())
    );
    assert_eq!(project.overrides.list.as_ref().unwrap().full, Some(true));
    assert_eq!(
        project.overrides.merge.as_ref().unwrap().squash,
        Some(false)
    );

    // Effective config for project
    let effective_cg = config.commit_generation(Some("github.com/user/repo"));
    assert_eq!(
        effective_cg.command,
        Some("claude -p --model opus".to_string())
    );

    let effective_merge = config.merge(Some("github.com/user/repo")).unwrap();
    assert_eq!(effective_merge.squash, Some(false));
}

#[test]
fn test_deprecated_commit_generation_format_serde() {
    // Test old format: [commit-generation] is still parsed for backward compatibility
    let content = r#"
[commit-generation]
command = "llm -m claude-haiku-4.5"

[projects."github.com/user/repo".commit-generation]
command = "claude -p --model opus"
"#;

    let config: UserConfig = toml::from_str(content).unwrap();

    // Old format parsed into commit_generation field
    assert_eq!(
        config.commit_generation.as_ref().unwrap().command,
        Some("llm -m claude-haiku-4.5".to_string())
    );

    // Project override uses deprecated field
    let project = config.projects.get("github.com/user/repo").unwrap();
    assert_eq!(
        project.commit_generation.as_ref().unwrap().command,
        Some("claude -p --model opus".to_string())
    );

    // Effective config uses the deprecated values
    let effective_cg = config.commit_generation(Some("github.com/user/repo"));
    assert_eq!(
        effective_cg.command,
        Some("claude -p --model opus".to_string())
    );
}

#[test]
fn test_deprecated_commit_generation_with_args_field() {
    // Test that old format with args field still parses (args is ignored)
    // This ensures backward compatibility for users who haven't migrated yet
    let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;

    let result: Result<UserConfig, _> = toml::from_str(content);
    assert!(
        result.is_ok(),
        "Old format with args field should parse (args is ignored): {:?}",
        result.err()
    );

    let config = result.unwrap();
    // Command is parsed, args is ignored (struct no longer has args field)
    assert_eq!(
        config.commit_generation.as_ref().unwrap().command,
        Some("llm".to_string())
    );
}

// Validation tests

#[test]
fn test_validation_empty_worktree_path() {
    let content = r#"worktree-path = """#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("worktree-path cannot be empty"), "{err}");
}

#[test]
fn test_validation_absolute_worktree_path_allowed() {
    // Absolute paths should be allowed for worktree-path
    let content = if cfg!(windows) {
        r#"worktree-path = "C:\\worktrees\\{{ branch | sanitize }}""#
    } else {
        r#"worktree-path = "/worktrees/{{ branch | sanitize }}""#
    };
    let result = UserConfig::load_from_str(content);
    assert!(
        result.is_ok(),
        "Absolute paths should be allowed: {:?}",
        result.err()
    );
}

#[test]
fn test_validation_project_empty_worktree_path() {
    let content = r#"
[projects."github.com/user/repo"]
worktree-path = ""
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("worktree-path cannot be empty"), "{err}");
}

#[test]
fn test_validation_project_absolute_worktree_path_allowed() {
    // Absolute paths should be allowed for per-project worktree-path
    let content = if cfg!(windows) {
        r#"
[projects."github.com/user/repo"]
worktree-path = "C:\\worktrees\\{{ branch | sanitize }}"
"#
    } else {
        r#"
[projects."github.com/user/repo"]
worktree-path = "/worktrees/{{ branch | sanitize }}"
"#
    };
    let result = UserConfig::load_from_str(content);
    assert!(
        result.is_ok(),
        "Absolute paths should be allowed: {:?}",
        result.err()
    );
}

#[test]
fn test_validation_template_mutual_exclusivity() {
    let content = r#"
[commit-generation]
template = "inline template"
template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_squash_template_mutual_exclusivity() {
    let content = r#"
[commit-generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_project_template_mutual_exclusivity() {
    let content = r#"
[projects."github.com/user/repo".commit-generation]
template = "inline template"
template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_project_squash_template_mutual_exclusivity() {
    let content = r#"
[projects."github.com/user/repo".commit-generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

// New format [commit.generation] validation tests

#[test]
fn test_validation_new_format_template_mutual_exclusivity() {
    let content = r#"
[commit.generation]
template = "inline template"
template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_new_format_squash_template_mutual_exclusivity() {
    let content = r#"
[commit.generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_new_format_project_template_mutual_exclusivity() {
    let content = r#"
[projects."github.com/user/repo".commit.generation]
template = "inline template"
template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn test_validation_new_format_project_squash_template_mutual_exclusivity() {
    let content = r#"
[projects."github.com/user/repo".commit.generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
    let result = UserConfig::load_from_str(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("mutually exclusive"), "{err}");
}

// =========================================================================
// save_to() tests
// =========================================================================

#[test]
fn test_save_to_new_file_with_commit_generation() {
    // Test that save_to() creates a new file with commit.generation section
    // This exercises the "create from scratch" branch when no existing file exists
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let config = UserConfig {
        configs: OverridableConfig {
            commit: Some(CommitConfig {
                stage: None,
                generation: Some(CommitGenerationConfig {
                    command: Some("llm -m haiku".to_string()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.save_to(&config_path).unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("[commit.generation]"),
        "Should use new format: {saved}"
    );
    assert!(
        saved.contains("command = \"llm -m haiku\""),
        "Should contain command: {saved}"
    );
    // When only generation is set (no stage), [commit] header should be implicit
    assert!(
        !saved.contains("[commit]\n"),
        "Should not have standalone [commit] header when only generation is set: {saved}"
    );
}

#[test]
fn test_save_to_new_file_commit_with_stage_and_generation() {
    // Test that when both stage and generation are set, [commit] header is explicit
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let config = UserConfig {
        configs: OverridableConfig {
            commit: Some(CommitConfig {
                stage: Some(StageMode::Tracked),
                generation: Some(CommitGenerationConfig {
                    command: Some("llm -m haiku".to_string()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.save_to(&config_path).unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("[commit]\n"),
        "Should have [commit] header when stage is set: {saved}"
    );
    assert!(
        saved.contains("stage = \"tracked\""),
        "Should contain stage: {saved}"
    );
    assert!(
        saved.contains("[commit.generation]"),
        "Should have generation section: {saved}"
    );
}

#[test]
fn test_save_to_new_file_with_deprecated_commit_generation() {
    // Test that save_to() serializes deprecated commit_generation field
    // (for backward compat when loading old configs and re-saving)
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let config = UserConfig {
        commit_generation: Some(CommitGenerationConfig {
            command: Some("old-llm".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    config.save_to(&config_path).unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("[commit-generation]"),
        "Should use deprecated format: {saved}"
    );
    assert!(
        saved.contains("command = \"old-llm\""),
        "Should contain command: {saved}"
    );
}

#[test]
fn test_save_to_new_file_with_skip_shell_integration() {
    // Test skip-shell-integration-prompt is only written when true
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let config = UserConfig {
        skip_shell_integration_prompt: true,
        ..Default::default()
    };

    config.save_to(&config_path).unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("skip-shell-integration-prompt = true"),
        "Should contain flag: {saved}"
    );
}

#[test]
fn test_save_to_new_file_with_worktree_path() {
    // Test worktree-path is written when set
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("../{{ repo }}.{{ branch }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    config.save_to(&config_path).unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("worktree-path = \"../{{ repo }}.{{ branch }}\""),
        "Should contain worktree-path: {saved}"
    );
}

// =========================================================================
// Per-project hooks tests (append semantics)
// =========================================================================

/// Helper to parse hooks from TOML
fn parse_hooks(toml_str: &str) -> HooksConfig {
    toml::from_str(toml_str).unwrap()
}

#[test]
fn test_hooks_merge_append_semantics() {
    // Global has post-start, per-project has post-start
    // Both should run (global first, then per-project)
    let mut config = UserConfig {
        configs: OverridableConfig {
            hooks: parse_hooks("post-start = \"echo global\""),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo project\""),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let effective = config.hooks(Some("github.com/user/repo"));
    let post_start = effective.post_start.unwrap();
    let commands = post_start.commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].template, "echo global");
    assert_eq!(commands[1].template, "echo project");
}

#[test]
fn test_hooks_no_project_override_uses_global() {
    // Global has hooks, project doesn't - global hooks used
    let config = UserConfig {
        configs: OverridableConfig {
            hooks: parse_hooks("post-start = \"echo global\""),
            ..Default::default()
        },
        ..Default::default()
    };

    let effective = config.hooks(Some("github.com/other/repo"));
    let post_start = effective.post_start.unwrap();
    let commands = post_start.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].template, "echo global");
}

#[test]
fn test_hooks_project_only_no_global() {
    // Project has hooks, global doesn't - project hooks used
    let mut config = UserConfig::default();

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo project\""),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let effective = config.hooks(Some("github.com/user/repo"));
    let post_start = effective.post_start.unwrap();
    let commands = post_start.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].template, "echo project");
}

#[test]
fn test_hooks_different_hook_types_not_merged() {
    // Global has post-start, per-project has pre-commit
    // These should remain separate (different hook types)
    let mut config = UserConfig {
        configs: OverridableConfig {
            hooks: parse_hooks("post-start = \"echo global-start\""),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                hooks: parse_hooks("pre-commit = \"echo project-commit\""),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let effective = config.hooks(Some("github.com/user/repo"));

    // post-start: only global
    let post_start = effective.post_start.unwrap();
    let start_commands = post_start.commands();
    assert_eq!(start_commands.len(), 1);
    assert_eq!(start_commands[0].template, "echo global-start");

    // pre-commit: only project
    let pre_commit = effective.pre_commit.unwrap();
    let commit_commands = pre_commit.commands();
    assert_eq!(commit_commands.len(), 1);
    assert_eq!(commit_commands[0].template, "echo project-commit");
}

#[test]
fn test_hooks_none_project_uses_global() {
    // When no project is provided, only global hooks are used
    let config = UserConfig {
        configs: OverridableConfig {
            hooks: parse_hooks("post-start = \"echo global\""),
            ..Default::default()
        },
        ..Default::default()
    };

    let effective = config.hooks(None);
    let post_start = effective.post_start.unwrap();
    let commands = post_start.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].template, "echo global");
}

#[test]
fn test_hooks_in_overridable_config_is_empty() {
    // Default hooks should be considered empty
    let config = OverridableConfig::default();
    assert!(config.is_empty());

    // With hooks set, should not be empty
    let config = OverridableConfig {
        hooks: parse_hooks("post-start = \"echo test\""),
        ..Default::default()
    };
    assert!(!config.is_empty());
}

/// Validates that valid_user_config_keys() includes all hook types from HookType enum.
///
/// The JsonSchema derivation should include all HooksConfig fields, which correspond
/// to HookType variants. HookType uses strum's Display with kebab-case serialization,
/// which matches the serde field names.
#[test]
fn test_valid_user_config_keys_includes_all_hook_types() {
    use crate::git::HookType;
    use strum::IntoEnumIterator;

    let valid_keys = valid_user_config_keys();

    for hook_type in HookType::iter() {
        let key = hook_type.to_string(); // e.g., "post-create", "pre-merge"
        assert!(
            valid_keys.contains(&key),
            "HookType::{hook_type:?} ({key}) is missing from valid_user_config_keys()"
        );
    }
}

/// Validates that all keys from valid_user_config_keys() are accepted by serde.
///
/// Creates a TOML config with each key set to a valid value and verifies
/// deserialization succeeds. This ensures the JsonSchema matches serde's expectations.
#[test]
fn test_valid_user_config_keys_all_deserialize() {
    let valid_keys = valid_user_config_keys();

    // Build a TOML string with all keys
    // Top-level scalar values must come before table sections
    let mut scalar_lines = Vec::new();
    let mut table_lines = Vec::new();

    for key in &valid_keys {
        match key.as_str() {
            "projects" => continue, // Skip - table type tested separately
            "skip-shell-integration-prompt" | "skip-commit-generation-prompt" => {
                scalar_lines.push(format!("{key} = true"));
            }
            "worktree-path" => {
                scalar_lines.push(format!("{key} = \"test-value\""));
            }
            "list" | "commit" | "merge" | "select" | "commit-generation" => {
                // Table sections with minimal content
                table_lines.push(format!("[{key}]"));
            }
            // Hook keys take string values
            _ => {
                scalar_lines.push(format!("{key} = \"test-value\""));
            }
        };
    }

    // Scalars first, then tables
    scalar_lines.extend(table_lines);
    let toml_content = scalar_lines.join("\n");

    // Should deserialize without error
    let result: Result<UserConfig, _> = toml::from_str(&toml_content);
    assert!(
        result.is_ok(),
        "Failed to deserialize config with all valid keys:\n{toml_content}\nError: {:?}",
        result.err()
    );
}

// =========================================================================
// Hooks Merge Behavior Tests
// =========================================================================
//
// Note: Merged configs are only used for execution, never serialized in
// production. These tests verify merge semantics for execution order.

/// Merging string-format global hooks with table-format per-project hooks
/// preserves both and maintains correct execution order.
#[test]
fn test_hooks_merge_mixed_formats_preserves_order() {
    // Global uses string format (unnamed command)
    let global_hooks = parse_hooks(r#"post-start = "npm install""#);

    // Per-project uses table format (named commands)
    let project_hooks = parse_hooks(
        r#"
[post-start]
setup = "echo setup"
"#,
    );

    let mut config = UserConfig {
        configs: OverridableConfig {
            hooks: global_hooks,
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                hooks: project_hooks,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    // Verify merge preserves order: global first, then project
    let effective = config.hooks(Some("github.com/user/repo"));
    let commands = effective.post_start.as_ref().unwrap().commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].template, "npm install"); // Global first
    assert_eq!(commands[1].template, "echo setup"); // Project second
}

/// When global and per-project both define same hook type, both run in order.
#[test]
fn test_hooks_merge_same_names_both_run() {
    // Both define "test" command - both should execute
    let global_hooks = parse_hooks(
        r#"
[post-start]
test = "cargo test"
"#,
    );

    let project_hooks = parse_hooks(
        r#"
[post-start]
test = "npm test"
"#,
    );

    let mut config = UserConfig {
        configs: OverridableConfig {
            hooks: global_hooks,
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                hooks: project_hooks,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    // Both commands present, global first
    let effective = config.hooks(Some("github.com/user/repo"));
    let commands = effective.post_start.as_ref().unwrap().commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].template, "cargo test");
    assert_eq!(commands[1].template, "npm test");
}

// =========================================================================
// reload_projects_from error path tests
// =========================================================================

/// Test that reload_projects_from returns a parse error with formatted path
/// when the config file contains invalid TOML.
#[test]
fn test_reload_projects_from_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    // Create initial valid config so file exists
    std::fs::write(&config_path, "# Valid config\n").unwrap();

    // Now corrupt it with invalid TOML
    std::fs::write(&config_path, "this is not valid toml [[[").unwrap();

    // Try to reload - should fail with parse error
    let mut config = UserConfig::default();
    let result = config.approve_command(
        "github.com/test/repo".to_string(),
        "echo test".to_string(),
        Some(&config_path),
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Failed to parse config file"),
        "Expected parse error, got: {err}"
    );
    // Verify path is included in error (format_path_for_display would format it)
    assert!(
        err.contains("config.toml"),
        "Expected path in error, got: {err}"
    );
}

/// Test that reload_projects_from handles permission errors
/// when the config file exists but cannot be read.
#[cfg(unix)]
#[test]
fn test_reload_projects_from_permission_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    // Create a valid config file
    std::fs::write(&config_path, "[projects]\n").unwrap();

    // Remove read permissions
    let mut perms = std::fs::metadata(&config_path).unwrap().permissions();
    perms.set_mode(0o000); // No permissions
    std::fs::set_permissions(&config_path, perms).unwrap();

    // Restore permissions on drop to allow cleanup
    struct RestorePerms<'a>(&'a std::path::Path);
    impl Drop for RestorePerms<'_> {
        fn drop(&mut self) {
            let mut perms = std::fs::metadata(self.0).unwrap().permissions();
            perms.set_mode(0o644);
            let _ = std::fs::set_permissions(self.0, perms);
        }
    }
    let _guard = RestorePerms(&config_path);

    // Skip this test when running as root (common in CI containers)
    if std::env::var("USER").as_deref() == Ok("root") {
        return;
    }

    // Try to reload - should fail with read error
    let mut config = UserConfig::default();
    let result = config.approve_command(
        "github.com/test/repo".to_string(),
        "echo test".to_string(),
        Some(&config_path),
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Failed to read config file"),
        "Expected read error, got: {err}"
    );
    // Verify path is included in error
    assert!(
        err.contains("config.toml"),
        "Expected path in error, got: {err}"
    );
}
