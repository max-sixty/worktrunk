use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anstyle::Style;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{self, Shell};
use worktrunk::styling::{
    INFO_SYMBOL, SUCCESS_SYMBOL, eprint, eprintln, format_bash_with_gutter, format_toml,
    format_with_gutter, hint_message, println, prompt_message, warning_message,
};
use worktrunk::utils::write_atomically;

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};
use crate::output::shell_integration::shell_extension_label;

pub struct ConfigureResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: ConfigAction,
    pub config_line: String,
}

pub struct UninstallResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: UninstallAction,
    /// Path that replaces this one (for deprecated location cleanup)
    pub superseded_by: Option<PathBuf>,
    /// The lines the action applies to, for a file worktrunk edits in place
    /// (the bash/zsh/PowerShell rc files, which are the user's).
    ///
    /// Shown before removal and again after, so no line leaves a user's rc file
    /// unseen: detection is a heuristic, and `wt config shell uninstall` is the
    /// one command that acts on it destructively. Empty for the fish/nushell
    /// wrappers, whole files worktrunk owns and the path already names.
    pub matched_lines: Vec<String>,
}

pub struct UninstallScanResult {
    pub results: Vec<UninstallResult>,
    pub completion_results: Vec<CompletionUninstallResult>,
    /// Shell extensions not found (bash/zsh show as "integration", fish as "shell extension")
    pub not_found: Vec<(Shell, PathBuf)>,
    /// Completion files not found (only fish has separate completion files)
    pub completion_not_found: Vec<(Shell, PathBuf)>,
}

pub struct CompletionUninstallResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: UninstallAction,
}

pub struct ScanResult {
    pub configured: Vec<ConfigureResult>,
    pub completion_results: Vec<CompletionResult>,
    pub skipped: Vec<(Shell, PathBuf)>, // Shell + first path that was checked
    /// Zsh was configured but compinit is missing (completions won't work without it)
    pub zsh_needs_compinit: bool,
    /// Legacy/stranded files cleaned up during install, paired with the shell
    /// they belong to so the display can name the canonical replacement.
    /// Covers the fish `conf.d/wt.fish` → `functions/wt.fish` migration (#566)
    /// and the nushell `<config-dir>/vendor/autoload` → `<data-dir>/vendor/autoload`
    /// move (#2878).
    pub legacy_cleanups: Vec<(Shell, PathBuf)>,
}

pub struct CompletionResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: ConfigAction,
}

#[derive(Debug, PartialEq)]
pub enum UninstallAction {
    Removed,
    WouldRemove,
}

