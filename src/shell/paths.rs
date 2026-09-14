//! Shell configuration and completion paths.
//!
//! This module handles locating shell configuration files (e.g., `.bashrc`, `.zshrc`)
//! and completion directories for different shells.

use etcetera::base_strategy::{BaseStrategy, Xdg, choose_base_strategy};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::path::home_dir;

/// Get the user's home directory or return an error.
pub fn home_dir_required() -> Result<PathBuf, std::io::Error> {
    home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot determine home directory. Set $HOME (Unix) or $USERPROFILE (Windows)",
        )
    })
}

/// Test override pinning the Nushell vendor-autoload directory.
///
/// Set by integration tests so the install target is deterministic across
/// platforms (and independent of whether `nu` is on PATH). Mirrors the
/// `WORKTRUNK_TEST_*` overrides consulted by `Shell::is_installed`.
const TEST_NU_VENDOR_AUTOLOAD_ENV: &str = "WORKTRUNK_TEST_NU_VENDOR_AUTOLOAD_DIR";

/// The Nushell directories worktrunk resolves from `nu`, queried at most once
/// per process.
///
/// Nushell autoloads `*.nu` files from `$nu.vendor-autoload-dirs`; the last
/// entry is the user-writable one (under `$nu.data-dir`) and is worktrunk's
/// install target. `$nu.default-config-dir` is kept only to locate files
/// stranded by older worktrunk versions, which installed under
/// `<default-config-dir>/vendor/autoload` — a path Nushell never autoloads
/// (issue #2878).
#[derive(Clone, Default)]
struct NuDirs {
    /// `$nu.vendor-autoload-dirs | last`. `None` when `nu` can't be queried.
    vendor_autoload: Option<PathBuf>,
    /// `$nu.default-config-dir` (legacy install root). `None` when unavailable.
    default_config: Option<PathBuf>,
}

/// Parse a single trimmed path line from `nu` stdout.
///
/// Returns `None` for empty / whitespace-only input.
fn parse_nu_path(line: &str) -> Option<PathBuf> {
    let path = PathBuf::from(line.trim());
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Query `nu` for the directories worktrunk cares about, spawning `nu` at most
/// once per process (memoised).
///
/// Returns [`NuDirs::default`] (all `None`) when `nu` is not in PATH or the
/// query fails. A single `wt config shell install` resolves both the write
/// target (`completion_path`) and the installed-state candidates
/// (`config_paths`), which would otherwise each spawn `nu` for the same answer.
fn nu_dirs() -> NuDirs {
    static CACHE: OnceLock<NuDirs> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            // Test override: skip spawning `nu` for deterministic, offline tests.
            if let Some(dir) = std::env::var_os(TEST_NU_VENDOR_AUTOLOAD_ENV) {
                return NuDirs {
                    vendor_autoload: parse_nu_path(&dir.to_string_lossy()),
                    default_config: None,
                };
            }

            let Some(output) = crate::shell_exec::Cmd::new("nu")
                .args([
                    "-c",
                    "print ($nu.vendor-autoload-dirs | last); print $nu.default-config-dir",
                ])
                .run()
                .ok()
                .filter(|o| o.status.success())
            else {
                return NuDirs::default();
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut lines = stdout.lines();
            NuDirs {
                vendor_autoload: lines.next().and_then(parse_nu_path),
                default_config: lines.next().and_then(parse_nu_path),
            }
        })
        .clone()
}

