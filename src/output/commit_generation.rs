//! Commit generation prompt for first-time LLM setup.
//!
//! Prompts users to configure LLM commit message generation when they first
//! attempt a commit without configuration. Detects available tools (claude, codex)
//! and offers to auto-configure the recommended settings.

use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::sync::LazyLock;

use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, format_toml, hint_message, info_message, success_message};

use super::prompt::{PromptResponse, prompt_yes_no_preview};

/// Example config file content, used to extract recommended commands.
const CONFIG_EXAMPLE: &str = include_str!("../../dev/config.example.toml");

/// Recommended commands parsed from the config example file (single source of truth).
///
/// Keyed by the h3 heading text in the config example (e.g., "Claude Code", "Codex").
static RECOMMENDED_COMMANDS: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| parse_recommended_commands(CONFIG_EXAMPLE));

/// Detected LLM tool available on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmTool {
    Claude,
    Codex,
}

impl LlmTool {
    /// Returns the command name for this tool.
    pub fn command_name(&self) -> &'static str {
        match self {
            LlmTool::Claude => "claude",
            LlmTool::Codex => "codex",
        }
    }

    /// The h3 heading text in config.example.toml for this tool's section.
    fn config_heading(&self) -> &'static str {
        match self {
            LlmTool::Claude => "Claude Code",
            LlmTool::Codex => "Codex",
        }
    }

    /// Returns the recommended configuration command for this tool.
    ///
    /// Parsed from the double-commented examples in dev/config.example.toml,
    /// which is the single source of truth for these commands.
    pub fn recommended_config(&self) -> &str {
        // Indexing is safe: all LlmTool variants have entries in the config example.
        &RECOMMENDED_COMMANDS[self.config_heading()]
    }
}

impl std::fmt::Display for LlmTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command_name())
    }
}

/// Parse tool commands from the double-commented config example.
///
/// Scans the entire file for `# ### ToolName` headings followed by `# # command = "..."`
/// lines. Currently only the LLM section uses this pattern; if other sections gain
/// `# # command = ` lines, scope the scan to the `## LLM commit messages` section.
/// The command value is TOML-unescaped via the `toml` crate.
fn parse_recommended_commands(config: &str) -> HashMap<String, String> {
    let mut commands = HashMap::new();
    let mut current_heading: Option<String> = None;

    for line in config.lines() {
        // H3 headings: "# ### Claude Code", "# ### Codex"
        if let Some(heading) = line.strip_prefix("# ### ") {
            current_heading = Some(heading.trim().to_string());
            continue;
        }

        // Double-commented command: "# # command = "...""
        if let Some(toml_part) = line.strip_prefix("# # ")
            && toml_part.starts_with("command = ")
            && let Some(heading) = current_heading.take()
        {
            // Config example is compile-time data; unwrap is safe.
            let table: toml::Table = toml_part.parse().unwrap();
            let cmd = table["command"].as_str().unwrap().to_string();
            commands.insert(heading, cmd);
        }
    }

    commands
}