impl UninstallAction {
    pub fn description(&self) -> &str {
        match self {
            UninstallAction::Removed => "Removed",
            UninstallAction::WouldRemove => "Will remove",
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            UninstallAction::Removed => SUCCESS_SYMBOL,
            UninstallAction::WouldRemove => INFO_SYMBOL,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ConfigAction {
    Added,
    AlreadyExists,
    Created,
    WouldAdd,
    WouldCreate,
}

impl ConfigAction {
    pub fn description(&self) -> &str {
        match self {
            ConfigAction::Added => "Added",
            ConfigAction::AlreadyExists => "Already configured",
            ConfigAction::Created => "Created",
            ConfigAction::WouldAdd => "Will add",
            ConfigAction::WouldCreate => "Will create",
        }
    }

    /// Returns the appropriate symbol for this action
    pub fn symbol(&self) -> &'static str {
        match self {
            ConfigAction::Added | ConfigAction::Created => SUCCESS_SYMBOL,
            ConfigAction::AlreadyExists => INFO_SYMBOL,
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => INFO_SYMBOL,
        }
    }
}

/// The header comment every wrapper template worktrunk has shipped opens with,
/// and which a user's own `wt.fish` / `wt.nu` would not carry.
const WRAPPER_MARKER: &str = "worktrunk shell integration for";

/// The header every fish completion file worktrunk has shipped opens with.
const COMPLETION_MARKER: &str = "# worktrunk completions for";

/// Whether a whole file is one worktrunk generated, and so is `uninstall`'s to
/// delete.
///
/// This is the one place ownership is read out of a file's contents, because
/// it's the one place worktrunk doesn't know the name. `uninstall` takes no
/// `--cmd`, so it lists the shell-owned directories and has to tell worktrunk's
/// `{cmd}.fish` from the user's own files sitting beside it. Wherever the
/// command name *is* known — install, and the legacy-location cleanups that
/// accompany it — the path names the file worktrunk owns and nothing reads it.
///
/// The answer is independent of the binary name embedded in the file, so a
/// wrapper installed as `wt.fish`, `git-wt.fish`, or `git-wt.nu` is recognized
/// the same way. Every wrapper template worktrunk renders carries the header,
/// so that answers first. The fallback covers the legacy `conf.d/{cmd}.fish`,
/// which predates the templates and holds nothing but the init line: a file
/// qualifies only when every non-blank, non-comment line is an integration line
/// (per the rc-file line detector, keeping one definition of "an integration
/// line").
///
/// The directories walked are the user's, so the question is asked of every
/// line rather than of the file as a blob. A user's own `wt.fish` that runs
/// `wt config shell init` amid other code — or merely mentions it in a comment
/// — survives; a whole-file substring test would delete it.
fn is_worktrunk_managed_content(content: &str) -> bool {
    if content.contains(WRAPPER_MARKER) {
        return true;
    }
    let code_lines = fish_code_lines(content);
    !code_lines.is_empty()
        && code_lines
            .into_iter()
            .all(shell::is_shell_integration_line_for_uninstall_any_cmd)
}

/// Take back the fish wrapper's legacy `conf.d` location after installing to
/// `functions/`.
///
/// Fish integration used to install to `~/.config/fish/conf.d/{cmd}.fish`,
/// which loads before Homebrew's PATH setup in `config.fish` (issue #566);
/// installs now write `functions/{cmd}.fish`, autoloaded on first use.
///
/// The path names the command being installed, so it's worktrunk's and the file
/// goes whole — the contents are never read. Leaving it would also break the
/// install it accompanies: `conf.d` is sourced at startup, so a `function
/// {cmd}` defined there is already loaded by the time fish would autoload
/// `functions/{cmd}.fish`, and the stale wrapper wins every time.
///
/// With `dry_run`, the same detection runs but nothing is removed — the returned
/// paths are what a real run *would* delete, so a preview and the confirmation
/// prompt can name them before the user consents (issue #3644).
///
/// Returns the paths of files that were cleaned up, each paired with `Shell::Fish`.
fn cleanup_legacy_fish_conf_d(
    configured: &[ConfigureResult],
    cmd: &str,
    dry_run: bool,
) -> Vec<(Shell, PathBuf)> {
    let mut cleaned = Vec::new();

    // Clean up if fish was part of the install (regardless of whether it already existed)
    // This handles the case where user manually created functions/wt.fish but still has
    // the old conf.d/wt.fish hanging around
    let fish_targeted = configured.iter().any(|r| r.shell == Shell::Fish);

    if !fish_targeted {
        return cleaned;
    }

    // Check for legacy conf.d file
    let Ok(legacy_path) = Shell::legacy_fish_conf_d_path(cmd) else {
        return cleaned;
    };

    if !legacy_path.exists() {
        return cleaned;
    }

    if dry_run {
        cleaned.push((Shell::Fish, legacy_path));
        return cleaned;
    }

    match fs::remove_file(&legacy_path) {
        Ok(()) => {
            cleaned.push((Shell::Fish, legacy_path));
        }
        Err(e) => {
            // Warn but don't fail - the new integration will still work
            eprintln!(
                "{}",
                warning_message(color_print::cformat!(
                    "Failed to remove deprecated <bold>{}</>: {e}",
                    format_path_for_display(&legacy_path)
                ))
            );
        }
    }

    cleaned
}

/// Clean up Nushell wrapper files stranded at legacy autoload locations.
///
/// Older worktrunk installed the wrapper under `<config-dir>/vendor/autoload`,
/// which Nushell never autoloads (issue #2878). After installing to the correct
/// vendor-autoload dir (`<data-dir>/vendor/autoload`), this removes the wrapper
/// left at the other candidate paths so a stale, never-loaded copy isn't left
/// behind.
///
/// Every path considered is one worktrunk itself computes for this command name
/// (`config_paths`), so — as with the fish `conf.d` cleanup — the path settles
/// ownership and the file is removed whole, unread.
///
/// With `dry_run`, the same detection runs but nothing is removed — the returned
/// paths are what a real run *would* delete, so a preview and the confirmation
/// prompt can name them before the user consents (issue #3644).
///
/// Returns the paths removed, each paired with `Shell::Nushell`.
fn cleanup_stranded_nushell(
    configured: &[ConfigureResult],
    cmd: &str,
    dry_run: bool,
) -> Vec<(Shell, PathBuf)> {
    let mut cleaned = Vec::new();

    // Only act if nushell was part of this install.
    let Some(nu_result) = configured.iter().find(|r| r.shell == Shell::Nushell) else {
        return cleaned;
    };
    // The canonical write target — never remove this one.
    let canonical = &nu_result.path;

    let Ok(candidates) = Shell::Nushell.config_paths(cmd) else {
        return cleaned;
    };

    for path in candidates {
        if &path == canonical || !path.exists() {
            continue;
        }
        if dry_run {
            cleaned.push((Shell::Nushell, path));
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => cleaned.push((Shell::Nushell, path)),
            Err(e) => {
                // Warn but don't fail - the new integration still works.
                eprintln!(
                    "{}",
                    warning_message(color_print::cformat!(
                        "Failed to remove deprecated <bold>{}</>: {e}",
                        format_path_for_display(&path)
                    ))
                );
            }
        }
    }

    cleaned
}

/// Both legacy-location cleanups for one install, as one list.
///
/// With `dry_run`, detects the files a real install would remove without
/// removing them, so a `--dry-run` preview and the install confirmation can name
/// the deletions before they happen (issue #3644). With `dry_run` false, removes
/// them and returns what was removed.
pub(crate) fn collect_legacy_cleanups(
    configured: &[ConfigureResult],
    cmd: &str,
    dry_run: bool,
) -> Vec<(Shell, PathBuf)> {
    let mut cleanups = cleanup_legacy_fish_conf_d(configured, cmd, dry_run);
    cleanups.extend(cleanup_stranded_nushell(configured, cmd, dry_run));
    cleanups
}

pub fn handle_configure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
    dry_run: bool,
    cmd: String,
) -> Result<ScanResult, String> {
    shell::validate_shell_command_name(&cmd)?;

    // First, do a dry-run to see what would be changed
    let preview = scan_shell_configs(shell_filter, true, &cmd)?;

    // Preview completions that would be written
    let shells: Vec<_> = preview.configured.iter().map(|r| r.shell).collect();
    let completion_preview = process_shell_completions(&shells, true, &cmd)?;

    // If nothing to do, return early
    if preview.configured.is_empty() {
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups: Vec::new(),
        });
    }

    // Check if any changes are needed (not all are AlreadyExists)
    let needs_shell_changes = preview
        .configured
        .iter()
        .any(|r| !matches!(r.action, ConfigAction::AlreadyExists));
    let needs_completion_changes = completion_preview
        .iter()
        .any(|r| !matches!(r.action, ConfigAction::AlreadyExists));

    // Detect (without removing) the legacy files a real install would delete, so
    // both the --dry-run preview and the confirmation prompt can name them before
    // the user consents — the removal is destructive and used to happen unpreviewed
    // (issue #3644).
    let legacy_preview = collect_legacy_cleanups(&preview.configured, &cmd, true);

    // For --dry-run, show preview and return without modifying anything
    if dry_run {
        let preview_text = show_install_preview(
            &preview.configured,
            &completion_preview,
            &legacy_preview,
            &cmd,
        );
        if !preview_text.is_empty() {
            println!("{preview_text}");
        }
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups: legacy_preview,
        });
    }

    // If nothing needs to be changed, there may still be legacy files to clean up
    // (a user who upgraded and has both functions/wt.fish and conf.d/wt.fish). That
    // removal is destructive, so gate it behind the same confirmation as an install
    // — never delete a hand-written file as a silent side effect (issue #3644).
    if !needs_shell_changes && !needs_completion_changes {
        if legacy_preview.is_empty() {
            return Ok(ScanResult {
                configured: preview.configured,
                completion_results: completion_preview,
                skipped: preview.skipped,
                zsh_needs_compinit: false,
                legacy_cleanups: Vec::new(),
            });
        }

        if !skip_confirmation
            && !prompt_for_install(
                &preview.configured,
                &completion_preview,
                &legacy_preview,
                &cmd,
                "Remove deprecated shell integration files?",
            )?
        {
            return Err("Cancelled by user".to_string());
        }

        let legacy_cleanups = collect_legacy_cleanups(&preview.configured, &cmd, false);
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups,
        });
    }

    // Show what will be done and ask for confirmation (unless --yes flag is used)
    if !skip_confirmation
        && !prompt_for_install(
            &preview.configured,
            &completion_preview,
            &legacy_preview,
            &cmd,
            "Install shell integration?",
        )?
    {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --yes flag was used), now actually apply the changes
    let result = scan_shell_configs(shell_filter, false, &cmd)?;
    let completion_results = process_shell_completions(&shells, false, &cmd)?;

    // Zsh completions require compinit to be enabled. Unlike bash/fish, zsh doesn't
    // enable its completion system by default - users must explicitly call compinit.
    // We detect this and return a flag so the caller can show an appropriate advisory.
    //
    // We only check this during `install`, not `init`, because:
    // - `init` outputs a script that gets eval'd - advisory would pollute that
    // - `install` is the user-facing command where hints are appropriate
    //
    // We check when:
    // - User explicitly runs `install zsh` (they clearly want zsh integration)
    // - User runs `install` (all shells) AND their current shell (process
    //   tree, falling back to $SHELL) is zsh (they use zsh daily)
    //
    // We skip if:
    // - User runs `install` but their current shell is bash/fish (they may be
    //   configuring zsh for occasional use; don't nag about their non-primary
    //   shell)
    // - Zsh was already configured (AlreadyExists) - they've seen this before
    let zsh_was_configured = result
        .configured
        .iter()
        .any(|r| r.shell == Shell::Zsh && !matches!(r.action, ConfigAction::AlreadyExists));
    let should_check_compinit = zsh_was_configured
        && (shell_filter == Some(Shell::Zsh)
            || (shell_filter.is_none() && shell::current_shell() == Some(Shell::Zsh)));

    // Probe a normal interactive zsh (global + user startup files) to check if
    // compinit is enabled in the session the freshly installed wrapper enters.
    // Only flag if we positively detect it's missing (Some(false)).
    // If detection fails (None), stay silent - we can't be sure.
    let zsh_needs_compinit = should_check_compinit
        && shell::probe_zsh_compdef(shell::ZshStartupScope::GlobalAndUser) == Some(false);

    // Clean up legacy fish conf.d file if we just installed to functions/
    // (issue #566), plus any nushell wrapper stranded at a legacy autoload
    // location (issue #2878). The confirmation above listed these removals
    // (issue #3644).
    let legacy_cleanups = collect_legacy_cleanups(&result.configured, &cmd, false);

    Ok(ScanResult {
        configured: result.configured,
        completion_results,
        skipped: result.skipped,
        zsh_needs_compinit,
        legacy_cleanups,
    })
}

