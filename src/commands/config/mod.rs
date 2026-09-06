//! Configuration commands.
//!
//! Commands for managing user config, project config, state, and hints.

mod alias;
mod approvals;
mod codex;
mod create;
mod hints;
pub mod opencode;
mod pi;
mod plugins;
mod show;
mod state;
mod update;

// Re-export public functions
pub use alias::{handle_alias_dry_run, handle_alias_show};
pub use approvals::{add_approvals, clear_approvals, list_approvals};
pub use codex::{handle_codex_install, handle_codex_uninstall};
pub use create::handle_config_create;
pub use hints::{handle_hints_clear, handle_hints_get};
pub use opencode::{handle_opencode_install, handle_opencode_uninstall};
pub use pi::{handle_pi_install, handle_pi_uninstall};
pub use plugins::{
    handle_claude_install, handle_claude_install_statusline, handle_claude_uninstall,
};
pub use show::handle_config_show;
pub use state::{
    handle_cache_clear, handle_cache_get, handle_logs_list, handle_logs_profile,
    handle_state_clear, handle_state_clear_all, handle_state_get, handle_state_set,
    handle_state_show, handle_vars_clear, handle_vars_get, handle_vars_list, handle_vars_set,
};
pub use update::handle_config_update;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message};

/// Install or update a file-based agent plugin after confirmation.
fn install_file_plugin(
    name: &str,
    target: &std::path::Path,
    source: &str,
    yes: bool,
) -> anyhow::Result<()> {
    use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};
    use anyhow::Context;
    use color_print::cformat;
    use worktrunk::path::format_path_for_display;

    let target_display = format_path_for_display(target);
    if target.exists()
        && let Ok(existing) = std::fs::read_to_string(target)
        && existing == source
    {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Plugin already installed @ <bold>{target_display}</>"
            ))
        );
        return Ok(());
    }

    let action = if target.exists() { "Update" } else { "Install" };
    let preview_msg = info_message(cformat!("Would write to <bold>{target_display}</>"));
    let preview = || eprintln!("{}", preview_msg);
    let confirmed = yes
        || prompt_yes_no_preview(
            &cformat!("{action} {name} plugin @ <bold>{target_display}</>?"),
            preview,
        )? == PromptResponse::Accepted;
    if !confirmed {
        return Ok(());
    }

    let parent = target
        .parent()
        .context("Plugin path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    worktrunk::utils::write_atomically(target, source)
        .with_context(|| format!("Failed to write plugin to {target_display}"))?;

    eprintln!(
        "{}",
        success_message(cformat!("Plugin installed @ <bold>{target_display}</>"))
    );
    eprintln!(
        "{}",
        hint_message(cformat!(
            "Activity markers (🤖/💬) will appear in <underline>wt list</>"
        ))
    );
    Ok(())
}

/// Remove a file-based agent plugin after confirmation.
fn uninstall_file_plugin(name: &str, target: &std::path::Path, yes: bool) -> anyhow::Result<()> {
    use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};
    use anyhow::Context;
    use color_print::cformat;
    use worktrunk::path::format_path_for_display;

    let target_display = format_path_for_display(target);
    if !target.exists() {
        eprintln!("{}", info_message("Plugin not installed"));
        return Ok(());
    }

    let preview_msg = info_message(cformat!("Would remove <bold>{target_display}</>"));
    let preview = || eprintln!("{}", preview_msg);
    let confirmed = yes
        || prompt_yes_no_preview(
            &cformat!("Remove {name} plugin @ <bold>{target_display}</>?"),
            preview,
        )? == PromptResponse::Accepted;
    if !confirmed {
        return Ok(());
    }

    std::fs::remove_file(target)
        .with_context(|| format!("Failed to remove plugin at {target_display}"))?;
    eprintln!(
        "{}",
        success_message(cformat!("Plugin removed from <bold>{target_display}</>"))
    );
    Ok(())
}

