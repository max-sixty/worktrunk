use worktrunk::config::{Command, ProjectConfig};
use worktrunk::git::HookType;

#[derive(Clone)]
pub struct HookCommand {
    pub hook_type: HookType,
    pub command: Command,
}

/// Collect commands for the given hook types, preserving order of the provided hooks.
pub fn collect_commands_for_hooks(
    project_config: &ProjectConfig,
    hooks: &[HookType],
) -> Vec<HookCommand> {
    let mut commands = Vec::new();
    for hook in hooks {
        if let Some(config) = project_config.hooks.get(*hook) {
            commands.extend(
                config
                    .commands()
                    .iter()
                    .cloned()
                    .map(|command| HookCommand {
                        hook_type: *hook,
                        command,
                    }),
            );
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
        assert_eq!(commands[0].command.template, "npm install");
    }

    #[test]
    fn test_collect_commands_for_hooks_multiple_hooks() {
        let config = make_project_config_with_hooks();
        let commands =
            collect_commands_for_hooks(&config, &[HookType::PostCreate, HookType::PreMerge]);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].command.template, "npm install");
        assert_eq!(commands[1].command.template, "cargo test");
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
        assert_eq!(commands[0].command.template, "cargo test");
        assert_eq!(commands[1].command.template, "npm install");
    }

    #[test]
    fn test_collect_commands_for_hooks_all_hook_types() {
        use strum::IntoEnumIterator;

        let config = ProjectConfig::default();
        // All hooks should work even when empty
        let hooks: Vec<_> = HookType::iter().collect();
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
        assert_eq!(commands[0].command.name, Some("install".to_string()));
        assert_eq!(commands[1].command.name, Some("build".to_string()));
    }

    #[test]
    fn test_collect_commands_for_hooks_phase_is_set() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[HookType::PostCreate]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].hook_type, HookType::PostCreate);
    }
}