/// Check if we should auto-configure PowerShell profiles.
///
/// **Non-Windows:** PowerShell Core sets PSModulePath, which we use to detect
/// PowerShell sessions. This is reliable because PowerShell must be explicitly
/// installed on these platforms.
///
/// **Windows:** We check that `SHELL` is NOT set. The `SHELL` env var is set by
/// Git Bash, MSYS2, and Cygwin, but NOT by cmd.exe or PowerShell. When `SHELL`
/// is absent on Windows, the user is likely in a Windows-native shell (cmd or
/// PowerShell), so we auto-configure both PowerShell profiles. This avoids the
/// PSModulePath false-positive issue (issue #885) while still supporting
/// PowerShell users who haven't created a profile yet.
fn should_auto_configure_powershell() -> bool {
    // Allow tests to override detection (set via Command::env() in integration tests)
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_POWERSHELL_ENV") {
        return val == "1";
    }

    #[cfg(windows)]
    {
        // On Windows, SHELL is set by Git Bash/MSYS2/Cygwin but not by cmd/PowerShell.
        // If SHELL is absent, we're likely in a Windows-native shell.
        std::env::var_os("SHELL").is_none()
    }

    #[cfg(not(windows))]
    {
        // On non-Windows, PSModulePath reliably indicates PowerShell Core
        std::env::var_os("PSModulePath").is_some()
    }
}

pub fn scan_shell_configs(
    shell_filter: Option<Shell>,
    dry_run: bool,
    cmd: &str,
) -> Result<ScanResult, String> {
    shell::validate_shell_command_name(cmd)?;

    // Iterate every supported shell. Shells the user doesn't have are filtered
    // out of the Skipped output by `is_installed()` below, matching how
    // bash/zsh/fish/nushell are handled.
    let default_shells = Shell::all();

    // Detect whether the user is *running in* PowerShell or Nushell right now.
    // This unlocks `allow_create` so we'll write a profile/autoload file even
    // when none exists — needed because PowerShell users may not have a profile
    // (issue #885) and Nushell's vendor/autoload was introduced in 0.96.0.
    // - PowerShell (non-Windows): PSModulePath set
    // - PowerShell (Windows): SHELL absent (Git Bash/MSYS2/Cygwin set it)
    // - Nushell: `nu` on PATH
    let in_powershell_env = should_auto_configure_powershell();
    let nushell_available = Shell::Nushell.is_installed();

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let mut results = Vec::new();
    let mut skipped = Vec::new();

    for shell in shells {
        let paths = shell
            .config_paths(cmd)
            .map_err(|e| format!("Failed to get config paths for {shell}: {e}"))?;

        // Find the first existing config file
        let target_path = paths.iter().find(|p| p.exists());

        // For Fish/Nushell, also check if any candidate's parent directory exists
        // since we create the file there rather than modifying an existing one
        let has_config_location = if shell.is_wrapper_based() {
            paths.iter().any(|p| p.parent().is_some_and(|d| d.exists())) || target_path.is_some()
        } else {
            target_path.is_some()
        };

        // Auto-configure shells when we detect them on the system, even if their
        // config directory doesn't exist yet:
        // - PowerShell: profile may not exist (issue #885)
        // - Nushell: vendor/autoload/ may not exist (introduced in nushell v0.96.0)
        let in_detected_shell = (matches!(shell, Shell::PowerShell) && in_powershell_env)
            || (matches!(shell, Shell::Nushell) && nushell_available);

        // Only configure if explicitly targeting this shell OR if config file/location exists
        // OR if we detected we're running in this shell's environment
        let should_configure = shell_filter.is_some() || has_config_location || in_detected_shell;

        // Allow creating the config file if explicitly targeting this shell,
        // or if we detected we're in this shell's environment
        let allow_create = shell_filter.is_some() || in_detected_shell;

        if should_configure {
            // Wrapper-based shells (Fish, Nushell) always write to the canonical
            // location (`paths.first()`), never to whichever candidate happens to
            // exist. For Nushell that matters: a wrapper stranded at a legacy
            // `<config-dir>/vendor/autoload` path (issue #2878) must not become
            // the write target — install writes the correct vendor-autoload path
            // and `cleanup_stranded_nushell` removes the stale copy. Eval-based
            // shells keep using the first existing config file.
            let path = if shell.is_wrapper_based() {
                paths.first()
            } else {
                target_path.or_else(|| paths.first())
            };
            if let Some(path) = path {
                match configure_shell_file(shell, path, dry_run, allow_create, cmd) {
                    Ok(Some(result)) => results.push(result),
                    Ok(None) => {} // No action needed
                    Err(e) => {
                        // For non-critical errors, we could continue with other shells
                        // but for now we'll fail fast
                        return Err(format!("Failed to configure {shell}: {e}"));
                    }
                }
            }
        } else if shell_filter.is_none() && shell.is_installed() {
            // Track skipped shells (only when not explicitly filtering, and only
            // when the shell binary is on PATH — otherwise the user almost
            // certainly doesn't use this shell and the entry is just clutter).
            // For Fish/Nushell, we check for parent directory; for others, the config file
            let skipped_path = if shell.is_wrapper_based() {
                paths
                    .first()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
            } else {
                paths.first().cloned()
            };
            if let Some(path) = skipped_path {
                skipped.push((shell, path));
            }
        }
    }

    Ok(ScanResult {
        configured: results,
        completion_results: Vec::new(), // Completions handled separately in handle_configure_shell
        skipped,
        zsh_needs_compinit: false,   // Caller handles compinit detection
        legacy_cleanups: Vec::new(), // Caller handles legacy cleanup
    })
}

