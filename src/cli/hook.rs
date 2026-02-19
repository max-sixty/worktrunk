use clap::Subcommand;

use super::config::ApprovalsCommand;

/// Run configured hooks
#[derive(Subcommand)]
pub enum HookCommand {
    /// Show configured hooks
    ///
    /// Lists user and project hooks. Project hooks show approval status (❓ = needs approval).
    Show {
        /// Hook type to show (default: all)
        #[arg(value_parser = ["pre-switch", "post-create", "post-start", "post-switch", "pre-commit", "pre-merge", "post-merge", "pre-remove", "post-remove"])]
        hook_type: Option<String>,

        /// Show expanded commands with current variables
        #[arg(long)]
        expanded: bool,
    },

    /// Run pre-switch hooks
    ///
    /// Blocking — waits for completion before continuing.
    PreSwitch {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-create hooks
    ///
    /// Blocking — waits for completion before continuing.
    PostCreate {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-start hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostStart {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Deprecated: use --foreground instead
        #[arg(long = "no-background", hide = true)]
        no_background: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-switch hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostSwitch {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Deprecated: use --foreground instead
        #[arg(long = "no-background", hide = true)]
        no_background: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-commit hooks
    PreCommit {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-merge hooks
    PreMerge {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-merge hooks
    PostMerge {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run pre-remove hooks
    PreRemove {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Run post-remove hooks
    ///
    /// Background by default. Use `--foreground` to run in foreground for debugging.
    PostRemove {
        /// Filter by command name
        ///
        /// Supports `user:name` or `project:name` to filter by source.
        /// `user:` alone runs all user hooks; `project:` alone runs all project hooks.
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Run in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Override built-in template variable (KEY=VALUE)
        #[arg(long = "var", value_name = "KEY=VALUE", value_parser = super::parse_key_val, action = clap::ArgAction::Append)]
        vars: Vec<(String, String)>,
    },

    /// Manage command approvals
    #[command(
        after_long_help = r#"Project hooks require approval on first run to prevent untrusted projects from running arbitrary commands.

## Examples

Pre-approve all commands for current project:
```console
wt hook approvals add
```

Clear approvals for current project:
```console
wt hook approvals clear
```

Clear global approvals:
```console
wt hook approvals clear --global
```

## How approvals work

Approved commands are saved to `~/.config/worktrunk/approvals.toml`. Re-approval is required when the command template changes or the project moves. Use `--yes` to bypass prompts in CI."#
    )]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}