/// `$nu.vendor-autoload-dirs | last` computed without spawning `nu`.
///
/// Delegates to `nu-config`, the crate Nushell itself resolves these paths
/// with: [`resolve_paths`](nu_config::resolve_paths) applies Nushell's own
/// rules for `$XDG_DATA_HOME`, `$XDG_DATA_DIRS`, the platform data directory
/// and `$NU_VENDOR_AUTOLOAD_DIR`, and its last entry is the user-writable one
/// — the same expression [`nu_dirs`] asks `nu` for. Nothing about Nushell's
/// layout is restated here, so the rules can't drift out of sync.
///
/// It answers for the `nu-config` release pinned in `Cargo.toml` rather than
/// for the installed `nu`, which is why [`nu_dirs`] is still preferred when
/// `nu` is on PATH; this is the offline fallback, and the one answer asking
/// the tool can't give.
///
/// `None` only when no home directory is discoverable. Both callers reach here
/// via [`home_dir_required`], which already failed in that case, so there is no
/// last-resort path to guess.
fn nushell_vendor_autoload_fallback() -> Option<PathBuf> {
    let (dirs, _warnings) =
        nu_config::resolve_paths(&nu_config::SystemEnv, &nu_config::CliOverrides::default())
            .ok()?;
    dirs.vendor_autoload_dirs.last().cloned()
}

/// Legacy `<config-dir>/vendor/autoload` directories where older worktrunk
/// versions wrongly installed the Nushell wrapper (issue #2878).
///
/// Returned so install/uninstall can find and remove files stranded there.
/// Mirrors the candidate set the buggy code wrote to: the queried
/// `$nu.default-config-dir`, then `$XDG_CONFIG_HOME`, `~/.config`, and
/// etcetera's base config dir — each under `nushell/vendor/autoload`.
fn legacy_nushell_autoload_dirs(
    home: &std::path::Path,
    default_config: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = default_config {
        dirs.push(dir.to_path_buf());
    }
    // etcetera's XDG strategy: `$XDG_CONFIG_HOME` when absolute, `~/.config`
    // otherwise. Reading the variable directly would put a bare
    // `nushell/vendor/autoload` into the stranded-file search when the value is
    // empty or relative, which then looks under the invocation directory
    // instead of a config dir. `~/.config` stays listed separately because an
    // absolute `$XDG_CONFIG_HOME` makes `xdg.config_dir()` that directory,
    // while `~/.config/nushell` is still a location older worktrunk wrote to.
    if let Ok(xdg) = Xdg::new() {
        dirs.push(xdg.config_dir().join("nushell"));
    }
    dirs.push(home.join(".config").join("nushell"));
    if let Ok(strategy) = choose_base_strategy() {
        dirs.push(strategy.config_dir().join("nushell"));
    }
    dirs.into_iter()
        .map(|d| d.join("vendor").join("autoload"))
        .collect()
}

/// Nushell autoload directories to check for an installed wrapper, in priority
/// order.
///
/// The first entry is the canonical write target (the current vendor-autoload
/// dir); the rest are the offline fallback and the legacy config-dir locations
/// kept so install/uninstall can clean up stranded files.
pub fn nushell_autoload_candidates(home: &std::path::Path) -> Vec<PathBuf> {
    let dirs = nu_dirs();
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(dirs.vendor_autoload.clone());
    // Also listed when `nu` answered, in case it was queryable at install time
    // but not now; identical to the entry above when it wasn't, and deduped.
    candidates.extend(nushell_vendor_autoload_fallback());
    candidates.extend(legacy_nushell_autoload_dirs(
        home,
        dirs.default_config.as_deref(),
    ));

    // Deduplicate while preserving priority order (write target first).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));
    candidates
}