/// Run a plugin-CLI command (`claude` / `codex`), surfacing a non-zero exit
/// as a typed [`worktrunk::git::CommandError`].
fn run_plugin_cli(program: &str, args: &[&str]) -> anyhow::Result<()> {
    use anyhow::Context;

    let output = worktrunk::shell_exec::Cmd::new(program)
        .args(args.iter().copied())
        .run()
        .with_context(|| format!("Failed to run {program} CLI"))?;
    if !output.status.success() {
        return Err(
            worktrunk::git::CommandError::from_failed_output(program, args, &output).into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use worktrunk::config::{ProjectConfig, UserConfig};

    use super::create::comment_out_config;
    use super::show::{render_ci_tool_status, warn_unknown_keys};

    // ==================== comment_out_config tests ====================

    #[test]
    fn test_comment_out_config() {
        // Basic key-value
        assert_snapshot!(comment_out_config("key = \"value\"\n"), @r#"# key = "value""#);

        // Preserves existing comments
        assert_snapshot!(comment_out_config("# This is a comment\nkey = \"value\"\n"), @r#"
        # This is a comment
        # key = "value"
        "#);

        // Preserves empty lines (not commented)
        assert_snapshot!(comment_out_config("key1 = \"value\"\n\nkey2 = \"value\"\n"), @r#"
        # key1 = "value"

        # key2 = "value"
        "#);

        // Section headers
        assert_snapshot!(comment_out_config("[hooks]\ncommand = \"npm test\"\n"), @r#"
        # [hooks]
        # command = "npm test"
        "#);

        // Empty input
        assert_snapshot!(comment_out_config(""), @"");

        // Only empty lines
        assert_snapshot!(comment_out_config("\n\n\n"), @"");

        // Only comments (unchanged)
        assert_snapshot!(comment_out_config("# comment 1\n# comment 2\n"), @"
        # comment 1
        # comment 2
        ");

        // Mixed content
        assert_snapshot!(
            comment_out_config("# Header comment\n\n[section]\nkey = \"value\"\n\n# Another comment\nkey2 = true\n"),
            @r#"
        # Header comment

        # [section]
        # key = "value"

        # Another comment
        # key2 = true
        "#
        );

        // Inline table
        assert_snapshot!(comment_out_config("point = { x = 1, y = 2 }\n"), @"# point = { x = 1, y = 2 }");

        // Multiline array
        assert_snapshot!(comment_out_config("args = [\n  \"--flag\",\n  \"value\"\n]\n"), @r#"
        # args = [
        #   "--flag",
        #   "value"
        # ]
        "#);

        // Whitespace-only lines are not empty, so they get commented
        assert_snapshot!(comment_out_config("key = 1\n   \nkey2 = 2\n"), @"
        # key = 1
        #    
        # key2 = 2
        ");
    }

    #[test]
    fn test_comment_out_config_preserves_trailing_newline() {
        assert!(comment_out_config("key = \"value\"\n").ends_with('\n'));
        assert!(!comment_out_config("key = \"value\"").ends_with('\n'));
    }

    // ==================== warn_unknown_keys tests ====================

    #[test]
    fn test_warn_unknown_keys_empty() {
        let out = warn_unknown_keys::<UserConfig>("");
        assert!(out.is_empty());
    }

    #[test]
    fn test_warn_unknown_keys() {
        // Single unknown key
        assert_snapshot!(warn_unknown_keys::<UserConfig>("unknown-key = \"value\"\n"), @"[33m▲[39m [33mUnknown key [1munknown-key[22m will be ignored[39m");

        // Multiple unknown keys (output is sorted deterministically)
        assert_snapshot!(warn_unknown_keys::<UserConfig>("key1 = \"v1\"\nkey2 = \"v2\"\n"), @"
        [33m▲[39m [33mUnknown key [1mkey1[22m will be ignored[39m
        [33m▲[39m [33mUnknown key [1mkey2[22m will be ignored[39m
        ");
    }

    #[test]
    fn test_warn_unknown_keys_nested() {
        // Nested typos surface as dotted paths — a UX win from round-trip analysis.
        insta::assert_snapshot!(warn_unknown_keys::<UserConfig>("[merge]\nsquas = true\n"));
    }

    #[test]
    fn test_warn_unknown_keys_suggests_other_config() {
        // skip-shell-integration-prompt in project config should suggest user config
        assert_snapshot!(
            warn_unknown_keys::<ProjectConfig>("skip-shell-integration-prompt = true\n"),
            @"[33m▲[39m [33mKey [1mskip-shell-integration-prompt[22m belongs in user config (will be ignored)[39m");

        // forge in user config should suggest project config
        assert_snapshot!(warn_unknown_keys::<UserConfig>("[forge]\nplatform = \"github\"\n"), @r#"[33m▲[39m [33mKey [1mforge[22m belongs in project config (will be ignored); to set it from user config, add it under [projects."<id>"][39m"#);
    }

    #[test]
    fn test_warn_unknown_keys_user_only_commit_key_redirects_to_user_config() {
        // The LLM `command` is user-config-only. In a *project* config it must
        // redirect the user to user config — both via the legacy flat
        // `[commit-generation]` and the canonical `[commit.generation]`, since
        // `[commit.generation]` is now a valid project section (for
        // `template-append`) so the offending key surfaces nested.
        assert_snapshot!(
            warn_unknown_keys::<ProjectConfig>("[commit-generation]\ncommand = \"llm\"\n"),
            @"[33m▲[39m [33mKey [1mcommit.generation.command[22m belongs in user config (will be ignored); to scope it to this repo, add it under [projects.\"<id>\"] in user config[39m");
        assert_snapshot!(
            warn_unknown_keys::<ProjectConfig>("[commit.generation]\ncommand = \"llm\"\n"),
            @"[33m▲[39m [33mKey [1mcommit.generation.command[22m belongs in user config (will be ignored); to scope it to this repo, add it under [projects.\"<id>\"] in user config[39m");

        // The one project-valid key in the section is unaffected.
        assert!(
            warn_unknown_keys::<ProjectConfig>("[commit.generation]\ntemplate-append = \"x\"\n")
                .is_empty()
        );
        // The same user-only key in user config is valid — no warning there.
        assert!(
            warn_unknown_keys::<UserConfig>("[commit.generation]\ncommand = \"llm\"\n").is_empty()
        );
    }

    #[test]
    fn test_warn_unknown_keys_deprecated_section_in_wrong_config() {
        // `ci` is deprecated in favor of `[forge]`, which is project-only; in
        // user config it redirects with the canonical form.
        assert_snapshot!(
            warn_unknown_keys::<UserConfig>("[ci]\nplatform = \"github\"\n"),
            @"[33m▲[39m [33mKey [1mci[22m belongs in project config as [forge][39m");
    }

    #[test]
    fn test_warn_unknown_keys_deprecated_in_right_config_is_skipped() {
        // commit-generation in user config should be skipped (deprecation system handles it)
        assert!(
            warn_unknown_keys::<UserConfig>("[commit-generation]\ncommand = \"llm\"\n").is_empty()
        );

        // ci in project config should be skipped (deprecation system handles it)
        assert!(warn_unknown_keys::<ProjectConfig>("[ci]\nplatform = \"github\"\n").is_empty());
    }

    // ==================== render_ci_tool_status tests ====================

    #[test]
    fn test_render_ci_tool_status() {
        // Installed and authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, true).unwrap();
        assert_snapshot!(out, @"[32m✓[39m [32m[1mgh[22m installed & authenticated[39m");

        // Installed but not authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, false).unwrap();
        assert_snapshot!(out, @"[33m▲[39m [33m[1mgh[22m installed but not authenticated; run [1mgh auth login[22m[39m");

        // The auth-setup command differs by CLI: `tea` uses `tea login add`,
        // `az` uses `az login`, the rest use `<tool> auth login`.
        let mut out = String::new();
        render_ci_tool_status(&mut out, "tea", "Gitea", true, false).unwrap();
        assert_snapshot!(out, @"[33m▲[39m [33m[1mtea[22m installed but not authenticated; run [1mtea login add[22m[39m");

        let mut out = String::new();
        render_ci_tool_status(&mut out, "az", "Azure DevOps", true, false).unwrap();
        assert_snapshot!(out, @"[33m▲[39m [33m[1maz[22m installed but not authenticated; run [1maz login[22m[39m");

        // Not installed
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", false, false).unwrap();
        assert_snapshot!(out, @"[2m↳[22m [2m[1mglab[22m not found (GitLab CI status unavailable)[22m");

        // glab installed and authenticated
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", true, true).unwrap();
        assert_snapshot!(out, @"[32m✓[39m [32m[1mglab[22m installed & authenticated[39m");
    }
}
