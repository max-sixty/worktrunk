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
/// query fails. A single `wt config shell install` or `wt config show` resolves
/// the Nushell candidates (`config_paths`, the uninstall scan, detection) more
/// than once, which would otherwise each spawn `nu` for the same answer.
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
    // otherwise. `~/.config` stays listed on its own because an absolute
    // `$XDG_CONFIG_HOME` displaces it, and older worktrunk wrote there too.
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
/// The first entry is the canonical write target (the vendor-autoload dir `nu`
/// reports); the rest are the legacy config-dir locations kept so
/// install/uninstall can clean up stranded files. Empty when `nu` can't be
/// queried: without `nu` there is no Nushell to integrate with.
pub fn nushell_autoload_candidates(home: &std::path::Path) -> Vec<PathBuf> {
    let dirs = nu_dirs();
    let Some(target) = dirs.vendor_autoload else {
        return Vec::new();
    };
    let mut candidates = vec![target];
    candidates.extend(legacy_nushell_autoload_dirs(
        home,
        dirs.default_config.as_deref(),
    ));

    // Deduplicate while preserving priority order (write target first).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));
    candidates
}

/// Parse the profile path `pwsh` prints: one absolute path, else `None`.
#[cfg(unix)]
fn parse_powershell_profile(stdout: &str) -> Option<PathBuf> {
    let line = stdout.trim();
    let path = PathBuf::from(line);
    (!line.contains('\n') && path.is_absolute()).then_some(path)
}

/// Classify a profile query's outcome.
///
/// `Ok(None)` — use the conventional path — when the runtime is missing,
/// fails, times out, or prints anything but one absolute path. `Err(signal)`
/// when the query was cancelled: the child died of SIGINT/SIGTERM, or the
/// command operation was cancelled while wt waited.
#[cfg(unix)]
fn powershell_profile_from(
    output: &std::io::Result<std::process::Output>,
    operation_signal: Option<i32>,
) -> Result<Option<PathBuf>, i32> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use std::os::unix::process::ExitStatusExt;
    let output = output.as_ref().ok();
    if let Some(signal) = output
        .and_then(|output| output.status.signal())
        .filter(|signal| matches!(*signal, SIGINT | SIGTERM))
        .or(operation_signal)
    {
        return Err(signal);
    }
    Ok(output
        .filter(|output| output.status.success())
        .and_then(|output| parse_powershell_profile(&String::from_utf8_lossy(&output.stdout))))
}

/// The profile query's answer, or the signal that cancelled it.
#[cfg(unix)]
static POWERSHELL_PROFILE: OnceLock<Result<Option<PathBuf>, i32>> = OnceLock::new();

/// The current-user, current-host profile (`$PROFILE.CurrentUserCurrentHost`)
/// the installed `pwsh` loads, queried at most once per process so install,
/// uninstall and detection all act on the same answer.
///
/// `-NoProfile` keeps the query from running the profiles it names, and the
/// telemetry and update-check opt-outs keep it off the network. Bounded by the
/// shared local-probe timeout. A timed child runs in its own process group, so
/// a terminal Ctrl-C reaches only wt, whose default disposition ends it;
/// SIGTERM to wt is latched by the command operation and read back here.
#[cfg(unix)]
fn queried_powershell_profile() -> std::io::Result<Option<PathBuf>> {
    POWERSHELL_PROFILE
        .get_or_init(|| {
            let output = crate::shell_exec::Cmd::new("pwsh")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "$PROFILE.CurrentUserCurrentHost",
                ])
                .env("POWERSHELL_TELEMETRY_OPTOUT", "1")
                .env("POWERSHELL_UPDATECHECK", "Off")
                .timeout(crate::git::reap::probe_timeout())
                .run();
            let result = powershell_profile_from(
                &output,
                crate::signal_forwarder::operation_interrupt_signal(),
            );
            if let Err(signal) = result {
                crate::signal_forwarder::cancel_foreground(signal);
            }
            result
        })
        .clone()
        .map_err(|signal| {
            std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                ProfileQueryInterrupted(signal),
            )
        })
}

