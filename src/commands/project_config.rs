use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use worktrunk::config::{Command, ProjectConfig};
use worktrunk::git::{HookType, Repository};

/// What triggered a project command — determines the label in approval prompts.
#[derive(Clone)]
pub enum Phase {
    Hook(HookType),
    Alias,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Hook(hook_type) => write!(f, "{hook_type}"),
            Phase::Alias => write!(f, "alias"),
        }
    }
}

/// A project-config command pending approval.
#[derive(Clone)]
pub struct ApprovableCommand {
    pub phase: Phase,
    pub command: Command,
}

/// Collect commands for the given hook types, preserving order of the provided hooks.
pub fn collect_commands_for_hooks(
    project_config: &ProjectConfig,
    hooks: &[HookType],
) -> Vec<ApprovableCommand> {
    let mut commands = Vec::new();
    for hook in hooks {
        if let Some(config) = project_config.hooks.get(*hook) {
            commands.extend(config.commands().cloned().map(|command| ApprovableCommand {
                phase: Phase::Hook(*hook),
                command,
            }));
        }
    }
    commands
}

/// Collect the project commands that run as part of removing a set of worktrees.
///
/// Each `pre-remove` reads the removed worktree's `.config/wt.toml`, falling
/// back to `primary_repo`'s config when the removed worktree carries none —
/// mirroring `output::handlers::execute_pre_remove_hooks_if_needed`, the
/// executor that runs the hook. `post-remove` and `post-switch` always read
/// `primary_repo`'s config (the removed worktree is gone by the time they
/// fire). For `wt merge`, `primary_repo` is the merge destination — same
/// fallback rule, different "primary".
///
/// Templates are deduped: when several removed worktrees fall back to
/// `primary_repo`'s config, the same `pre-remove` command would otherwise
/// appear in the approval prompt once per worktree.
///
/// Callers feed the result into [`super::command_approval::approve_command_batch`].
/// `wt merge` prepends its own `pre-commit` / `post-commit` / `pre-merge` /
/// `post-merge` commands to the same batch; `wt remove` and `wt step prune`
/// approve the helper's output on its own.
pub fn collect_remove_hook_commands(
    primary_repo: &Repository,
    removed_worktree_paths: &[&Path],
) -> anyhow::Result<Vec<ApprovableCommand>> {
    let mut commands: Vec<ApprovableCommand> = Vec::new();

    for &wt_path in removed_worktree_paths {
        let wt_repo = match Repository::at(wt_path) {
            Ok(r) if r.load_project_config().ok().flatten().is_some() => r,
            _ => primary_repo.clone(),
        };
        if let Some(cfg) = wt_repo.load_project_config()? {
            commands.extend(collect_commands_for_hooks(&cfg, &[HookType::PreRemove]));
        }
    }

    if let Some(cfg) = primary_repo.load_project_config()? {
        commands.extend(collect_commands_for_hooks(
            &cfg,
            &[HookType::PostRemove, HookType::PostSwitch],
        ));
    }

    let mut seen = HashSet::new();
    commands.retain(|cmd| seen.insert(cmd.command.template.clone()));

    Ok(commands)
}

/// Collect commands for every project-config alias, in `BTreeMap` (alphabetical) order.
///
/// Mirrors `approve_alias_commands` in `command_approval.rs`: unnamed steps within
/// an alias inherit the alias name, so users see a stable label in approval prompts.
pub fn collect_commands_for_aliases(project_config: &ProjectConfig) -> Vec<ApprovableCommand> {
    project_config
        .aliases
        .iter()
        .flat_map(|(alias_name, alias_cfg)| {
            alias_cfg.commands().map(move |cmd| ApprovableCommand {
                phase: Phase::Alias,
                command: Command::new(
                    Some(cmd.name.clone().unwrap_or_else(|| alias_name.clone())),
                    cmd.template.clone(),
                ),
            })
        })
        .collect()
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
        let commands = collect_commands_for_hooks(&config, &[HookType::PreStart]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command.template, "npm install");
    }

    #[test]
    fn test_collect_commands_for_hooks_multiple_hooks() {
        let config = make_project_config_with_hooks();
        let commands =
            collect_commands_for_hooks(&config, &[HookType::PreStart, HookType::PreMerge]);
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
            collect_commands_for_hooks(&config, &[HookType::PreMerge, HookType::PreStart]);
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
        let commands = collect_commands_for_hooks(&config, &[HookType::PreStart]);
        assert_eq!(commands.len(), 2);
        // Named commands preserve order from TOML
        assert_eq!(commands[0].command.name, Some("install".to_string()));
        assert_eq!(commands[1].command.name, Some("build".to_string()));
    }

    #[test]
    fn test_collect_commands_for_hooks_phase_is_set() {
        let config = make_project_config_with_hooks();
        let commands = collect_commands_for_hooks(&config, &[HookType::PreStart]);
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0].phase, Phase::Hook(HookType::PreStart)));
    }
}
