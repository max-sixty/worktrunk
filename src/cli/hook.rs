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
        #[arg(value_parser = ["post-create", "post-start", "post-switch", "pre-commit", "pre-merge", "post-merge", "pre-remove"])]
        hook_type: Option<String>,

        /// Show expanded commands with current variables
        #[arg(long)]
        expanded: bool,
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

    /// Manage command approvals
    #[command(after_long_help = r#"## How Approvals Work

Commands from project hooks (`.config/wt.toml`) require approval on first run.
This prevents untrusted projects from running arbitrary commands.

**Approval flow:**
1. Command is shown as a template (variables not expanded)
2. User approves or denies
3. Approved commands are saved to user config under `[projects."project-id"]`

**When re-approval is required:**
- Command template changes (not just variable values)
- Project ID changes (repository moves)

**Bypassing prompts:**
- `--yes` flag on individual commands (e.g., `wt merge --yes`)
- Useful for CI/automation where prompts aren't possible

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
```"#)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}
