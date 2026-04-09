use super::*;
use crate::config::HooksConfig;
use crate::git::HookType;
use crate::testing::TestRepo;

fn test_repo() -> TestRepo {
    TestRepo::new()
}

#[test]
fn test_default_config_path_returns_platform_path() {
    // default_config_path() returns the platform-specific path without
    // CLI or env var overrides. Verify it returns a valid path.
    let path = default_config_path();
    assert!(path.is_some(), "default_config_path should return Some");
    let path = path.unwrap();
    assert!(
        path.ends_with("worktrunk/config.toml") || path.ends_with("worktrunk\\config.toml"),
        "Expected path ending in worktrunk/config.toml, got: {path:?}"
    );
}

#[test]
fn test_config_path_falls_through_to_default() {
    // When no CLI override or WORKTRUNK_CONFIG_PATH env var is set,
    // config_path() should fall through to default_config_path().
    // This also verifies both functions return the same path.
    let default = default_config_path().unwrap();
    let resolved = config_path().unwrap();
    assert_eq!(
        resolved, default,
        "config_path() should match default_config_path() when no overrides are set"
    );
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

[list]
full = true

[commit]
stage = "all"

[commit.generation]
command = "llm"

[merge]
squash = true

[step.copy-ignored]
exclude = [".conductor/"]

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
    };
    let toml = toml::to_string(&config).unwrap();
    insta::assert_snapshot!(toml, @r#"
    approved-commands = ["npm install"]
    worktree-path = ".worktrees/{{ branch | sanitize }}"
    "#);

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
        summary: None,
        task_timeout_ms: Some(500),
        timeout_ms: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: ListConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.full, Some(true));
    assert_eq!(parsed.branches, Some(false));
    assert_eq!(parsed.remotes, None);
    assert_eq!(parsed.summary, None);
    assert_eq!(parsed.task_timeout_ms, Some(500));
    assert_eq!(parsed.timeout_ms, None);
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
    assert!(!config.skip_shell_integration_prompt);
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
    let repo_path = test.repo.repo_path().unwrap().to_string_lossy();
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
    let repo_path = test.repo.repo_path().unwrap().to_string_lossy();
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
fn test_worktrunk_config_format_path_owner_variable() {
    let mut test = TestRepo::with_initial_commit();
    test.setup_remote("main");
    test.run_git(&[
        "remote",
        "set-url",
        "origin",
        "git@github.com:max-sixty/worktrunk.git",
    ]);

    let config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("{{ owner }}/{{ repo }}/{{ branch }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();

    assert_eq!(path, "max-sixty/myrepo/feature/branch");
}

#[test]
fn test_worktrunk_config_format_path_owner_uses_full_namespace() {
    let mut test = TestRepo::with_initial_commit();
    test.setup_remote("main");
    test.run_git(&[
        "remote",
        "set-url",
        "origin",
        "git@gitlab.com:group/subgroup/project.git",
    ]);

    let config = UserConfig {
        configs: OverridableConfig {
            worktree_path: Some("{{ owner }}/{{ repo }}/{{ branch }}".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let path = config
        .format_path("myrepo", "feature/branch", &test.repo, None)
        .unwrap();

    assert_eq!(path, "group/subgroup/myrepo/feature/branch");
}

#[test]
fn test_merge_config_serde() {
    let config = MergeConfig {
        squash: Some(true),
        commit: Some(true),
        rebase: Some(false),
        remove: Some(true),
        verify: Some(true),
        ff: None,
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

#[test]
fn test_set_project_worktree_path() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "# empty config\n").unwrap();

    let mut config = UserConfig::default();
    config
        .set_project_worktree_path(
            "github.com/user/repo",
            "../{{ branch | sanitize }}".to_string(),
            Some(&config_path),
        )
        .unwrap();

    assert_eq!(
        config.worktree_path_for_project("github.com/user/repo"),
        "../{{ branch | sanitize }}"
    );

    // Verify it was saved to disk
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("[projects.\"github.com/user/repo\"]"));
    assert!(content.contains("worktree-path"));
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
        summary: Some(true),
        task_timeout_ms: Some(1000),
        timeout_ms: Some(2000),
    };
    let override_config = ListConfig {
        full: None,            // Should fall back to base
        branches: Some(true),  // Should override
        remotes: Some(true),   // Should override (base was None)
        summary: None,         // Should fall back to base
        task_timeout_ms: None, // Should fall back to base
        timeout_ms: None,      // Should fall back to base
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.full, Some(true)); // From base
    assert_eq!(merged.branches, Some(true)); // From override
    assert_eq!(merged.remotes, Some(true)); // From override
    assert_eq!(merged.summary, Some(true)); // From base
    assert_eq!(merged.task_timeout_ms, Some(1000)); // From base
    assert_eq!(merged.timeout_ms, Some(2000)); // From base
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
        ff: Some(true),
    };
    let override_config = MergeConfig {
        squash: Some(false), // Override
        commit: None,        // Fall back to base
        rebase: None,        // Fall back to base
        remove: Some(false), // Override
        verify: None,        // Fall back to base
        ff: Some(false),     // Override
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.squash, Some(false));
    assert_eq!(merged.commit, Some(true));
    assert_eq!(merged.rebase, Some(true));
    assert_eq!(merged.remove, Some(false));
    assert_eq!(merged.verify, Some(true));
    assert_eq!(merged.ff, Some(false));
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
                ff: Some(true),
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
                    ff: None,
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
                    ..Default::default()
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
    assert!(config.task_timeout().is_none());
    assert!(config.timeout().is_none());
}

#[test]
fn test_list_config_accessor_methods_with_values() {
    let config = ListConfig {
        full: Some(true),
        branches: Some(true),
        remotes: Some(false),
        summary: Some(true),
        task_timeout_ms: Some(5000),
        timeout_ms: Some(3000),
    };
    assert!(config.full());
    assert!(config.branches());
    assert!(!config.remotes());
    assert!(config.summary());
    assert_eq!(
        config.task_timeout(),
        Some(std::time::Duration::from_millis(5000))
    );
    assert_eq!(
        config.timeout(),
        Some(std::time::Duration::from_millis(3000))
    );
}

#[test]
fn test_merge_config_accessor_methods_defaults() {
    let config = MergeConfig::default();
    // MergeConfig defaults are all true (including ff)
    assert!(config.squash());
    assert!(config.commit());
    assert!(config.rebase());
    assert!(config.remove());
    assert!(config.verify());
    assert!(config.ff());
}

#[test]
fn test_merge_config_accessor_methods_with_values() {
    let config = MergeConfig {
        squash: Some(false),
        commit: Some(false),
        rebase: Some(false),
        remove: Some(false),
        verify: Some(false),
        ff: Some(false),
    };
    assert!(!config.squash());
    assert!(!config.commit());
    assert!(!config.rebase());
    assert!(!config.remove());
    assert!(!config.verify());
    assert!(!config.ff());
}

#[test]
fn test_deprecated_no_ff_migrated_to_ff() {
    let config = UserConfig::load_from_str("[merge]\nno-ff = true\n").unwrap();
    assert!(!config.configs.merge.unwrap().ff());
}

#[test]
fn test_deprecated_no_ff_does_not_override_explicit_ff() {
    // If both `ff` and `no-ff` are set, `ff` wins (no-ff is ignored)
    let config = UserConfig::load_from_str("[merge]\nff = true\nno-ff = true\n").unwrap();
    assert!(config.configs.merge.unwrap().ff());
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

// =========================================================================
// SwitchPickerConfig tests
// =========================================================================

#[test]
fn test_switch_picker_config_accessor_methods() {
    use crate::config::user::SwitchPickerConfig;

    let config = SwitchPickerConfig::default();
    assert!(config.pager().is_none());
    // Default wall-clock budget is 500ms
    assert_eq!(
        config.timeout(),
        Some(std::time::Duration::from_millis(500))
    );

    let config = SwitchPickerConfig {
        pager: Some("delta --paging=never".to_string()),
        timeout_ms: Some(1000),
    };
    assert_eq!(config.pager(), Some("delta --paging=never"));
    assert_eq!(
        config.timeout(),
        Some(std::time::Duration::from_millis(1000))
    );
}

#[test]
fn test_switch_picker_timeout_zero_disables() {
    use crate::config::user::SwitchPickerConfig;

    let config = SwitchPickerConfig {
        timeout_ms: Some(0),
        ..Default::default()
    };
    assert!(config.timeout().is_none());
}

#[test]
fn test_switch_picker_timeout_none_uses_default() {
    use crate::config::user::SwitchPickerConfig;

    let config = SwitchPickerConfig::default();
    assert_eq!(
        config.timeout(),
        Some(std::time::Duration::from_millis(500))
    );
}

#[test]
fn test_switch_picker_config_parse_toml() {
    let content = r#"
[switch.picker]
pager = "delta --paging=never"
timeout-ms = 300
"#;
    let config: UserConfig = toml::from_str(content).unwrap();
    let picker = config
        .configs
        .switch
        .as_ref()
        .unwrap()
        .picker
        .as_ref()
        .unwrap();
    assert_eq!(picker.pager.as_deref(), Some("delta --paging=never"));
    assert_eq!(picker.timeout_ms, Some(300));
}

#[test]
fn test_switch_picker_merge() {
    use crate::config::user::{Merge, SwitchPickerConfig};

    let base = SwitchPickerConfig {
        pager: Some("delta".to_string()),
        timeout_ms: Some(500),
    };
    let override_config = SwitchPickerConfig {
        pager: None,         // Fall back to base
        timeout_ms: Some(0), // Override: disable timeout
    };

    let merged = base.merge_with(&override_config);
    assert_eq!(merged.pager.as_deref(), Some("delta"));
    assert_eq!(merged.timeout_ms, Some(0));
}

#[test]
fn test_switch_config_merge() {
    use crate::config::user::{Merge, SwitchConfig, SwitchPickerConfig};

    // Both have picker
    let base = SwitchConfig {
        picker: Some(SwitchPickerConfig {
            pager: Some("delta".to_string()),
            timeout_ms: None,
        }),
        ..Default::default()
    };
    let other = SwitchConfig {
        picker: Some(SwitchPickerConfig {
            pager: None,
            timeout_ms: Some(300),
        }),
        ..Default::default()
    };
    let merged = base.merge_with(&other);
    assert_eq!(
        merged.picker.as_ref().unwrap().pager.as_deref(),
        Some("delta")
    );
    assert_eq!(merged.picker.as_ref().unwrap().timeout_ms, Some(300));

    // Base has picker, other doesn't
    let other_none = SwitchConfig::default();
    let merged = base.merge_with(&other_none);
    assert_eq!(
        merged.picker.as_ref().unwrap().pager.as_deref(),
        Some("delta")
    );

    // Neither has picker
    let merged = SwitchConfig::default().merge_with(&other_none);
    assert!(merged.picker.is_none());
}

#[test]
fn test_switch_config_cd_accessor() {
    use crate::config::user::SwitchConfig;

    // Default is true
    let config = SwitchConfig::default();
    assert!(config.cd());

    // Explicit true
    let config = SwitchConfig {
        cd: Some(true),
        ..Default::default()
    };
    assert!(config.cd());

    // Explicit false
    let config = SwitchConfig {
        cd: Some(false),
        ..Default::default()
    };
    assert!(!config.cd());
}

#[test]
fn test_switch_config_cd_merge() {
    use crate::config::user::{Merge, SwitchConfig};

    // Other overrides base
    let base = SwitchConfig {
        cd: Some(true),
        ..Default::default()
    };
    let other = SwitchConfig {
        cd: Some(false),
        ..Default::default()
    };
    let merged = base.merge_with(&other);
    assert!(!merged.cd());

    // Base preserved when other is None
    let base = SwitchConfig {
        cd: Some(false),
        ..Default::default()
    };
    let merged = base.merge_with(&SwitchConfig::default());
    assert!(!merged.cd());

    // Neither set
    let merged = SwitchConfig::default().merge_with(&SwitchConfig::default());
    assert!(merged.cd()); // default true
}

#[test]
fn test_switch_config_cd_from_toml() {
    let toml = r#"
[switch]
cd = false
"#;
    let config = UserConfig::load_from_str(toml).unwrap();
    let switch = config.switch(None).unwrap();
    assert!(!switch.cd());
}

#[test]
fn test_switch_config_cd_resolved() {
    let toml = r#"
[switch]
cd = false
"#;
    let config = UserConfig::load_from_str(toml).unwrap();
    let resolved = config.resolved(None);
    assert!(!resolved.switch.cd());
}

#[test]
fn test_deprecated_no_cd_migrated_to_cd() {
    let config = UserConfig::load_from_str("[switch]\nno-cd = true\n").unwrap();
    assert!(!config.configs.switch.unwrap().cd());
}

#[test]
fn test_deprecated_no_cd_does_not_override_explicit_cd() {
    let config = UserConfig::load_from_str("[switch]\ncd = true\nno-cd = true\n").unwrap();
    assert!(config.configs.switch.unwrap().cd());
}

#[test]
fn test_switch_picker_fallback_from_select() {
    let config = UserConfig::load_from_str(
        r#"
[select]
pager = "bat"
"#,
    )
    .unwrap();

    let picker = config.switch_picker(None);
    assert_eq!(picker.pager.as_deref(), Some("bat"));
    // [select] is migrated to [switch.picker] at the TOML level before parsing
    assert_eq!(
        config
            .configs
            .switch
            .as_ref()
            .and_then(|switch| switch.picker.as_ref())
            .and_then(|picker| picker.pager.as_deref()),
        Some("bat")
    );
    // timeout_ms not available from select, so default applies
    assert_eq!(picker.timeout_ms, None);
    assert_eq!(
        picker.timeout(),
        Some(std::time::Duration::from_millis(500))
    );
}

#[test]
fn test_switch_picker_prefers_new_over_select() {
    let config = UserConfig::load_from_str(
        r#"
[switch.picker]
pager = "delta"
timeout-ms = 100

[select]
pager = "bat"
"#,
    )
    .unwrap();

    let picker = config.switch_picker(None);
    assert_eq!(picker.pager.as_deref(), Some("delta"));
    assert_eq!(picker.timeout_ms, Some(100));
}

#[test]
fn test_switch_picker_project_override() {
    use crate::config::user::{SwitchConfig, SwitchPickerConfig};

    let mut config = UserConfig {
        configs: OverridableConfig {
            switch: Some(SwitchConfig {
                picker: Some(SwitchPickerConfig {
                    pager: Some("delta".to_string()),
                    timeout_ms: Some(200),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    config.projects.insert(
        "github.com/user/repo".to_string(),
        UserProjectOverrides {
            overrides: OverridableConfig {
                switch: Some(SwitchConfig {
                    picker: Some(SwitchPickerConfig {
                        pager: Some("bat".to_string()),
                        timeout_ms: None, // Fall back to global
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let picker = config.switch_picker(Some("github.com/user/repo"));
    assert_eq!(picker.pager.as_deref(), Some("bat")); // From project
    assert_eq!(picker.timeout_ms, Some(200)); // From global
}

#[test]
fn test_switch_picker_project_fallback_from_select() {
    let config = UserConfig::load_from_str(
        r#"
[switch.picker]
pager = "delta"
timeout-ms = 300

[projects."github.com/user/repo".select]
pager = "bat"
"#,
    )
    .unwrap();

    let picker = config.switch_picker(Some("github.com/user/repo"));
    assert_eq!(picker.pager.as_deref(), Some("bat"));
    assert_eq!(picker.timeout_ms, Some(300));
    // [select] is migrated to [switch.picker] at the TOML level before parsing,
    // so it ends up in the switch.picker field, not select
    assert!(
        config
            .projects
            .get("github.com/user/repo")
            .unwrap()
            .overrides
            .switch
            .as_ref()
            .and_then(|s| s.picker.as_ref())
            .and_then(|p| p.pager.as_deref())
            == Some("bat")
    );
}

#[test]
fn test_resolved_config_for_project() {
    use crate::config::user::SwitchConfig;
    use crate::config::user::SwitchPickerConfig;

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
            switch: Some(SwitchConfig {
                picker: Some(SwitchPickerConfig {
                    pager: Some("less".to_string()),
                    timeout_ms: Some(300),
                }),
                ..Default::default()
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
    assert_eq!(resolved.switch_picker.pager(), Some("less"));
    assert_eq!(resolved.switch_picker.timeout_ms, Some(300));
    assert!(resolved.switch.cd()); // Default true
}

// =========================================================================
// Per-project config serde tests
// =========================================================================

#[test]
fn test_user_project_config_with_nested_configs_serde() {
    let config = UserProjectOverrides {
        approved_commands: vec!["npm install".to_string()],
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
fn test_copy_ignored_config_merges_global_and_project() {
    let project_id = "github.com/user/repo";
    let config = UserConfig::load_from_str(
        r#"
[step.copy-ignored]
exclude = [".conductor/", ".entire/"]

[projects."github.com/user/repo".step.copy-ignored]
exclude = [".repo-local/", ".entire/"]
"#,
    )
    .unwrap();

    let expected_global = vec![".conductor/".to_string(), ".entire/".to_string()];
    let expected_merged = vec![
        ".conductor/".to_string(),
        ".entire/".to_string(),
        ".repo-local/".to_string(),
    ];

    assert_eq!(config.copy_ignored(None).exclude, expected_global);
    assert_eq!(
        config.copy_ignored(Some(project_id)).exclude,
        expected_merged.clone()
    );
    assert_eq!(
        config
            .resolved(Some(project_id))
            .step
            .copy_ignored()
            .exclude,
        expected_merged
    );
}

#[test]
fn test_deprecated_commit_generation_migrated_on_load() {
    // [commit-generation] is migrated to [commit.generation] at the TOML level
    // before serde parsing, so it lands in configs.commit.generation
    let content = r#"
[commit-generation]
command = "llm -m claude-haiku-4.5"

[projects."github.com/user/repo".commit-generation]
command = "claude -p --model opus"
"#;

    let config = UserConfig::load_from_str(content).unwrap();

    assert_eq!(
        config
            .configs
            .commit
            .as_ref()
            .and_then(|commit| commit.generation.as_ref())
            .and_then(|generation| generation.command.as_deref()),
        Some("llm -m claude-haiku-4.5")
    );

    let project = config.projects.get("github.com/user/repo").unwrap();
    assert_eq!(
        project
            .overrides
            .commit
            .as_ref()
            .and_then(|commit| commit.generation.as_ref())
            .and_then(|generation| generation.command.as_deref()),
        Some("claude -p --model opus")
    );

    let effective_cg = config.commit_generation(Some("github.com/user/repo"));
    assert_eq!(
        effective_cg.command,
        Some("claude -p --model opus".to_string())
    );
}

#[test]
fn test_deprecated_commit_generation_with_args_field() {
    // Test that old format with args field is migrated: args merged into command
    let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;

    let config = UserConfig::load_from_str(content).unwrap();
    // Migration merges args into command and renames section
    assert_eq!(
        config
            .configs
            .commit
            .as_ref()
            .and_then(|c| c.generation.as_ref())
            .and_then(|g| g.command.as_deref()),
        Some("llm -m claude-haiku-4.5")
    );
}

// Validation tests

#[test]
fn test_validation_empty_worktree_path() {
    let content = r#"worktree-path = """#;
    let result = UserConfig::load_from_str(content);
    let err = result.unwrap_err().to_string();
    insta::assert_snapshot!(err, @"worktree-path cannot be empty");
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
    let err = result.unwrap_err().to_string();
    insta::assert_snapshot!(err, @"projects.github.com/user/repo.worktree-path cannot be empty");
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
    let cases = [
        ("[commit-generation]\ntemplate = \"inline\"\ntemplate-file = \"path\""),
        ("[commit-generation]\nsquash-template = \"inline\"\nsquash-template-file = \"path\""),
        ("[projects.\"github.com/user/repo\".commit-generation]\ntemplate = \"inline\"\ntemplate-file = \"path\""),
        ("[projects.\"github.com/user/repo\".commit-generation]\nsquash-template = \"inline\"\nsquash-template-file = \"path\""),
        ("[commit.generation]\ntemplate = \"inline\"\ntemplate-file = \"path\""),
        ("[commit.generation]\nsquash-template = \"inline\"\nsquash-template-file = \"path\""),
        ("[projects.\"github.com/user/repo\".commit.generation]\ntemplate = \"inline\"\ntemplate-file = \"path\""),
        ("[projects.\"github.com/user/repo\".commit.generation]\nsquash-template = \"inline\"\nsquash-template-file = \"path\""),
    ];
    for content in cases {
        let err = UserConfig::load_from_str(content).unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive"),
            "{content}: expected 'mutually exclusive', got: {err}"
        );
    }
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
    let commands: Vec<_> = post_start.commands().collect();
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
    let commands: Vec<_> = post_start.commands().collect();
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
    let commands: Vec<_> = post_start.commands().collect();
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
    let start_commands: Vec<_> = post_start.commands().collect();
    assert_eq!(start_commands.len(), 1);
    assert_eq!(start_commands[0].template, "echo global-start");

    // pre-commit: only project
    let pre_commit = effective.pre_commit.unwrap();
    let commit_commands: Vec<_> = pre_commit.commands().collect();
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
    let commands: Vec<_> = post_start.commands().collect();
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
            "list" | "commit" | "merge" | "switch" | "step" | "select" | "commit-generation"
            | "aliases" => {
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
    let commands: Vec<_> = effective.post_start.as_ref().unwrap().commands().collect();
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
    let commands: Vec<_> = effective.post_start.as_ref().unwrap().commands().collect();
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

    // Try to reload via a mutation — should fail with parse error
    let mut config = UserConfig::default();
    let result = config.set_skip_shell_integration_prompt(Some(&config_path));

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

// =========================================================================
// System config loading and merge tests
// =========================================================================

#[test]
fn test_system_config_merged_with_user_config() {
    // System config provides base defaults
    let system_toml = r#"
[merge]
squash = false
rebase = false

[list]
full = true
"#;

    // User config overrides some settings
    let user_toml = r#"
[merge]
squash = true
"#;

    // Parse both configs separately
    let system_config = UserConfig::load_from_str(system_toml).unwrap();
    let user_config = UserConfig::load_from_str(user_toml).unwrap();

    // Verify system config values
    assert_eq!(
        system_config.configs.merge.as_ref().unwrap().squash,
        Some(false)
    );
    assert_eq!(
        system_config.configs.merge.as_ref().unwrap().rebase,
        Some(false)
    );
    assert_eq!(
        system_config.configs.list.as_ref().unwrap().full,
        Some(true)
    );

    // Verify user config values
    assert_eq!(
        user_config.configs.merge.as_ref().unwrap().squash,
        Some(true)
    );

    // Simulate the merge that happens via the config crate's builder:
    // When both system and user configs define [merge], the config crate
    // performs a deep merge where user values override system values.
    // This is tested end-to-end via integration tests; here we verify
    // the Merge trait works correctly for the layering.
    let system_merge = system_config.configs.merge.as_ref().unwrap();
    let user_merge = user_config.configs.merge.as_ref().unwrap();
    let merged = system_merge.merge_with(user_merge);

    assert_eq!(merged.squash, Some(true)); // User overrides
    assert_eq!(merged.rebase, Some(false)); // System default preserved
}

#[test]
fn test_system_config_worktree_path_overridden_by_user() {
    let system_toml = r#"worktree-path = "/company/worktrees/{{ repo }}/{{ branch | sanitize }}""#;
    let user_toml = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}""#;

    let system_config = UserConfig::load_from_str(system_toml).unwrap();
    let user_config = UserConfig::load_from_str(user_toml).unwrap();

    assert_eq!(
        system_config.worktree_path(),
        "/company/worktrees/{{ repo }}/{{ branch | sanitize }}"
    );
    assert_eq!(
        user_config.worktree_path(),
        "../{{ repo }}.{{ branch | sanitize }}"
    );
}

#[test]
fn test_system_config_commit_generation_merged() {
    let system_toml = r#"
[commit.generation]
command = "company-llm-tool"
template = "Company standard template: {{ git_diff }}"
"#;
    let user_toml = r#"
[commit.generation]
command = "my-preferred-llm"
"#;

    let system_config = UserConfig::load_from_str(system_toml).unwrap();
    let user_config = UserConfig::load_from_str(user_toml).unwrap();

    let system_gen = system_config.commit_generation(None);
    assert_eq!(system_gen.command, Some("company-llm-tool".to_string()));
    assert_eq!(
        system_gen.template,
        Some("Company standard template: {{ git_diff }}".to_string())
    );

    let user_gen = user_config.commit_generation(None);
    assert_eq!(user_gen.command, Some("my-preferred-llm".to_string()));
    // User didn't set template, so in a merged scenario the system template
    // would be preserved via the config crate's deep merge
}

#[test]
fn test_hooks_merge_trait_appends_for_global_project_merge() {
    // The Merge trait uses append semantics — used for global→per-project merging
    // (in accessors.rs). NOT used for system→user config merging, which goes
    // through the config crate's replacement semantics instead.
    let global_hooks = parse_hooks("pre-merge = \"global-lint\"");
    let project_hooks = parse_hooks("pre-merge = \"project-lint\"");

    let merged = global_hooks.merge_with(&project_hooks);
    let pre_merge = merged.pre_merge.unwrap();
    let commands: Vec<_> = pre_merge.commands().collect();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].template, "global-lint"); // Global first
    assert_eq!(commands[1].template, "project-lint"); // Project second
}

#[test]
fn test_hooks_merge_folds_post_create_into_pre_start() {
    // User config uses deprecated `post-create`, project uses `pre-start`.
    // merge_with should combine them so the user's hook isn't silently dropped.
    let user_hooks = parse_hooks("post-create = \"npm install\"");
    let project_hooks = parse_hooks("pre-start = \"cargo test\"");

    let merged = user_hooks.merge_with(&project_hooks);
    let pre_start = merged
        .get(HookType::PreStart)
        .expect("should have pre-start");
    let commands: Vec<_> = pre_start.commands().collect();
    assert_eq!(commands.len(), 2, "Both hooks should be present");
    assert_eq!(commands[0].template, "npm install"); // User's post-create first
    assert_eq!(commands[1].template, "cargo test"); // Project's pre-start second
}

#[test]
fn test_hooks_merge_same_source_both_pre_start_and_post_create() {
    // Single config with both fields — merge_with folds post_create into pre_start.
    // This is an unusual config (user wrote both), but if it goes through merge
    // both commands should run rather than silently dropping one.
    let both = parse_hooks("pre-start = \"npm install\"\npost-create = \"cargo build\"");
    let empty = HooksConfig::default();

    let merged = both.merge_with(&empty);
    let pre_start = merged
        .get(HookType::PreStart)
        .expect("should have pre-start");
    let commands: Vec<_> = pre_start.commands().collect();
    assert_eq!(
        commands.len(),
        2,
        "Both commands from same source should be present"
    );
    assert_eq!(commands[0].template, "npm install"); // pre-start first
    assert_eq!(commands[1].template, "cargo build"); // post-create second
}

#[test]
fn test_hooks_merge_post_create_both_sides() {
    // Both configs use deprecated `post-create` — should still combine
    let global = parse_hooks("post-create = \"npm install\"");
    let project = parse_hooks("post-create = \"cargo build\"");

    let merged = global.merge_with(&project);
    let pre_start = merged
        .get(HookType::PreStart)
        .expect("should have pre-start");
    let commands: Vec<_> = pre_start.commands().collect();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].template, "npm install");
    assert_eq!(commands[1].template, "cargo build");
}

#[test]
fn test_aliases_accessor_appends_on_collision() {
    let toml_str = r#"
[aliases]
shared = "global-cmd"
global-only = "only-global"

[projects."test-project".aliases]
shared = "project-cmd"
project-only = "only-project"
"#;
    let config: UserConfig = toml::from_str(toml_str).unwrap();

    let aliases = config.aliases(Some("test-project"));

    // Non-colliding aliases are present
    assert_eq!(aliases["global-only"].commands().count(), 1);
    assert_eq!(
        aliases["global-only"].commands().next().unwrap().template,
        "only-global"
    );
    assert_eq!(aliases["project-only"].commands().count(), 1);
    assert_eq!(
        aliases["project-only"].commands().next().unwrap().template,
        "only-project"
    );

    // Colliding alias: both commands run (global first, then per-project)
    let shared: Vec<_> = aliases["shared"].commands().collect();
    assert_eq!(shared.len(), 2);
    assert_eq!(shared[0].template, "global-cmd");
    assert_eq!(shared[1].template, "project-cmd");

    // Without project: only global aliases
    let global_only = config.aliases(None);
    assert_eq!(global_only["shared"].commands().count(), 1);
    assert_eq!(
        global_only["shared"].commands().next().unwrap().template,
        "global-cmd"
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

    // Try to reload via a mutation — should fail with read error
    let mut config = UserConfig::default();
    let result = config.set_skip_shell_integration_prompt(Some(&config_path));

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