fn configure_shell_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
    allow_create: bool,
    cmd: &str,
) -> Result<Option<ConfigureResult>, String> {
    // The line we write to the config file (also used for display)
    let config_line = shell.config_line(cmd);

    // For Fish and Nushell, we write the full wrapper to a file that gets autoloaded.
    // This allows updates to worktrunk to automatically provide the latest wrapper logic
    // without requiring reinstall.
    if shell.is_wrapper_based() {
        let init = shell::ShellInit::with_prefix(shell, cmd.to_string());
        let wrapper = if matches!(shell, Shell::Fish) {
            init.generate_fish_wrapper()
                .map_err(|e| format!("Failed to generate fish wrapper: {e}"))?
        } else {
            init.generate()
                .map_err(|e| format!("Failed to generate nushell wrapper: {e}"))?
        };
        return configure_wrapper_file(shell, path, &wrapper, dry_run, allow_create, &config_line);
    }

    // For other shells, check if file exists
    if path.exists() {
        // Read the file and check if our integration already exists
        let file = fs::File::open(path)
            .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

        let reader = BufReader::new(file);

        // Check for the canonical line and older/manual forms for this shell.
        for line in reader.lines() {
            let line = line.map_err(|e| {
                format!(
                    "Failed to read line from {}: {}",
                    format_path_for_display(path),
                    e
                )
            })?;

            if is_install_shell_integration_line(&line, shell, cmd) {
                return Ok(Some(ConfigureResult {
                    shell,
                    path: path.to_path_buf(),
                    action: ConfigAction::AlreadyExists,
                    config_line: config_line.clone(),
                }));
            }
        }

        // Line doesn't exist, add it
        if dry_run {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::WouldAdd,
                config_line: config_line.clone(),
            }));
        }

        // Append the line with proper spacing
        let mut file = OpenOptions::new().append(true).open(path).map_err(|e| {
            format!(
                "Failed to open {} for writing: {}",
                format_path_for_display(path),
                e
            )
        })?;

        // Add blank line before config, then the config line with its own newline
        write!(file, "\n{}\n", config_line).map_err(|e| {
            format!(
                "Failed to write to {}: {}",
                format_path_for_display(path),
                e
            )
        })?;

        Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action: ConfigAction::Added,
            config_line: config_line.clone(),
        }))
    } else {
        // File doesn't exist
        // Only create if allowed (explicitly targeting this shell or detected environment)
        if allow_create {
            if dry_run {
                return Ok(Some(ConfigureResult {
                    shell,
                    path: path.to_path_buf(),
                    action: ConfigAction::WouldCreate,
                    config_line: config_line.clone(),
                }));
            }

            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create directory {}: {}",
                        format_path_for_display(parent),
                        e
                    )
                })?;
            }

            // Write the config content
            write_atomically(path, &format!("{}\n", config_line)).map_err(|e| {
                format!(
                    "Failed to write to {}: {}",
                    format_path_for_display(path),
                    e
                )
            })?;

            Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::Created,
                config_line: config_line.clone(),
            }))
        } else {
            // Don't create config files for shells the user might not use
            Ok(None)
        }
    }
}

fn is_install_shell_integration_line(line: &str, shell: Shell, cmd: &str) -> bool {
    shell::is_shell_integration_line(line, cmd)
        && line
            .to_ascii_lowercase()
            .contains(&format!("config shell init {shell}"))
}

/// Extract non-comment, non-blank lines from fish/nushell source.
///
/// Install compares wrapper code lines so it detects existing installations even
/// when comment text has changed between versions (e.g. updated documentation
/// URLs); uninstall classifies a headerless file by whether its code lines are
/// all integration lines.
fn fish_code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Write the autoloaded wrapper: fish's `functions/{cmd}.fish`, nushell's
/// `vendor/autoload/{cmd}.nu`.
///
/// The path is named after the command being installed, so it names the file
/// worktrunk owns and an existing one there is replaced whole. The read below
/// only separates "already installed" from "needs writing"; ownership is read
/// out of a file's contents in one place only, `is_worktrunk_managed_content`.
fn configure_wrapper_file(
    shell: Shell,
    path: &Path,
    content: &str,
    dry_run: bool,
    allow_create: bool,
    config_line: &str,
) -> Result<Option<ConfigureResult>, String> {
    // For Fish and Nushell, we write the full wrapper to a file that gets autoloaded.
    // - Fish: functions/{cmd}.fish is autoloaded on first invocation
    // - Nushell: vendor/autoload/{cmd}.nu is autoloaded automatically at startup

    // Check if it already exists and has our integration
    // Read errors (including not-found) fall through to "not configured"
    if let Ok(existing_content) = fs::read_to_string(path) {
        // Compare only non-comment lines so that comment changes (e.g. updated
        // URLs) don't cause existing installations to appear unconfigured.
        if fish_code_lines(&existing_content) == fish_code_lines(content) {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::AlreadyExists,
                config_line: config_line.to_string(),
            }));
        }
    }

    // File doesn't exist or doesn't have our integration
    // For Fish/Nushell, create if parent directory exists or if explicitly allowed
    // This is different from other shells because these use autoload directories
    // which may exist even if the specific wrapper file doesn't
    if !allow_create && !path.exists() {
        // Check if parent directory exists
        if !path.parent().is_some_and(|p| p.exists()) {
            return Ok(None);
        }
    }

    if dry_run {
        // Fish/Nushell write the complete file - use WouldAdd if file exists, WouldCreate if new
        let action = if path.exists() {
            ConfigAction::WouldAdd
        } else {
            ConfigAction::WouldCreate
        };
        return Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action,
            config_line: config_line.to_string(),
        }));
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create directory {}: {e}",
                format_path_for_display(parent)
            )
        })?;
    }

    // Write the complete wrapper file
    write_atomically(path, &format!("{}\n", content))
        .map_err(|e| format!("Failed to write {}: {e}", format_path_for_display(path)))?;

    Ok(Some(ConfigureResult {
        shell,
        path: path.to_path_buf(),
        action: ConfigAction::Created,
        config_line: config_line.to_string(),
    }))
}

/// Format what will be installed (shell extensions and completions) and what
/// legacy files will be removed to make way for them.
///
/// Returns the preview as a string (no trailing newline); the caller picks the
/// sink. `--dry-run` is the command's answer, so it prints to stdout; the
/// interactive `?` re-preview during the install prompt is mid-prompt narration,
/// so it prints to stderr. See /writing-user-outputs.
///
/// `legacy_cleanups` are the deprecated wrapper files a real install would
/// delete (fish `conf.d`, stranded nushell autoload). Listing them here is what
/// makes the removal consented rather than a silent side effect (issue #3644);
/// the message mirrors the after-the-fact "Removed … (deprecated; now using …)"
/// line, in the future tense.
///
/// Note: I/O errors are intentionally ignored - preview is best-effort
/// and shouldn't block the prompt flow.
pub fn show_install_preview(
    results: &[ConfigureResult],
    completion_results: &[CompletionResult],
    legacy_cleanups: &[(Shell, PathBuf)],
    cmd: &str,
) -> String {
    let bold = Style::new().bold();
    let mut blocks: Vec<String> = Vec::new();

    // Show shell extension changes
    for result in results {
        // Skip items that are already configured
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        let what = shell_extension_label(shell);

        // Show the config content that will be added with gutter
        // Fish: show the wrapper (it's a complete file that sources the full function)
        // Other shells: show the one-liner that gets appended
        let content = if matches!(shell, Shell::Fish) {
            shell::ShellInit::with_prefix(shell, cmd.to_string())
                .generate_fish_wrapper()
                .unwrap_or_else(|_| result.config_line.clone())
        } else {
            result.config_line.clone()
        };
        let nushell_note = if matches!(shell, Shell::Nushell) {
            format!("\n{}", hint_message("Nushell support is experimental"))
        } else {
            String::new()
        };

        blocks.push(format!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}\n{}{nushell_note}",
            result.action.symbol(),
            result.action.description(),
            format_bash_with_gutter(&content),
        ));
    }

    // Show completion changes (only fish has separate completion files)
    for result in completion_results {
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        // Show the completion content that will be written
        let fish_completion = fish_completion_content(cmd);
        blocks.push(format!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}\n{}",
            result.action.symbol(),
            result.action.description(),
            format_bash_with_gutter(fish_completion.trim()),
        ));
    }

    // Show legacy files that will be removed. The canonical replacement is the
    // path this shell is being configured at (found in `results`), matching the
    // "now using <new>" the after-the-fact removal message shows.
    for (shell, legacy_path) in legacy_cleanups {
        let old_path = format_path_for_display(legacy_path);
        let new_path = results
            .iter()
            .find(|r| r.shell == *shell)
            .map(|r| format_path_for_display(&r.path))
            .unwrap_or_default();
        blocks.push(format!(
            "{INFO_SYMBOL} Will remove {bold}{old_path}{bold:#} (deprecated; now using {bold}{new_path}{bold:#})",
        ));
    }

    blocks.join("\n\n")
}