/// The payload of the `io::Error` the profile path helpers return when the
/// PowerShell profile query was cancelled, naming the signal.
#[cfg(unix)]
#[derive(Debug)]
struct ProfileQueryInterrupted(i32);

#[cfg(unix)]
impl std::fmt::Display for ProfileQueryInterrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PowerShell profile query interrupted by signal {}",
            self.0
        )
    }
}

#[cfg(unix)]
impl std::error::Error for ProfileQueryInterrupted {}

/// The signal that cancelled the PowerShell profile query, when `error` is
/// that cancellation rather than an ordinary path failure.
pub fn profile_query_interruption(error: &std::io::Error) -> Option<i32> {
    #[cfg(unix)]
    {
        error
            .get_ref()?
            .downcast_ref::<ProfileQueryInterrupted>()
            .map(|interrupted| interrupted.0)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        None
    }
}

/// Get PowerShell profile paths in order of preference.
///
/// On Windows, returns both PowerShell Core (7+) and Windows PowerShell (5.1) paths.
/// On Unix, returns the current-user, current-host profile the installed `pwsh`
/// reports, and nothing when `pwsh` can't be queried: without `pwsh` there is
/// no PowerShell to integrate with. Errors only when the query was cancelled.
pub fn powershell_profile_paths(home: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        // Use platform-specific Documents path (handles non-English Windows)
        let docs = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
        Ok(vec![
            // PowerShell Core 6+ (pwsh.exe) - preferred
            docs.join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
            // Windows PowerShell 5.1 (powershell.exe) - legacy but still common
            docs.join("WindowsPowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ])
    }
    #[cfg(not(windows))]
    {
        let _ = home;
        #[cfg(unix)]
        let queried = queried_powershell_profile()?;
        #[cfg(not(unix))]
        let queried = None;
        Ok(queried.into_iter().collect())
    }
}

/// The directory fish reads its configuration from — fish's own
/// `$__fish_config_dir`: `$XDG_CONFIG_HOME/fish` when that variable holds an
/// absolute path, otherwise `~/.config/fish`. `etcetera`'s [`Xdg`] strategy is
/// that rule, so nothing about it is restated here.
///
/// [`Xdg`] rather than `choose_base_strategy`, which resolves `%APPDATA%` on
/// Windows: fish has no `%APPDATA%` notion on any platform, so one directory
/// holds the wrapper and the completion there too. It isn't one a Windows fish
/// reads — `Xdg::new` takes its home from `std::env::home_dir`, which is
/// `%USERPROFILE%` there and, since Rust 1.85, deliberately not the POSIX
/// `$HOME` a Cygwin or MSYS fish runs under. The two strategies are the same
/// everywhere else.
///
/// Every path fish itself reads goes through this one function — the wrapper
/// (`config_paths`) and the completion (`fish_completion_path`) — because they have
/// to agree. Resolving the wrapper's directory separately from the completion's
/// put the wrapper under `~/.config/fish` whenever `$XDG_CONFIG_HOME` pointed
/// anywhere else, which fish never reads, while the completion landed in the
/// directory it does read; install reported success for both and `wt` was never
/// defined. `legacy_fish_conf_d_path` is deliberately not one of these — see
/// its own note.
pub fn fish_config_dir(home: &std::path::Path) -> PathBuf {
    Xdg::new()
        .map(|xdg| xdg.config_dir())
        .unwrap_or_else(|_| home.join(".config"))
        .join("fish")
}

/// The directory worktrunk reads and writes zsh's rc file in: `$ZDOTDIR` when
/// it holds an absolute path, otherwise `$HOME`.
///
/// zsh's rule is `$ZDOTDIR`, or `$HOME` when it is unset. The absolute-only
/// guard on top is the same one etcetera's [`Xdg`] applies to
/// `$XDG_CONFIG_HOME` ([`fish_config_dir`]).
///
/// A non-absolute value resolves against the current directory, and that
/// directory is `wt`'s: zsh resolves one against *zsh's* own startup
/// directory, which `wt` has no way to know at install time. Honouring it
/// would send install, and the whole-file rewrite `wt config shell uninstall`
/// performs, to a file neither side meant — a `.zshrc` in a dotfiles checkout
/// that happened to be the invocation directory. `$HOME` is also where
/// `wt config show` reads back from, so install and detection agree.
pub(super) fn zsh_config_dir(home: &std::path::Path) -> PathBuf {
    std::env::var_os("ZDOTDIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.to_path_buf())
}

/// Rc/profile files scanned line-by-line for integration lines.
///
/// Bash/Zsh/PowerShell integration is one line in an rc file, so these paths
/// are independent of the binary name; Fish and Nushell use per-name wrapper
/// files instead and have no line-based config. `config_paths` builds on this
/// for the line-based shells, so install and uninstall cannot drift apart.
pub fn line_based_config_paths(
    shell: super::Shell,
    home: &std::path::Path,
) -> std::io::Result<Vec<PathBuf>> {
    Ok(match shell {
        super::Shell::Bash => {
            // Use .bashrc - sourced by interactive shells (login shells should source .bashrc)
            vec![home.join(".bashrc")]
        }
        super::Shell::Zsh => vec![zsh_config_dir(home).join(".zshrc")],
        super::Shell::PowerShell => powershell_profile_paths(home)?,
        super::Shell::Fish | super::Shell::Nushell => Vec::new(),
    })
}

/// Returns the config file paths for a shell.
///
/// The `cmd` parameter affects the Fish functions filename (e.g., `wt.fish` or `git-wt.fish`).
/// Returns paths in order of preference. The first existing file should be used.
pub(super) fn config_paths(shell: super::Shell, cmd: &str) -> Result<Vec<PathBuf>, std::io::Error> {
    let home = home_dir_required()?;

    Ok(match shell {
        super::Shell::Bash | super::Shell::Zsh | super::Shell::PowerShell => {
            line_based_config_paths(shell, &home)?
        }
        super::Shell::Fish => {
            // For fish, we write to functions/ which is autoloaded on first use.
            // This ensures PATH is fully configured before our function loads,
            // fixing the issue where Homebrew PATH setup in config.fish runs
            // after conf.d/ files. See: https://github.com/max-sixty/worktrunk/issues/566
            vec![
                fish_config_dir(&home)
                    .join("functions")
                    .join(format!("{}.fish", cmd)),
            ]
        }
        super::Shell::Nushell => {
            // Nushell autoloads `*.nu` from `$nu.vendor-autoload-dirs`; the last
            // entry (under `$nu.data-dir`) is the write target. The rest are the
            // legacy `<config-dir>/vendor/autoload` locations older worktrunk
            // wrote to (never autoloaded — issue #2878), kept so
            // install/uninstall can clean them up.
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
///
/// Hardcoded to `~/.config`, unlike [`fish_config_dir`], because this names a
/// file worktrunk *wrote*, not a file fish reads: the write was hardcoded for
/// as long as it existed, and install removes what this resolves to whole and
/// unread. Resolving it through `$XDG_CONFIG_HOME` would both stop finding the
/// stranded file and start deleting a `conf.d/{cmd}.fish` in the directory fish
/// actually loads — one worktrunk never wrote and whose contents are never
/// read. Uninstall's directory scan is gated on worktrunk-managed content and
/// carries no such constraint.
pub(super) fn legacy_fish_conf_d_path(cmd: &str) -> Result<PathBuf, std::io::Error> {
    let home = home_dir_required()?;
    Ok(home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join(format!("{}.fish", cmd)))
}

/// The completion file worktrunk installs for fish, the one shell whose
/// completions live in their own file (`<fish config>/completions/<cmd>.fish`).
/// Bash, Zsh, Nushell and PowerShell register completions inline in the
/// integration they install.
pub fn fish_completion_path(cmd: &str) -> Result<PathBuf, std::io::Error> {
    Ok(fish_config_dir(&home_dir_required()?)
        .join("completions")
        .join(format!("{cmd}.fish")))
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

    #[cfg(unix)]
    #[test]
    fn test_parse_powershell_profile() {
        assert_eq!(
            parse_powershell_profile("/x/powershell/Microsoft.PowerShell_profile.ps1\n"),
            Some(PathBuf::from(
                "/x/powershell/Microsoft.PowerShell_profile.ps1"
            ))
        );
        assert_eq!(parse_powershell_profile(""), None);
        assert_eq!(parse_powershell_profile("relative/profile.ps1"), None);
        assert_eq!(parse_powershell_profile("/a.ps1\n/b.ps1\n"), None);
    }

    /// Stand-in runtimes via `/bin/sh -c`: the reported path is trusted, a
    /// runtime that can't answer falls back, and SIGINT/SIGTERM — on the child
    /// or latched by the operation — cancels instead of falling back.
    #[cfg(unix)]
    #[test]
    fn test_powershell_profile_from() {
        let sh = |script: &str| {
            crate::shell_exec::Cmd::new("/bin/sh")
                .args(["-c", script])
                .run()
        };
        assert_eq!(
            powershell_profile_from(&sh("echo /custom/powershell/profile.ps1"), None),
            Ok(Some(PathBuf::from("/custom/powershell/profile.ps1")))
        );
        assert_eq!(
            powershell_profile_from(&sh("echo /custom/profile.ps1; exit 1"), None),
            Ok(None)
        );
        let missing = crate::shell_exec::Cmd::new("/nonexistent/pwsh-xxxxxxxx").run();
        assert_eq!(powershell_profile_from(&missing, None), Ok(None));
        assert_eq!(powershell_profile_from(&sh("kill -INT $$"), None), Err(2));
        assert_eq!(powershell_profile_from(&sh("kill -TERM $$"), None), Err(15));
        // A crash is a failed query, not a cancellation.
        assert_eq!(
            powershell_profile_from(&sh("kill -KILL $$"), None),
            Ok(None)
        );
        assert_eq!(
            powershell_profile_from(&sh("echo /custom/profile.ps1"), Some(15)),
            Err(15)
        );
    }

    /// Only the cancelled query's own error reads as an interruption; an
    /// ordinary `Interrupted` io error stays an ordinary failure.
    #[cfg(unix)]
    #[test]
    fn test_profile_query_interruption() {
        for signal in [2, 15] {
            let cancelled = std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                ProfileQueryInterrupted(signal),
            );
            assert_eq!(profile_query_interruption(&cancelled), Some(signal));
            assert_eq!(
                cancelled.to_string(),
                format!("PowerShell profile query interrupted by signal {signal}")
            );
        }
        let ordinary = std::io::Error::from(std::io::ErrorKind::Interrupted);
        assert_eq!(profile_query_interruption(&ordinary), None);
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

        // The write target (first entry) is the directory `nu` reports; without
        // `nu` there is nothing to integrate with, so no candidates at all.
        assert_eq!(candidates.first(), nu_dirs().vendor_autoload.as_ref());
        // No duplicates.
        let unique: std::collections::HashSet<_> = candidates.iter().collect();
        assert_eq!(
            candidates.len(),
            unique.len(),
            "candidates must not contain duplicates: {candidates:?}"
        );
        // Whenever there is a target, the legacy ~/.config location is there
        // too, for cleanup.
        assert_eq!(
            candidates
                .iter()
                .any(|p| p == &home.join(".config/nushell/vendor/autoload")),
            !candidates.is_empty(),
            "legacy ~/.config/nushell location must be a candidate: {candidates:?}"
        );
    }
}