/// Get PowerShell profile paths in order of preference.
/// On Windows, returns both PowerShell Core (7+) and Windows PowerShell (5.1) paths.
/// On Unix, uses the conventional ~/.config/powershell location.
pub fn powershell_profile_paths(home: &std::path::Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // Use platform-specific Documents path (handles non-English Windows)
        let docs = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
        vec![
            // PowerShell Core 6+ (pwsh.exe) - preferred
            docs.join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
            // Windows PowerShell 5.1 (powershell.exe) - legacy but still common
            docs.join("WindowsPowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![
            home.join(".config")
                .join("powershell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ]
    }
}

/// Rc/profile files scanned line-by-line for integration lines.
///
/// Bash/Zsh/PowerShell integration is one line in an rc file, so these paths
/// are independent of the binary name; Fish and Nushell use per-name wrapper
/// files instead and have no line-based config. `config_paths` builds on this
/// for the line-based shells, so install and uninstall cannot drift apart.
pub fn line_based_config_paths(shell: super::Shell, home: &std::path::Path) -> Vec<PathBuf> {
    match shell {
        super::Shell::Bash => {
            // Use .bashrc - sourced by interactive shells (login shells should source .bashrc)
            vec![home.join(".bashrc")]
        }
        super::Shell::Zsh => {
            let zdotdir = std::env::var("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.to_path_buf());
            vec![zdotdir.join(".zshrc")]
        }
        super::Shell::PowerShell => powershell_profile_paths(home),
        super::Shell::Fish | super::Shell::Nushell => Vec::new(),
    }
}

/// Returns the config file paths for a shell.
///
/// The `cmd` parameter affects the Fish functions filename (e.g., `wt.fish` or `git-wt.fish`).
/// Returns paths in order of preference. The first existing file should be used.
pub(super) fn config_paths(shell: super::Shell, cmd: &str) -> Result<Vec<PathBuf>, std::io::Error> {
    let home = home_dir_required()?;

    Ok(match shell {
        super::Shell::Bash | super::Shell::Zsh | super::Shell::PowerShell => {
            line_based_config_paths(shell, &home)
        }
        super::Shell::Fish => {
            // For fish, we write to functions/ which is autoloaded on first use.
            // This ensures PATH is fully configured before our function loads,
            // fixing the issue where Homebrew PATH setup in config.fish runs
            // after conf.d/ files. See: https://github.com/max-sixty/worktrunk/issues/566
            vec![
                home.join(".config")
                    .join("fish")
                    .join("functions")
                    .join(format!("{}.fish", cmd)),
            ]
        }
        super::Shell::Nushell => {
            // Nushell autoloads `*.nu` from `$nu.vendor-autoload-dirs`; the last
            // entry (under `$nu.data-dir`) is the write target. Earlier entries
            // are fallbacks plus the legacy `<config-dir>/vendor/autoload`
            // locations older worktrunk wrote to (never autoloaded — issue
            // #2878), kept so install/uninstall can clean them up.
            nushell_autoload_candidates(&home)
                .into_iter()
                .map(|autoload_dir| autoload_dir.join(format!("{}.nu", cmd)))
                .collect()
        }
    })
}

/// Returns the legacy fish conf.d path for cleanup purposes.
///
/// Previously, fish shell integration was installed to `~/.config/fish/conf.d/{cmd}.fish`.
/// This caused issues with Homebrew PATH setup (see issue #566). We now install to
/// `functions/{cmd}.fish` instead. This method returns the legacy path so install/uninstall
/// can clean it up.
pub(super) fn legacy_fish_conf_d_path(cmd: &str) -> Result<PathBuf, std::io::Error> {
    let home = home_dir_required()?;
    Ok(home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join(format!("{}.fish", cmd)))
}

/// Returns the path to the native completion directory for a shell.
///
/// The `cmd` parameter affects the completion filename (e.g., `wt.fish` or `git-wt.fish`).
///
/// Note: Bash and Zsh use inline lazy completions in the init script.
/// Only Fish uses a separate completion file at ~/.config/fish/completions/
/// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
/// the shell function wrapper.
pub(super) fn completion_path(shell: super::Shell, cmd: &str) -> Result<PathBuf, std::io::Error> {
    let home = home_dir_required()?;

    // Use etcetera for XDG-compliant paths when available
    let strategy = choose_base_strategy().ok();

    Ok(match shell {
        super::Shell::Bash => {
            // XDG_DATA_HOME defaults to ~/.local/share
            let data_home = strategy
                .as_ref()
                .map(|s| s.data_dir())
                .unwrap_or_else(|| home.join(".local").join("share"));
            data_home
                .join("bash-completion")
                .join("completions")
                .join(cmd)
        }
        super::Shell::Zsh => home.join(".zfunc").join(format!("_{}", cmd)),
        super::Shell::Fish => {
            let config_home = strategy
                .as_ref()
                .map(|s| s.config_dir())
                .unwrap_or_else(|| home.join(".config"));
            config_home
                .join("fish")
                .join("completions")
                .join(format!("{}.fish", cmd))
        }
        super::Shell::Nushell => {
            // Nushell completions are defined inline in the init script.
            // Return the canonical vendor-autoload path (same as config): what
            // `nu` reports, else nu-config's answer for the same expression.
            let dirs = nu_dirs();
            dirs.vendor_autoload
                .clone()
                .or_else(nushell_vendor_autoload_fallback)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Cannot determine Nushell's vendor-autoload directory. Set $HOME (Unix) or $USERPROFILE (Windows)",
                    )
                })?
                .join(format!("{}.nu", cmd))
        }
        super::Shell::PowerShell => {
            // PowerShell doesn't use a separate completion file - completions are
            // registered inline in the profile using Register-ArgumentCompleter
            // Return a dummy path that won't be used
            home.join(format!(".{}-powershell-completions", cmd))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nu_path() {
        assert_eq!(
            parse_nu_path("/home/user/.local/share/nushell/vendor/autoload\n"),
            Some(PathBuf::from(
                "/home/user/.local/share/nushell/vendor/autoload"
            ))
        );
        // Trims surrounding whitespace
        assert_eq!(parse_nu_path("  /a/b  "), Some(PathBuf::from("/a/b")));
        // Empty / whitespace-only
        assert_eq!(parse_nu_path(""), None);
        assert_eq!(parse_nu_path("  \n"), None);
    }

    #[test]
    fn test_nushell_vendor_autoload_fallback_is_a_vendor_autoload_dir() {
        let Some(dir) = nushell_vendor_autoload_fallback() else {
            panic!("nu-config should resolve a vendor-autoload dir when $HOME is set");
        };
        // nu-config owns the rule; what worktrunk depends on is that the last
        // entry is a `vendor/autoload` directory, never a bare config dir (the
        // shape of issue #2878).
        assert!(
            dir.ends_with("vendor/autoload"),
            "fallback should be a vendor/autoload dir: {dir:?}"
        );
    }

    #[test]
    fn test_legacy_nushell_autoload_dirs_are_config_rooted() {
        let home = PathBuf::from("/home/user");
        let default_config = PathBuf::from("/home/user/.config/nushell");
        let dirs = legacy_nushell_autoload_dirs(&home, Some(&default_config));

        // Includes the queried default-config-dir location...
        assert!(
            dirs.contains(&default_config.join("vendor").join("autoload")),
            "should include the queried default-config-dir: {dirs:?}"
        );
        // ...and the XDG default ~/.config/nushell location.
        assert!(
            dirs.contains(&home.join(".config/nushell/vendor/autoload")),
            "should include ~/.config/nushell: {dirs:?}"
        );
        // Every legacy dir is a vendor/autoload dir.
        assert!(
            dirs.iter().all(|p| p.ends_with("vendor/autoload")),
            "all legacy dirs should end with vendor/autoload: {dirs:?}"
        );
    }

    #[test]
    fn test_nushell_autoload_candidates_write_target_first_and_unique() {
        let home = PathBuf::from("/home/user");
        let candidates = nushell_autoload_candidates(&home);

        assert!(!candidates.is_empty(), "must return at least one candidate");
        // The write target (first entry) is a vendor/autoload directory.
        assert!(
            candidates[0].ends_with("vendor/autoload"),
            "write target should be a vendor/autoload dir: {:?}",
            candidates[0]
        );
        // No duplicates.
        let unique: std::collections::HashSet<_> = candidates.iter().collect();
        assert_eq!(
            candidates.len(),
            unique.len(),
            "candidates must not contain duplicates: {candidates:?}"
        );
        // The legacy ~/.config location is always present for cleanup.
        assert!(
            candidates
                .iter()
                .any(|p| p == &home.join(".config/nushell/vendor/autoload")),
            "legacy ~/.config/nushell location must be a candidate: {candidates:?}"
        );
    }
}
