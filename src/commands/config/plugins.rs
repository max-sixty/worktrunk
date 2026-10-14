//! Plugin management commands for AI coding tools.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::path::paths_match;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprintln, info_message, println, progress_message, success_message};

use super::show::{claude_config_dir, is_claude_available, is_statusline_configured};
use crate::commands::is_worktree_at_expected_path;
use crate::output::print_json;
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const MARKETPLACE_SOURCE: &str = "max-sixty/worktrunk";
const MARKETPLACE_NAME: &str = "worktrunk";
/// `PLUGIN@MARKETPLACE` selector `claude plugin install` / `uninstall` take,
/// and the `id` Claude Code lists the plugin under.
const PLUGIN_SELECTOR: &str = "worktrunk@worktrunk";

/// Handle `wt config plugins claude install`
pub fn handle_claude_install(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    // Only a confident `Some(true)` short-circuits. Both commands below are
    // idempotent — Claude Code answers an already-installed plugin and an
    // already-added marketplace with a success — so an answer wt could not
    // read costs a redundant run rather than a wrong one.
    if is_plugin_installed() == Some(true) {
        eprintln!("{}", info_message("Plugin already installed"));
        return Ok(());
    }

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Install Worktrunk plugin for <bold>Claude Code</>?"),
            || {
                let commands = format!(
                    "claude plugin marketplace add {MARKETPLACE_SOURCE}\nclaude plugin install {PLUGIN_SELECTOR}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Adding plugin from marketplace..."));
    super::run_plugin_cli(
        "claude",
        &["plugin", "marketplace", "add", MARKETPLACE_SOURCE],
    )?;

    eprintln!("{}", progress_message("Installing plugin..."));
    super::run_plugin_cli("claude", &["plugin", "install", PLUGIN_SELECTOR])?;

    eprintln!("{}", success_message("Plugin installed"));

    Ok(())
}

