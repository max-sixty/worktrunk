mod alias;
pub(crate) mod command_approval;
pub(crate) mod command_executor;
pub(crate) mod commit;
pub(crate) mod config;
pub(crate) mod configure_shell;
pub(crate) mod context;
pub(crate) mod custom;
mod eval;
mod for_each;
mod handle_switch;
mod hook_commands;
mod hook_filter;
pub(crate) mod hooks;
pub(crate) mod init;
pub(crate) mod list;
pub(crate) mod merge;
#[cfg(unix)]
pub(crate) mod picker;
pub(crate) mod pipeline_spec;
pub(crate) mod process;
pub(crate) mod project_config;
mod relocate;
pub(crate) mod repository_ext;
mod run_pipeline;
pub(crate) mod statusline;
pub(crate) mod step_commands;
pub(crate) mod worktree;

pub(crate) use alias::{
    AliasOptions, HelpContext, alias_names_for_suggestions, augment_help, step_alias, try_alias,
};
pub(crate) use config::{
    add_approvals, clear_approvals, handle_claude_install, handle_claude_install_statusline,
    handle_claude_uninstall, handle_config_create, handle_config_show, handle_config_update,
    handle_hints_clear, handle_hints_get, handle_logs_list, handle_opencode_install,
    handle_opencode_uninstall, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show, handle_vars_clear, handle_vars_get, handle_vars_list,
    handle_vars_set,
};
pub(crate) use configure_shell::{
    handle_configure_shell, handle_show_theme, handle_unconfigure_shell,
};
pub(crate) use custom::handle_custom_command;
pub(crate) use eval::step_eval;
pub(crate) use for_each::step_for_each;
pub(crate) use handle_switch::{SwitchOptions, handle_switch};
pub(crate) use hook_commands::{handle_hook_show, run_hook};
pub(crate) use init::{handle_completions, handle_init};
pub(crate) use list::handle_list;
pub(crate) use merge::{MergeOptions, handle_merge};
#[cfg(unix)]
pub(crate) use picker::handle_picker;
pub(crate) use repository_ext::RemoveTarget;
pub(crate) use run_pipeline::run_pipeline;
pub(crate) use step_commands::{
    PromoteResult, RebaseResult, SquashResult, handle_promote, handle_rebase, handle_squash,
    step_commit, step_copy_ignored, step_diff, step_prune, step_relocate, step_show_squash_prompt,
};
pub(crate) use worktree::{
    OperationMode, is_worktree_at_expected_path, resolve_worktree_arg, worktree_display_name,
};

// Re-export Shell from the canonical location
pub(crate) use worktrunk::shell::Shell;

use color_print::cformat;
use worktrunk::styling::{eprintln, format_with_gutter};

/// Format command execution label with optional command name.
///
/// Examples:
/// - `format_command_label("post-create", Some("install"))` → `"Running post-create install"` (with bold)
/// - `format_command_label("post-create", None)` → `"Running post-create"`
pub(crate) fn format_command_label(command_type: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => cformat!("Running {command_type} <bold>{name}</>"),
        None => format!("Running {command_type}"),
    }
}

/// Return candidates similar to `query`, sorted by descending Jaro–Winkler
/// similarity, filtered by `score > 0.7`, and deduplicated while preserving
/// order. The 0.7 threshold matches clap's internal `did_you_mean` so
/// wt-synthesized "unrecognized subcommand" tips read identically to clap's
/// native output — keep them aligned if clap ever changes it.
pub(crate) fn did_you_mean(
    query: &str,
    candidates: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut scored: Vec<(f64, String)> = candidates
        .into_iter()
        .map(|candidate| (strsim::jaro_winkler(query, &candidate), candidate))
        .filter(|(score, _)| *score > 0.7)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, n)| seen.insert(n.clone()))
        .map(|(_, n)| n)
        .collect()
}

/// Force concurrent steps to run serially. Test-only escape hatch — set via
/// `WORKTRUNK_TEST_SERIAL_CONCURRENT=1` to make output ordering deterministic
/// for snapshot tests, mirroring how `RAYON_NUM_THREADS=1` is used elsewhere.
///
/// Honored by both alias `HookStep::Concurrent` execution and the background
/// pipeline runner's concurrent groups.
pub(crate) fn force_serial_concurrent() -> bool {
    std::env::var_os("WORKTRUNK_TEST_SERIAL_CONCURRENT").is_some()
}

/// Show detailed diffstat for a given commit range.
///
/// Displays the diff statistics (file changes, insertions, deletions) in a gutter format.
/// Used after commit/squash to show what was included in the commit.
///
/// # Arguments
/// * `repo` - The repository to query
/// * `range` - The commit range to diff (e.g., "HEAD~1..HEAD" or "main..HEAD")
pub(crate) fn show_diffstat(repo: &worktrunk::git::Repository, range: &str) -> anyhow::Result<()> {
    let term_width = crate::display::terminal_width();
    let stat_width = term_width.saturating_sub(worktrunk::styling::GUTTER_OVERHEAD);
    let diff_stat = repo
        .run_command(&[
            "diff",
            "--color=always",
            "--stat",
            &format!("--stat-width={}", stat_width),
            range,
        ])?
        .trim_end()
        .to_string();

    if !diff_stat.is_empty() {
        eprintln!("{}", format_with_gutter(&diff_stat, None));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_command_label() {
        use insta::assert_snapshot;
        assert_snapshot!(format_command_label("post-create", Some("install")), @"Running post-create [1minstall[22m");
        assert_snapshot!(format_command_label("pre-merge", None), @"Running pre-merge");
        assert_snapshot!(format_command_label("post-start", Some("build")), @"Running post-start [1mbuild[22m");
        assert_snapshot!(format_command_label("pre-commit", None), @"Running pre-commit");
    }
}
