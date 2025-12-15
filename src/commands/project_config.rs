use worktrunk::config::{Command, ProjectConfig};
use worktrunk::git::HookType;

/// Collect commands for the given hook types, preserving order of the provided hooks.
pub fn collect_commands_for_hooks(
    project_config: &ProjectConfig,
    hooks: &[HookType],
) -> Vec<Command> {
    let mut commands = Vec::new();
    for hook in hooks {
        let cfg = match hook {
            HookType::PostCreate => &project_config.post_create,
            HookType::PostStart => &project_config.post_start,
            HookType::PreCommit => &project_config.pre_commit,
            HookType::PreMerge => &project_config.pre_merge,
            HookType::PostMerge => &project_config.post_merge,
            HookType::PreRemove => &project_config.pre_remove,
        };
        if let Some(config) = cfg {
            commands.extend(config.commands_with_phase(*hook));
        }
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_project_config_with_hooks() -> ProjectConfig {
        // Use TOML deserialization to create ProjectConfig
        let toml_content = r#"
post-create = "npm install"
pre-merge = "cargo test"
"#;
        toml::from_str(toml_content).unwrap()
    }

    #[test]
    fn test_collect_commands_for_hooks_empty_hooks() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[]);
        assert!(commands.is_empty());
    }

    #[test]
    fn test_collect_commands_for_hooks_single_hook() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[HookType::PostCreate]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].template, "npm install");
    }

    #[test]
    fn test_collect_commands_for_hooks_multiple_hooks() {
        let config = make_project_config_with_hooks();
        let commands =
            collect_commands_for_hooks(&config, &[HookType::PostCreate, HookType::PreMerge]);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].template, "npm install");
        assert_eq!(commands[1].template, "cargo test");
    }

    #[test]
    fn test_collect_commands_for_hooks_missing_hook() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[HookType::PostStart]);
        assert!(commands.is_empty());
    }

    #[test]
    fn test_collect_commands_for_hooks_order_preserved() {
        let config = make_project_config_with_hooks();
        // Order should match the order of hooks provided
        let commands =
            collect_commands_for_hooks(&config, &[HookType::PreMerge, HookType::PostCreate]);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].template, "cargo test");
        assert_eq!(commands[1].template, "npm install");
    }

    #[test]
    fn test_collect_commands_for_hooks_all_hook_types() {
        let config = ProjectConfig::default();
        // All hooks should work even when empty
        let hooks = [
            HookType::PostCreate,
            HookType::PostStart,
            HookType::PreCommit,
            HookType::PreMerge,
            HookType::PostMerge,
            HookType::PreRemove,
        ];
        let commands = collect_commands_for_hooks(&config, &hooks);
        assert!(commands.is_empty());
    }

    #[test]
    fn test_collect_commands_for_hooks_named_commands() {
        let toml_content = r#"
[post-create]
install = "npm install"
build = "npm run build"
"#;
        let config: ProjectConfig = toml::from_str(toml_content).unwrap();
        let commands = collect_commands_for_hooks(&config, &[HookType::PostCreate]);
        assert_eq!(commands.len(), 2);
        // Named commands preserve order from TOML
        assert_eq!(commands[0].name, Some("install".to_string()));
        assert_eq!(commands[1].name, Some("build".to_string()));
    }

    #[test]
    fn test_collect_commands_for_hooks_phase_is_set() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[HookType::PostCreate]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].phase, HookType::PostCreate);
    }
}
