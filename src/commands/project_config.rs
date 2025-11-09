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
            HookType::PostCreate => &project_config.post_create_command,
            HookType::PostStart => &project_config.post_start_command,
            HookType::PreCommit => &project_config.pre_commit_command,
            HookType::PreMerge => &project_config.pre_merge_command,
            HookType::PostMerge => &project_config.post_merge_command,
        };
        if let Some(config) = cfg {
            commands.extend(config.commands_with_phase(*hook));
        }
    }
    commands
}
