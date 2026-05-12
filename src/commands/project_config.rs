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
/// Each `pre-remove` and `post-remove` reads the removed worktree's own
/// `.config/wt.toml` — both hooks are *about* that worktree, and at approval
/// time it's still on disk. `post-switch` reads the destination worktree's
/// config — the post-removal working directory the user lands in, which
/// `prepare_worktree_removal` records as [`super::worktree::RemoveResult`]'s
/// `main_path` (the primary worktree, except cwd when the primary worktree is
/// itself being removed; the merge destination for `wt merge`). Pass those
/// `destination_path()`s in — the gate must read the same config the executor
/// will (`output::handlers::spawn_hooks_after_remove`).
///
/// No fallback to another worktree's config, mirroring the executors
/// (`output::handlers::execute_pre_remove_hooks_if_needed` and the
/// `post-remove` snapshot path in `spawn_hooks_after_remove`). A
/// present-but-malformed worktree config surfaces as an error so the user
/// fixes it rather than silently running a different one.
///
/// Templates are deduped so the approval prompt shows each command once — so
/// `destination_paths` may contain duplicates (the common case: every removal
/// lands in the same primary worktree).
///
/// Callers feed the result into [`super::command_approval::approve_command_batch`].
/// `wt merge` prepends its own `pre-commit` / `post-commit` / `pre-merge` /
/// `post-merge` commands to the same batch; `wt remove` and `wt step prune`
/// approve the helper's output on its own.
pub fn collect_remove_hook_commands(
    removed_worktree_paths: &[&Path],
    destination_paths: &[&Path],
) -> anyhow::Result<Vec<ApprovableCommand>> {
    let mut commands: Vec<ApprovableCommand> = Vec::new();

    for &wt_path in removed_worktree_paths {
        // A `Repository::at` failure means git can't recognize the path as a
        // worktree — propagate rather than silently fall back. See #2708.
        let wt_repo = Repository::at(wt_path)?;
        if let Some(cfg) = wt_repo.load_project_config()? {
            commands.extend(collect_commands_for_hooks(
                &cfg,
                &[HookType::PreRemove, HookType::PostRemove],
            ));
        }
    }

    // `destination_paths` is usually one worktree repeated (every removal lands
    // in the same primary) — read each one's config at most once.
    let mut seen_dests: HashSet<&Path> = HashSet::new();
    for &dest_path in destination_paths {
        if !seen_dests.insert(dest_path) {
            continue;
        }
        // Propagate a `Repository::at` failure rather than silently skipping
        // — same as the removed-worktree loop above. See #2708.
        let dest_repo = Repository::at(dest_path)?;
        if let Some(cfg) = dest_repo.load_project_config()? {
            commands.extend(collect_commands_for_hooks(&cfg, &[HookType::PostSwitch]));
        }
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