/// Format what will be uninstalled (shell extensions and completions), naming
/// the individual rc-file lines under each entry.
///
/// Returns the preview as a string (no trailing newline). The caller picks the
/// stream: `--dry-run` mutates nothing, so its preview is the command's answer
/// and goes to stdout; the same text inside the confirmation prompt is
/// narration and goes to stderr. See /writing-user-outputs.
pub fn show_uninstall_preview(
    results: &[UninstallResult],
    completion_results: &[CompletionUninstallResult],
) -> String {
    let bold = Style::new().bold();
    let mut lines: Vec<String> = Vec::new();

    for result in results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        // Deprecated files get a different message format
        if let Some(canonical) = &result.superseded_by {
            let canonical_path = format_path_for_display(canonical);
            lines.push(format!(
                "{INFO_SYMBOL} {} {bold}{path}{bold:#} (deprecated; now using {bold}{canonical_path}{bold:#})",
                result.action.description(),
            ));
        } else {
            let what = shell_extension_label(shell);

            lines.push(format!(
                "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}{}",
                result.action.symbol(),
                result.action.description(),
                format_matched_lines(&result.matched_lines),
            ));
        }
    }

    for result in completion_results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        lines.push(format!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ));
    }

    lines.join("\n")
}

/// The rc-file lines an uninstall entry covers, as a gutter block under the
/// entry's own line. Empty for a whole file worktrunk owns, whose path is
/// already the whole story.
pub(crate) fn format_matched_lines(matched_lines: &[String]) -> String {
    if matched_lines.is_empty() {
        return String::new();
    }
    format!("\n{}", format_bash_with_gutter(&matched_lines.join("\n")))
}

/// Prompt for install with [y/N/?] options
///
/// - `y` or `yes`: Accept and return true
/// - `n`, `no`, or empty: Decline and return false
/// - `?`: Show preview (via show_install_preview) and re-prompt
pub fn prompt_for_install(
    results: &[ConfigureResult],
    completion_results: &[CompletionResult],
    legacy_cleanups: &[(Shell, PathBuf)],
    cmd: &str,
    prompt_text: &str,
) -> Result<bool, String> {
    let response = prompt_yes_no_preview(prompt_text, || {
        // Mid-prompt re-preview is narration, so it goes to stderr (the trailing
        // blank separates it from the re-prompt). See /writing-user-outputs.
        eprintln!(
            "{}\n",
            show_install_preview(results, completion_results, legacy_cleanups, cmd)
        );
    })
    .map_err(|e| e.to_string())?;

    Ok(response == PromptResponse::Accepted)
}

/// Prompt user for yes/no confirmation (simple [y/N] prompt)
fn prompt_yes_no() -> Result<bool, String> {
    // Blank line before prompt for visual separation
    eprintln!();
    eprint!(
        "{} ",
        prompt_message(color_print::cformat!("Proceed? <bold>[y/N]</>"))
    );
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Fish completion content - finds command in PATH, with WORKTRUNK_BIN as optional override
pub(crate) fn fish_completion_content(cmd: &str) -> String {
    format!(
        r#"# worktrunk completions for fish
complete --keep-order --exclusive --command {cmd} --arguments "(test -n \"\$WORKTRUNK_BIN\"; or set -l WORKTRUNK_BIN (type -P {cmd} 2>/dev/null); and COMPLETE=fish \$WORKTRUNK_BIN -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"
"#
    )
}

/// Process shell completions - either preview or write based on dry_run flag
///
/// Note: Bash and Zsh use inline lazy completions in the init script.
/// Fish uses a separate completion file at ~/.config/fish/completions/{cmd}.fish
/// that finds the command in PATH (with WORKTRUNK_BIN as optional override) to bypass the shell wrapper.
pub fn process_shell_completions(
    shells: &[Shell],
    dry_run: bool,
    cmd: &str,
) -> Result<Vec<CompletionResult>, String> {
    let mut results = Vec::new();
    let fish_completion = fish_completion_content(cmd);

    for &shell in shells {
        // Only fish has a separate completion file
        if shell != Shell::Fish {
            continue;
        }

        let completion_path = shell
            .completion_path(cmd)
            .map_err(|e| format!("Failed to get completion path for {shell}: {e}"))?;

        // Check if completions already exist with correct content
        // Read errors (including not-found) fall through to "not configured"
        if let Ok(existing) = fs::read_to_string(&completion_path)
            && existing == fish_completion
        {
            results.push(CompletionResult {
                shell,
                path: completion_path,
                action: ConfigAction::AlreadyExists,
            });
            continue;
        }

        if dry_run {
            let action = if completion_path.exists() {
                ConfigAction::WouldAdd
            } else {
                ConfigAction::WouldCreate
            };
            results.push(CompletionResult {
                shell,
                path: completion_path,
                action,
            });
            continue;
        }

        // Create parent directory if needed
        if let Some(parent) = completion_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create directory {}: {e}",
                    format_path_for_display(parent)
                )
            })?;
        }

        // Write the completion file
        write_atomically(&completion_path, &fish_completion).map_err(|e| {
            format!(
                "Failed to write {}: {e}",
                format_path_for_display(&completion_path)
            )
        })?;

        results.push(CompletionResult {
            shell,
            path: completion_path,
            action: ConfigAction::Created,
        });
    }

    Ok(results)
}