/// Check if a command is available in PATH.
///
/// Uses platform-appropriate method: `where` on Windows, `which` on Unix.
fn command_exists(cmd: &str) -> bool {
    #[cfg(windows)]
    let check_cmd = "where";
    #[cfg(not(windows))]
    let check_cmd = "which";

    std::process::Command::new(check_cmd)
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Format a command string as TOML for display.
///
/// Uses the toml crate for proper escaping. The result may wrap in terminal
/// but is always valid TOML.
fn format_command_for_display(command: &str) -> String {
    toml::Value::String(command.to_string()).to_string()
}

/// Detect available LLM tool on the system.
///
/// Checks for claude first (preferred), then codex.
/// Returns None if neither is found.
pub fn detect_llm_tool() -> Option<LlmTool> {
    if command_exists("claude") {
        Some(LlmTool::Claude)
    } else if command_exists("codex") {
        Some(LlmTool::Codex)
    } else {
        None
    }
}

/// Prompt for commit generation configuration.
///
/// Shows a one-time prompt when the user attempts to commit without LLM configuration.
/// Detects available tools and offers to auto-configure.
///
/// Note: This function does NOT emit hints about fallback messages. The existing
/// `CommitGenerator::emit_hint_if_needed()` handles that. This function only handles
/// the interactive prompt for first-time setup.
///
/// Returns `Ok(true)` if configuration was set up, `Ok(false)` otherwise.
pub fn prompt_commit_generation(config: &mut UserConfig) -> anyhow::Result<bool> {
    let is_tty = io::stdin().is_terminal() && io::stderr().is_terminal();

    // Skip if already configured
    if config
        .commit_generation(None)
        .command
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return Ok(false);
    }

    // Skip if prompt was previously declined or dismissed
    if config.skip_commit_generation_prompt {
        return Ok(false);
    }

    // Skip if not a TTY (non-interactive)
    if !is_tty {
        return Ok(false);
    }

    // Detect available tool
    let Some(tool) = detect_llm_tool() else {
        // No tool found - set skip flag so we don't check every time
        let _ = config.set_skip_commit_generation_prompt(None);
        return Ok(false);
    };

    // Build preview content (used by both ? and success cases)
    let command = tool.recommended_config();
    let formatted_command = format_command_for_display(command);
    let config_preview = format!("[commit.generation]\ncommand = {formatted_command}");

    // Show prompt with preview on ?
    let response = prompt_yes_no_preview(
        &cformat!("Configure <bold>{tool}</> for commit messages?"),
        || {
            eprintln!(
                "{}",
                info_message(cformat!(
                    "Would add to <bold>~/.config/worktrunk/config.toml</>:"
                ))
            );
            eprintln!("{}", format_toml(&config_preview));
            eprintln!();
        },
    )?;

    match response {
        PromptResponse::Accepted => {
            // Set the configuration
            let command = command.to_string();
            if let Err(e) = config.set_commit_generation_command(command.clone(), None) {
                log::error!("Failed to save config: {}", e);
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Config save failed; add manually to <underline>~/.config/worktrunk/config.toml</>"
                    ))
                );
                return Ok(false);
            }

            // Show what was added
            eprintln!("{}", success_message(cformat!("Added to user config:")));
            eprintln!("{}", format_toml(&config_preview));
            eprintln!(
                "{}",
                hint_message(cformat!("View config: <underline>wt config show</>"))
            );

            // Blank line separates this setup phase from the main operation that follows
            eprintln!();

            Ok(true)
        }
        PromptResponse::Declined => {
            // Set skip flag so we don't prompt again
            let _ = config.set_skip_commit_generation_prompt(None);
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_llm_tool_command_name() {
        assert_eq!(LlmTool::Claude.command_name(), "claude");
        assert_eq!(LlmTool::Codex.command_name(), "codex");
    }

    #[test]
    fn test_llm_tool_recommended_config() {
        assert_snapshot!(LlmTool::Claude.recommended_config(), @"CLAUDECODE= MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''");
        assert_snapshot!(LlmTool::Codex.recommended_config(), @r#"codex exec -m gpt-5.1-codex-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == "agent_message")] | last.item.text'"#);
    }

    #[test]
    fn test_parse_recommended_commands() {
        let config = "\
# ### MyTool
#
# # [commit.generation]
# # command = \"echo hello\"
#
# ### OtherTool
#
# # [commit.generation]
# # command = \"jq -sr '[.[] | select(.type? == \\\"msg\\\")]'\"
";
        let commands = parse_recommended_commands(config);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands["MyTool"], "echo hello");
        assert_eq!(
            commands["OtherTool"],
            r#"jq -sr '[.[] | select(.type? == "msg")]'"#
        );
    }

    #[test]
    fn test_parse_recommended_commands_ignores_non_command_lines() {
        let config = "\
# ### ToolA
#
# # [commit.generation]
# # template = \"not a command\"
# ### ToolB
# # command = \"real command\"
";
        let commands = parse_recommended_commands(config);
        // ToolA has no command line, ToolB does
        assert_eq!(commands.len(), 1);
        assert_eq!(commands["ToolB"], "real command");
    }

    #[test]
    fn test_llm_tool_display() {
        assert_eq!(format!("{}", LlmTool::Claude), "claude");
        assert_eq!(format!("{}", LlmTool::Codex), "codex");
    }

    #[test]
    fn test_format_command_produces_valid_toml() {
        // Uses toml crate for proper escaping
        let result = format_command_for_display("echo hello");
        assert_eq!(result, "\"echo hello\"");

        // Long commands stay as single-line TOML
        let cmd = LlmTool::Claude.recommended_config();
        let result = format_command_for_display(cmd);
        assert_snapshot!(result, @r#""CLAUDECODE= MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''""#);
    }

    #[test]
    fn test_format_command_special_chars() {
        let result = format_command_for_display(r#"echo "hello""#);
        assert_snapshot!(result, @r#"'echo "hello"'"#);
    }

    #[test]
    fn test_command_exists_known_command() {
        // 'which' (Unix) or 'where' (Windows) should always exist
        #[cfg(not(windows))]
        assert!(command_exists("which"));
        #[cfg(windows)]
        assert!(command_exists("where"));
    }

    #[test]
    fn test_command_exists_nonexistent() {
        // A command that definitely doesn't exist
        assert!(!command_exists("__nonexistent_command_12345__"));
    }

    #[test]
    fn test_detect_llm_tool() {
        // Just exercise the function - result depends on what's installed
        let _ = detect_llm_tool();
    }
}