/// Handle `wt config plugins claude uninstall`
pub fn handle_claude_uninstall(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Uninstall Worktrunk plugin from <bold>Claude Code</>?"),
            || {
                let commands = format!(
                    "claude plugin uninstall {PLUGIN_SELECTOR}\nclaude plugin marketplace remove {MARKETPLACE_NAME}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    // Both steps run unconditionally, each tolerating only the absence Claude
    // Code itself reports. Skipping a step on a reader's say-so instead would
    // put the decision before the command rather than after it, and the two
    // halves come apart: an uninstall that removed the plugin and then failed
    // on the marketplace leaves one gone and one behind.
    eprintln!("{}", progress_message("Uninstalling plugin..."));
    super::run_plugin_removal(
        "claude",
        &["plugin", "uninstall", PLUGIN_SELECTOR],
        is_plugin_installed,
    )?;

    eprintln!(
        "{}",
        progress_message("Removing Claude Code plugin marketplace...")
    );
    super::run_plugin_removal(
        "claude",
        &["plugin", "marketplace", "remove", MARKETPLACE_NAME],
        is_marketplace_configured,
    )?;

    eprintln!("{}", success_message("Plugin & marketplace removed"));

    Ok(())
}

/// A Claude Code hook payload, by the event the plugin's hook command
/// received. Each variant carries only the fields its handler reads.
#[derive(serde::Deserialize)]
#[serde(tag = "hook_event_name")]
enum ClaudeHook {
    UserPromptSubmit {},
    Notification {},
    PreToolUse {},
    PermissionRequest(PermissionRequest),
    Stop {},
    SessionEnd {},
    WorktreeCreate { name: String },
    WorktreeRemove { worktree_path: PathBuf },
}

/// The fields of a `PermissionRequest` payload the approval reads. Each
/// defaults, so a payload missing one declines rather than failing the hook.
#[derive(serde::Deserialize)]
struct PermissionRequest {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    cwd: PathBuf,
    #[serde(default)]
    tool_input: serde_json::Value,
}

/// Handle `wt config plugins claude hook`, the command behind every hook in
/// the plugin's `hooks/hooks.json`.
///
/// Claude Code pipes each hook's payload to stdin; `hook_event_name` picks the
/// action. Keeping the logic here rather than in the hook commands leaves each
/// command a literal path and literal arguments, which Anthropic's plugin
/// directory requires, and needs no `jq`.
///
/// - Activity markers resolve against `CLAUDE_PROJECT_DIR`, the directory the
///   session launched in, so a shell `cd` during a turn can't retarget them to
///   another repository (#3921). A marker failure never fails the hook.
/// - `PermissionRequest` approves an `EnterWorktree` into a worktrunk-managed
///   worktree (see [`approves_enter_worktree`]) and otherwise marks the
///   session 💬 while the dialog waits. One hook does both because Claude Code
///   runs every matching hook in parallel.
/// - `WorktreeCreate` runs `wt switch --create` and prints the new worktree's
///   path, which Claude Code reads as the hook's answer. A failure exits
///   nonzero with nothing on stdout (#3545).
/// - `WorktreeRemove` resolves against the worktree path Claude Code hands
///   over, never `CLAUDE_PROJECT_DIR`: the `claude agents` view spans
///   repositories, so no single project dir is right for every session
///   (#3754). It never force-deletes, since Claude Code fires it on session
///   exit for any clean worktree (#2939).
///
/// Each action runs this `wt` binary again with the command a user would type,
/// so the hook behaves exactly as that command does.
pub fn handle_claude_hook() -> anyhow::Result<()> {
    let hook: ClaudeHook = serde_json::from_reader(std::io::stdin().lock())
        .context("Failed to parse Claude Code hook payload")?;
    let project_dir = match std::env::var_os("CLAUDE_PROJECT_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir().context("Failed to read the current directory")?,
    };

    match hook {
        ClaudeHook::UserPromptSubmit {} => set_marker(&project_dir, &["set", "🤖"]),
        ClaudeHook::Notification {} | ClaudeHook::PreToolUse {} | ClaudeHook::Stop {} => {
            set_marker(&project_dir, &["set", "💬"])
        }
        ClaudeHook::SessionEnd {} => set_marker(&project_dir, &["clear"]),
        ClaudeHook::PermissionRequest(request) => {
            // A payload `wt` can't resolve, such as a `cwd` in no repository,
            // declines, leaving the dialog.
            if approves_enter_worktree(&request).unwrap_or(false) {
                return print_allow_decision();
            }
            set_marker(&project_dir, &["set", "💬"])
        }
        ClaudeHook::WorktreeCreate { name } => {
            let stdout = run_wt(
                Cmd::new(wt_binary()?)
                    .args(["switch", "--create", &name, "--no-cd", "--format=json"])
                    .current_dir(&project_dir),
            )?;
            let created: serde_json::Value = serde_json::from_slice(&stdout)
                .context("Failed to parse `wt switch --format=json` output")?;
            let path = created["path"]
                .as_str()
                .context("`wt switch --format=json` output has no `path`")?;
            println!("{path}");
            Ok(())
        }
        ClaudeHook::WorktreeRemove { worktree_path } => {
            // Already gone, or never a worktree: nothing for wt to remove.
            if !worktree_path.join(".git").exists() {
                return Ok(());
            }
            let path = worktree_path.to_string_lossy();
            run_wt(Cmd::new(wt_binary()?).args([
                "-C",
                path.as_ref(),
                "remove",
                "--foreground",
                path.as_ref(),
            ]))?;
            Ok(())
        }
    }
}

/// Handle `wt config plugins claude approve-enter-worktree`, the
/// `PermissionRequest` hook of plugin copies installed before
/// [`handle_claude_hook`] existed. Marketplace installs don't update on their
/// own, so those copies keep calling it: it prints the allow decision, or exits
/// 1 so their hook command's `||` sets the 💬 marker.
pub fn handle_claude_approve_enter_worktree() -> anyhow::Result<()> {
    let request: PermissionRequest = serde_json::from_reader(std::io::stdin().lock())
        .context("Failed to parse PermissionRequest payload")?;
    if approves_enter_worktree(&request)? {
        return print_allow_decision();
    }
    Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into())
}

/// Prints the `PermissionRequest` decision that allows the call.
fn print_allow_decision() -> anyhow::Result<()> {
    print_json(&serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": { "behavior": "allow" },
        }
    }))
}

/// Sets or clears the activity marker of the worktree at `dir`, ignoring
/// failure.
fn set_marker(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let dir = dir.to_string_lossy();
    let _ = run_wt(
        Cmd::new(wt_binary()?)
            .args(["-C", dir.as_ref(), "config", "state", "marker"])
            .args(args.iter().copied()),
    );
    Ok(())
}

/// The running `wt` binary, which the hook runs again for each action.
fn wt_binary() -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("Failed to resolve wt binary path")?;
    Ok(exe.to_string_lossy().into_owned())
}