pub fn handle_unconfigure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
    dry_run: bool,
) -> Result<UninstallScanResult, String> {
    // First, do a dry-run to see what would be changed
    let preview = scan_for_uninstall(shell_filter, true)?;

    // If nothing to do, return early
    if preview.results.is_empty() && preview.completion_results.is_empty() {
        return Ok(preview);
    }

    // For --dry-run, show preview and return without prompting or applying. The
    // early-return above guarantees at least one result, and
    // show_uninstall_preview emits a line per result, so the preview is never
    // empty here (unlike the install path, where AlreadyExists entries are
    // skipped and the preview can be empty). The preview is the command's
    // answer, so it goes to stdout. See /writing-user-outputs.
    if dry_run {
        println!(
            "{}",
            show_uninstall_preview(&preview.results, &preview.completion_results)
        );
        return Ok(preview);
    }

    // Show what will be done and ask for confirmation (unless --yes flag is used)
    if !skip_confirmation
        && !prompt_for_uninstall_confirmation(&preview.results, &preview.completion_results)?
    {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --yes flag was used), now actually apply the changes
    scan_for_uninstall(shell_filter, false)
}

/// Remove a config file with a context-rich error message.
fn remove_config_file(path: &std::path::Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|e| format!("Failed to remove {}: {e}", format_path_for_display(path)))
}

/// Uninstall scans the shell-owned directories and removes every worktrunk-managed
/// file or line, regardless of the binary name it was installed under.
///
/// For Fish/Nushell wrapper files and Fish completion files (one file per binary
/// name), this lists the owning directories and admits any file whose content
/// matches the worktrunk marker (`is_worktrunk_managed_content`, or the
/// completion header for `completions/`). For Bash/Zsh/PowerShell (line-based),
/// it scans the rc/profile files and uses `is_shell_integration_line_for_uninstall_any_cmd`.
/// No `--cmd` is needed because the marker is the content, not the file name.
fn scan_for_uninstall(
    shell_filter: Option<Shell>,
    dry_run: bool,
) -> Result<UninstallScanResult, String> {
    // For uninstall, scan every shell (Shell::all includes PowerShell) to clean
    // up any existing profiles.
    let default_shells = Shell::all();

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let home =
        shell::home_dir_required().map_err(|e| format!("Cannot determine home directory: {e}"))?;

    let mut results = Vec::new();
    let mut not_found = Vec::new();

    for &shell in &shells {
        match shell {
            Shell::Fish => {
                let functions_dir = home.join(".config").join("fish").join("functions");
                let confd_dir = home.join(".config").join("fish").join("conf.d");

                let canonical =
                    scan_managed_files(&functions_dir, "fish", is_worktrunk_managed_content)?;
                let legacy = scan_managed_files(&confd_dir, "fish", is_worktrunk_managed_content)?;
                let found_any = !canonical.is_empty() || !legacy.is_empty();

                for path in &canonical {
                    let action = if dry_run {
                        UninstallAction::WouldRemove
                    } else {
                        remove_config_file(path)?;
                        UninstallAction::Removed
                    };
                    results.push(UninstallResult {
                        shell,
                        path: path.clone(),
                        action,
                        superseded_by: None,
                        matched_lines: Vec::new(),
                    });
                }

                for path in &legacy {
                    let superseded_by = path.file_name().map(|n| functions_dir.join(n));
                    let action = if dry_run {
                        UninstallAction::WouldRemove
                    } else {
                        remove_config_file(path)?;
                        UninstallAction::Removed
                    };
                    results.push(UninstallResult {
                        shell,
                        path: path.clone(),
                        action,
                        superseded_by,
                        matched_lines: Vec::new(),
                    });
                }

                if !found_any {
                    not_found.push((shell, functions_dir));
                }
            }

            Shell::Nushell => {
                let mut found_any = false;
                let candidates = shell::nushell_autoload_candidates(&home);
                for autoload_dir in &candidates {
                    let nu_files =
                        scan_managed_files(autoload_dir, "nu", is_worktrunk_managed_content)?;
                    for path in &nu_files {
                        found_any = true;
                        let action = if dry_run {
                            UninstallAction::WouldRemove
                        } else {
                            remove_config_file(path)?;
                            UninstallAction::Removed
                        };
                        results.push(UninstallResult {
                            shell,
                            path: path.clone(),
                            action,
                            superseded_by: None,
                            matched_lines: Vec::new(),
                        });
                    }
                }
                if !found_any {
                    // Report the first candidate's autoload dir as the expected location
                    if let Some(first) = candidates.first() {
                        not_found.push((shell, first.clone()));
                    }
                }
            }

            Shell::Bash | Shell::Zsh | Shell::PowerShell => {
                let paths = shell::line_based_config_paths(shell, &home);
                let mut found = false;

                for path in &paths {
                    if !path.exists() {
                        continue;
                    }

                    if let Some(result) = uninstall_from_file(shell, path, dry_run)? {
                        results.push(result);
                        found = true;
                    }
                }

                if !found && let Some(first_path) = paths.first() {
                    not_found.push((shell, first_path.clone()));
                }
            }
        }
    }

    // Fish completion files carry their own header marker, so they are scanned
    // the same way as the wrappers — a completion whose wrapper is already gone
    // is still cleaned up.
    let mut completion_results = Vec::new();
    let mut completion_not_found = Vec::new();
    if shells.contains(&Shell::Fish) {
        let completions_dir = home.join(".config").join("fish").join("completions");
        let completions =
            scan_managed_files(&completions_dir, "fish", |c| c.contains(COMPLETION_MARKER))?;
        if completions.is_empty() {
            completion_not_found.push((Shell::Fish, completions_dir));
        }
        for path in completions {
            let action = if dry_run {
                UninstallAction::WouldRemove
            } else {
                remove_config_file(&path)?;
                UninstallAction::Removed
            };
            completion_results.push(CompletionUninstallResult {
                shell: Shell::Fish,
                path,
                action,
            });
        }
    }

    Ok(UninstallScanResult {
        results,
        completion_results,
        not_found,
        completion_not_found,
    })
}

/// List `*.{extension}` files in `dir` whose content `is_managed` accepts.
/// Returns empty if `dir` does not exist; sorted so removal order is stable.
fn scan_managed_files(
    dir: &Path,
    extension: &str,
    is_managed: fn(&str) -> bool,
) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read {}: {e}", format_path_for_display(dir)))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("Failed to read {}: {e}", format_path_for_display(dir)))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(extension) {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue, // unreadable file: skip, don't fail the whole uninstall
        };
        if is_managed(&content) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn uninstall_from_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
) -> Result<Option<UninstallResult>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

    let lines: Vec<&str> = content.lines().collect();
    let integration_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| shell::is_shell_integration_line_for_uninstall_any_cmd(line))
        .map(|(i, line)| (i, *line))
        .collect();

    if integration_lines.is_empty() {
        return Ok(None);
    }

    let matched_lines: Vec<String> = integration_lines
        .iter()
        .map(|(_, line)| line.trim().to_string())
        .collect();

    if dry_run {
        return Ok(Some(UninstallResult {
            shell,
            path: path.to_path_buf(),
            action: UninstallAction::WouldRemove,
            superseded_by: None,
            matched_lines,
        }));
    }

    // Remove matching lines and any immediately preceding blank line
    // (install adds "\n{line}\n", so we remove both the blank and the integration line)
    let mut indices_to_remove: HashSet<usize> = integration_lines.iter().map(|(i, _)| *i).collect();
    for &(i, _) in &integration_lines {
        if i > 0 && lines[i - 1].trim().is_empty() {
            indices_to_remove.insert(i - 1);
        }
    }
    let new_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices_to_remove.contains(i))
        .map(|(_, line)| *line)
        .collect();

    let new_content = new_lines.join("\n");
    // Preserve trailing newline if original had one
    let new_content = if content.ends_with('\n') {
        format!("{}\n", new_content)
    } else {
        new_content
    };

    write_atomically(path, &new_content)
        .map_err(|e| format!("Failed to write {}: {e}", format_path_for_display(path)))?;

    Ok(Some(UninstallResult {
        shell,
        path: path.to_path_buf(),
        action: UninstallAction::Removed,
        superseded_by: None,
        matched_lines,
    }))
}

/// Show what uninstall would take, then ask.
///
/// Renders through `show_uninstall_preview` so the confirmation lists the exact
/// rc-file lines `--dry-run` lists — the user approves the lines, not just the
/// file. Shown inside the prompt, so it narrates to stderr rather than joining
/// the `--dry-run` answer on stdout. See /writing-user-outputs.
fn prompt_for_uninstall_confirmation(
    results: &[UninstallResult],
    completion_results: &[CompletionUninstallResult],
) -> Result<bool, String> {
    eprintln!("{}", show_uninstall_preview(results, completion_results));

    prompt_yes_no()
}

/// Show samples of all output message types
pub fn handle_show_theme() {
    use color_print::cformat;
    use worktrunk::styling::{
        error_message, hint_message, info_message, progress_message, success_message,
    };

    // Progress
    eprintln!(
        "{}",
        progress_message(cformat!("Rebasing <bold>feature</> onto <bold>main</>..."))
    );

    // Success
    eprintln!(
        "{}",
        success_message(cformat!(
            "Created worktree for <bold>feature</> @ <bold>/path/to/worktree</>"
        ))
    );

    // Error
    eprintln!(
        "{}",
        error_message(cformat!("Branch <bold>feature</> not found"))
    );

    // Warning
    eprintln!(
        "{}",
        warning_message(cformat!("Branch <bold>feature</> has uncommitted changes"))
    );

    // Hint
    eprintln!(
        "{}",
        hint_message(cformat!("To rebase onto main, run <underline>wt merge</>"))
    );

    // Info
    eprintln!("{}", info_message(cformat!("Showing <bold>5</> worktrees")));

    eprintln!();

    // Gutter - error details (plain text, no syntax highlighting)
    eprintln!("{}", info_message("Gutter formatting (error details):"));
    eprintln!(
        "{}",
        format_with_gutter("expected `=`, found newline at line 3 column 1", None,)
    );

    eprintln!();

    // Gutter - TOML config (syntax highlighted)
    eprintln!("{}", info_message("Gutter formatting (config):"));
    eprintln!(
        "{}",
        format_toml("[commit.generation]\ncommand = \"llm --model claude\"")
    );

    eprintln!();

    // Gutter - bash code (short, long wrapping, multi-line string, multi-line command, and template)
    eprintln!("{}", info_message("Gutter formatting (shell code):"));
    eprintln!(
        "{}",
        format_bash_with_gutter(
            "eval \"$(wt config shell init bash)\"\necho 'This is a long command that will wrap to the next line when the terminal is narrow enough to require wrapping.'\necho 'hello\nworld'\ncargo build --release &&\ncargo test\ncp {{ repo_root }}/target {{ worktree }}/target"
        )
    );

    eprintln!();

    // Prompt
    eprintln!("{}", info_message("Prompt formatting:"));
    eprintln!("{} ", prompt_message("Proceed? [y/N]"));

    eprintln!();

    // Color palette — each color rendered in itself
    eprintln!("{}", info_message("Color palette:"));
    use anstyle::{AnsiColor, Color};
    let fg = |c: AnsiColor| Some(Color::Ansi(c));
    let palette: &[(&str, Style)] = &[
        ("red", Style::new().fg_color(fg(AnsiColor::Red))),
        ("green", Style::new().fg_color(fg(AnsiColor::Green))),
        ("yellow", Style::new().fg_color(fg(AnsiColor::Yellow))),
        ("blue", Style::new().fg_color(fg(AnsiColor::Blue))),
        ("cyan", Style::new().fg_color(fg(AnsiColor::Cyan))),
        ("bold", Style::new().bold()),
        ("dim", Style::new().dimmed()),
        ("bold red", Style::new().fg_color(fg(AnsiColor::Red)).bold()),
        (
            "bold green",
            Style::new().fg_color(fg(AnsiColor::Green)).bold(),
        ),
        (
            "bold yellow",
            Style::new().fg_color(fg(AnsiColor::Yellow)).bold(),
        ),
        (
            "bold cyan",
            Style::new().fg_color(fg(AnsiColor::Cyan)).bold(),
        ),
        (
            "dim bright-black",
            Style::new().fg_color(fg(AnsiColor::BrightBlack)).dimmed(),
        ),
        (
            "dim blue",
            Style::new().fg_color(fg(AnsiColor::Blue)).dimmed(),
        ),
        (
            "dim green",
            Style::new().fg_color(fg(AnsiColor::Green)).dimmed(),
        ),
        (
            "dim cyan",
            Style::new().fg_color(fg(AnsiColor::Cyan)).dimmed(),
        ),
        (
            "dim magenta",
            Style::new().fg_color(fg(AnsiColor::Magenta)).dimmed(),
        ),
        (
            "dim yellow",
            Style::new().fg_color(fg(AnsiColor::Yellow)).dimmed(),
        ),
    ];

    let palette_text: String = palette
        .iter()
        .map(|(name, style)| format!("{style}{name}{style:#}"))
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!("{}", format_with_gutter(&palette_text, None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninstall_action_description() {
        assert_eq!(UninstallAction::Removed.description(), "Removed");
        assert_eq!(UninstallAction::WouldRemove.description(), "Will remove");
    }

    #[test]
    fn test_uninstall_action_emoji() {
        assert_eq!(UninstallAction::Removed.symbol(), SUCCESS_SYMBOL);
        assert_eq!(UninstallAction::WouldRemove.symbol(), INFO_SYMBOL);
    }

    #[test]
    fn test_config_action_description() {
        assert_eq!(ConfigAction::Added.description(), "Added");
        assert_eq!(
            ConfigAction::AlreadyExists.description(),
            "Already configured"
        );
        assert_eq!(ConfigAction::Created.description(), "Created");
        assert_eq!(ConfigAction::WouldAdd.description(), "Will add");
        assert_eq!(ConfigAction::WouldCreate.description(), "Will create");
    }

    #[test]
    fn test_config_action_emoji() {
        assert_eq!(ConfigAction::Added.symbol(), SUCCESS_SYMBOL);
        assert_eq!(ConfigAction::Created.symbol(), SUCCESS_SYMBOL);
        assert_eq!(ConfigAction::AlreadyExists.symbol(), INFO_SYMBOL);
        assert_eq!(ConfigAction::WouldAdd.symbol(), INFO_SYMBOL);
        assert_eq!(ConfigAction::WouldCreate.symbol(), INFO_SYMBOL);
    }

    #[test]
    fn test_is_shell_integration_line() {
        // Valid integration lines for "wt"
        assert!(shell::is_shell_integration_line(
            "eval \"$(wt config shell init bash)\"",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "  eval \"$(wt config shell init zsh)\"  ",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "if command -v wt; then eval \"$(wt config shell init bash)\"; fi",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "source <(wt config shell init fish)",
            "wt"
        ));

        // Valid integration lines for "git-wt"
        assert!(shell::is_shell_integration_line(
            "eval \"$(git-wt config shell init bash)\"",
            "git-wt"
        ));
        assert!(!shell::is_shell_integration_line(
            "eval \"$(wt config shell init bash)\"",
            "git-wt"
        ));

        // Not integration lines (comments)
        assert!(!shell::is_shell_integration_line(
            "# eval \"$(wt config shell init bash)\"",
            "wt"
        ));

        // Not integration lines (no eval/source/if)
        assert!(!shell::is_shell_integration_line(
            "wt config shell init bash",
            "wt"
        ));
        assert!(!shell::is_shell_integration_line(
            "echo wt config shell init bash",
            "wt"
        ));
    }

    #[test]
    fn test_fish_completion_content() {
        insta::assert_snapshot!(fish_completion_content("wt"));
    }

    #[test]
    fn test_fish_completion_content_custom_cmd() {
        insta::assert_snapshot!(fish_completion_content("myapp"));
    }

    // Note: should_auto_configure_powershell() is tested via WORKTRUNK_TEST_POWERSHELL_ENV
    // override in tests/integration_tests/configure_shell.rs.

    #[test]
    fn test_fish_code_lines_strips_comments_and_blanks() {
        let source = "# comment\n\nfunction wt\n    command wt $argv\nend\n";
        assert_eq!(
            fish_code_lines(source),
            vec!["function wt", "command wt $argv", "end"]
        );
    }

    #[test]
    fn test_fish_code_lines_matches_despite_different_comments() {
        let old = "# Docs: https://worktrunk.dev/docs/shell-integration\nfunction wt\n    command wt $argv\nend";
        let new = "# Docs: https://worktrunk.dev/config/#shell-integration\nfunction wt\n    command wt $argv\nend";
        assert_eq!(fish_code_lines(old), fish_code_lines(new));
    }

    #[test]
    fn test_managed_content_matches_wrappers_not_mentions() {
        // Ours: the header every wrapper template carries, under any binary
        // name, plus the headerless legacy `conf.d/{cmd}.fish`, whose code
        // lines are nothing but the init invocation.
        for content in [
            "# worktrunk shell integration for fish\nfunction git-wt\nend\n",
            "# worktrunk shell integration for nushell\ndef --env wt [] {}\n",
            "wt config shell init fish | source",
            "# added by worktrunk\nif type -q wt; command wt config shell init fish | source; end\n",
        ] {
            assert!(is_worktrunk_managed_content(content), "{content}");
        }

        // Not ours: the uninstall scan walks directories the user owns and has
        // only the contents to go on, so a file that merely mentions the command
        // must survive — as must a user's own file that runs the init line amid
        // other code, which a per-line any-match would delete whole. The second
        // and third also carry the init command and a `| source` somewhere,
        // which is all a whole-file substring test asks before deleting.
        for content in [
            "function notes\n    echo run wt config shell init fish to set up\nend\n",
            "# reminder: wt config shell init fish\nfunction helpers\n    cat ~/.aliases | source\nend\n",
            "function helper\n    command wt config shell init fish | source\nend\n",
            "",
        ] {
            assert!(!is_worktrunk_managed_content(content), "{content}");
        }
    }

    #[test]
    fn test_line_based_config_paths_fish_and_nushell_have_no_line_files() {
        // Fish and Nushell are configured via wrapper files, not line-based rc
        // edits, so the line-based path list is empty for them.
        let home = Path::new("/home/user");
        assert!(shell::line_based_config_paths(Shell::Fish, home).is_empty());
        assert!(shell::line_based_config_paths(Shell::Nushell, home).is_empty());
    }

    #[test]
    fn test_scan_managed_files_skips_wrong_extension_and_directories() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // Wrong extension: skipped.
        fs::write(root.join("notes.txt"), "anything").unwrap();
        // A directory carrying the wrapper extension: skipped (not a file).
        fs::create_dir(root.join("nested.fish")).unwrap();
        assert!(
            scan_managed_files(root, "fish", is_worktrunk_managed_content)
                .unwrap()
                .is_empty()
        );
    }

    /// An rc file with one integration line and content the user owns on either
    /// side of it. Unix-only, like the atomic-rewrite tests that use it: mode
    /// and symlink semantics have no Windows equivalent.
    #[cfg(unix)]
    fn write_rc(path: &Path) {
        fs::write(
            path,
            "export EDITOR=hx\n\neval \"$(wt config shell init bash)\"\nalias ll='ls -l'\n",
        )
        .unwrap();
    }

    #[cfg(unix)]
    const RC_AFTER_UNINSTALL: &str = "export EDITOR=hx\nalias ll='ls -l'\n";

    #[cfg(unix)]
    #[test]
    fn test_uninstall_from_file_preserves_mode_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let rc = dir.path().join(".bashrc");
        write_rc(&rc);
        // 0644 is the usual rc mode and differs from the 0600 a fresh temp file
        // is created with, so an uncopied mode shows up as a diff.
        fs::set_permissions(&rc, fs::Permissions::from_mode(0o644)).unwrap();

        uninstall_from_file(Shell::Bash, &rc, false)
            .unwrap()
            .unwrap();

        assert_eq!(fs::read_to_string(&rc).unwrap(), RC_AFTER_UNINSTALL);
        assert_eq!(
            fs::metadata(&rc).unwrap().permissions().mode() & 0o777,
            0o644
        );
        // The temp file is renamed, not left beside the target.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_uninstall_from_file_writes_through_symlink() {
        // A dotfile manager's layout: the real file lives in a repo and `~/.bashrc`
        // is a link to it. The rewrite must follow the link, not replace it —
        // otherwise the user's rc silently stops tracking their dotfiles.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("dotfiles");
        let home = dir.path().join("home");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&home).unwrap();
        let real = repo.join("bashrc");
        let link = home.join(".bashrc");
        write_rc(&real);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        uninstall_from_file(Shell::Bash, &link, false)
            .unwrap()
            .unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), RC_AFTER_UNINSTALL);
        // The temp file was created next to the resolved target, so a rename
        // across filesystems can't silently downgrade to a copy.
        assert_eq!(fs::read_dir(&repo).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_uninstall_from_file_leaves_rc_intact_when_write_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let rc = dir.path().join(".bashrc");
        write_rc(&rc);
        let original = fs::read_to_string(&rc).unwrap();
        // Read and traverse still work, so the rewrite gets as far as creating
        // its temp file and fails there — the point a truncate-in-place write
        // would already have emptied the file.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();

        let result = uninstall_from_file(Shell::Bash, &rc, false);

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            result.is_err(),
            "expected the rewrite to fail in a read-only directory"
        );
        assert_eq!(fs::read_to_string(&rc).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_managed_files_skips_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let unreadable = root.join("locked.fish");
        fs::write(&unreadable, "function wt\nend\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        // An unreadable wrapper file is skipped, not surfaced as an error.
        let found = scan_managed_files(root, "fish", is_worktrunk_managed_content);
        // Restore perms so TempDir cleanup can remove the file.
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(found.unwrap().is_empty());
    }
}