/// Runs `cmd`, passing its stderr through, and returns its stdout. A failure
/// exits with the child's status, its own error already on stderr.
fn run_wt(cmd: Cmd) -> anyhow::Result<Vec<u8>> {
    let output = cmd.run().context("Failed to run wt")?;
    std::io::stderr()
        .write_all(&output.stderr)
        .context("Failed to write wt's stderr")?;
    if !output.status.success() {
        return Err(WorktrunkError::AlreadyDisplayed {
            exit_code: output.status.code().unwrap_or(1),
        }
        .into());
    }
    Ok(output.stdout)
}

/// Whether a `PermissionRequest` is an `EnterWorktree` the plugin approves.
///
/// Claude Code confirms an `EnterWorktree` into any worktree outside its own
/// `.claude/worktrees/`, which in worktrunk's layout is every worktree, and a
/// background session waits at that dialog until someone attaches. This
/// extends Claude Code's exemption to worktrunk's managed location: the call's
/// `path` names a worktree of the repository the payload's `cwd` is in,
/// sitting at the path the `worktree-path` template gives its branch.
fn approves_enter_worktree(request: &PermissionRequest) -> anyhow::Result<bool> {
    if request.tool_name != "EnterWorktree" {
        return Ok(false);
    }
    let Some(path) = request.tool_input.get("path").and_then(|p| p.as_str()) else {
        return Ok(false);
    };
    let path = request.cwd.join(path);
    let repo = Repository::at(&request.cwd)?;
    let config = repo.user_config();
    Ok(repo
        .list_worktrees()?
        .iter()
        .any(|wt| paths_match(&wt.path, &path) && is_worktree_at_expected_path(wt, &repo, config)))
}

/// Whether Claude Code lists the worktrunk plugin, or `None` where its answer
/// cannot be read.
///
/// `claude plugin list --json` prints a bare array of plugin objects whose
/// `id` is the same `PLUGIN@MARKETPLACE` selector the install and uninstall
/// commands take.
pub(super) fn is_plugin_installed() -> Option<bool> {
    let listed = super::harness_listing("claude", &["plugin", "list", "--json"])?;
    super::listing_names(listed.as_array()?, "id", PLUGIN_SELECTOR)
}

/// Whether Claude Code lists the worktrunk marketplace, or `None` where its
/// answer cannot be read.
///
/// `claude plugin marketplace list --json` prints a bare array of marketplace
/// objects where Codex nests its own under `marketplaces`.
pub(super) fn is_marketplace_configured() -> Option<bool> {
    let listed = super::harness_listing("claude", &["plugin", "marketplace", "list", "--json"])?;
    super::listing_names(listed.as_array()?, "name", MARKETPLACE_NAME)
}

/// Handle `wt config plugins claude install-statusline`
pub fn handle_claude_install_statusline(yes: bool) -> anyhow::Result<()> {
    if is_statusline_configured() {
        eprintln!("{}", info_message("Statusline already configured"));
        return Ok(());
    }

    let settings_path = require_settings_path()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Configure statusline for <bold>Claude Code</>?"),
            || {
                eprintln!(
                    "{}",
                    worktrunk::styling::format_with_gutter(
                        r#"{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}"#,
                        None,
                    )
                );
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create Claude Code config directory")?;
    }

    // Read existing settings or start with empty object
    let mut settings: serde_json::Map<String, serde_json::Value> = if settings_path.exists() {
        let content =
            std::fs::read_to_string(&settings_path).context("Failed to read settings.json")?;
        if content.trim().is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_str(&content).context("Failed to parse settings.json")?
        }
    } else {
        serde_json::Map::new()
    };

    // Merge in the statusLine config
    settings.insert(
        "statusLine".to_string(),
        serde_json::json!({
            "type": "command",
            "command": "wt list statusline --format=claude-code"
        }),
    );

    let json = serde_json::to_string_pretty(&settings).context("Failed to serialize settings")?;
    worktrunk::utils::write_atomically(&settings_path, &(json + "\n"))
        .context("Failed to write settings.json")?;

    eprintln!("{}", success_message("Statusline configured"));

    Ok(())
}

/// Get the path to Claude Code's `settings.json` (under `CLAUDE_CONFIG_DIR` or
/// `~/.claude`), or bail if the config directory can't be determined
fn require_settings_path() -> anyhow::Result<PathBuf> {
    let Some(config_dir) = claude_config_dir() else {
        bail!("Could not determine Claude Code config directory");
    };
    Ok(config_dir.join("settings.json"))
}

/// Bail if `claude` CLI is not available
fn require_claude_cli() -> anyhow::Result<()> {
    if is_claude_available() {
        return Ok(());
    }
    bail!("claude CLI not found. Install Claude Code first: https://code.claude.com/docs/en/setup");
}
