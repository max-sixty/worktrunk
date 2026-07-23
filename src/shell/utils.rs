//! Shell detection and utility functions.
//!
//! This module provides utilities for detecting the current shell, extracting
//! shell names from paths, and probing shell configuration state.

use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use super::Shell;

/// Extract executable name from a path, stripping `.exe` on Windows.
///
/// Uses `std::path::Path` for platform-native path handling:
/// - Unix: `/usr/bin/bash` -> "bash"
/// - Windows: `C:\Program Files\Git\usr\bin\bash.exe` -> "bash"
///
/// Only strips `.exe` extension (not other extensions like `.9` in `zsh-5.9`).
pub fn extract_filename_from_path(path: &str) -> Option<&str> {
    let filename = std::path::Path::new(path).file_name()?.to_str()?;

    // Strip .exe extension (case-insensitive for Windows)
    // Don't use file_stem() because it would strip version numbers like ".9" from "zsh-5.9"
    // Handle all case variants: .exe, .EXE, .Exe, .eXe, etc.
    if filename.len() > 4 && filename[filename.len() - 4..].eq_ignore_ascii_case(".exe") {
        Some(&filename[..filename.len() - 4])
    } else {
        Some(filename)
    }
}

/// Determine Shell variant from a shell name (without path or extension).
///
/// Handles versioned binaries like `zsh-5.9` or `bash5` by accepting a known
/// shell name followed by a non-alphabetic character. The boundary check
/// keeps unrelated commands that merely start with a shell name (`fishd`,
/// `bashtop`, `numactl`) from matching — important now that names also come
/// from the process-tree walk, not just `$SHELL`.
pub fn shell_from_name(shell_name: &str) -> Option<Shell> {
    // Try exact match first
    if let Ok(shell) = shell_name.parse() {
        return Some(shell);
    }

    let name_lower = shell_name.to_lowercase();
    // "nushell" precedes "nu" so its remainder isn't rejected as alphabetic.
    for (prefix, shell) in [
        ("powershell", Shell::PowerShell),
        ("pwsh", Shell::PowerShell),
        ("nushell", Shell::Nushell),
        ("nu", Shell::Nushell),
        ("bash", Shell::Bash),
        ("zsh", Shell::Zsh),
        ("fish", Shell::Fish),
    ] {
        if name_matches_shell(&name_lower, prefix) {
            return Some(shell);
        }
    }
    None
}

/// Display name of the current shell.
///
/// Prefers the process-tree walk (the shell wt is actually running under,
/// possibly an unsupported one like "tcsh"); falls back to the `$SHELL`
/// basename. Returns `None` when neither source names a shell.
pub fn current_shell_name() -> Option<String> {
    if let Some(ancestor) = ancestor_shell() {
        return Some(ancestor.name.clone());
    }
    shell_name_from_env()
}

/// Read `$SHELL` and extract the executable name (e.g. `/usr/bin/zsh` -> "zsh").
fn shell_name_from_env() -> Option<String> {
    let shell_path = std::env::var("SHELL").ok()?;
    extract_filename_from_path(&shell_path).map(String::from)
}

/// Detect the shell wt is running under.
///
/// Uses three strategies, most reliable first:
/// 1. Process-tree walk: the nearest enclosing shell process. `$SHELL` names
///    the *login* shell, which is wrong whenever the interactive shell
///    differs (bash launched from zsh, a terminal profile running fish), so
///    the process tree is consulted first. A known-but-unsupported enclosing
///    shell (e.g. tcsh) returns `None` here — falling back to `$SHELL` would
///    reintroduce the wrong-shell answer; `current_shell_name()` still names
///    it for messages.
/// 2. `$SHELL` environment variable (Unix standard, also set by Git Bash on
///    Windows), when the walk finds no shell (unsupported platform, wt
///    spawned outside any shell).
/// 3. `PSModulePath` environment variable (indicates PowerShell on all
///    platforms). On Windows this has some false positives (PSModulePath can
///    be set system-wide), but for diagnostic purposes that's acceptable — a
///    slightly less accurate message is better than "shell integration not
///    installed" when it IS installed.
pub fn current_shell() -> Option<Shell> {
    if let Some(ancestor) = ancestor_shell() {
        return ancestor.shell;
    }

    if let Some(name) = shell_name_from_env() {
        return shell_from_name(&name);
    }

    if std::env::var_os("PSModulePath").is_some() {
        return Some(Shell::PowerShell);
    }

    None
}

/// Nearest enclosing shell process, found by walking up the process tree.
#[derive(Debug, Clone)]
pub struct AncestorShell {
    /// Process name as observed, login-shell `-` prefix stripped (e.g.
    /// "zsh", "tcsh").
    pub name: String,
    /// The parsed shell; `None` for a known-but-unsupported shell.
    pub shell: Option<Shell>,
}

/// Plain POSIX script interpreters: virtually never the interactive shell on
/// modern systems, so the walk treats them as plumbing (Makefiles, `sh -c`
/// wrappers) and keeps walking toward the real shell.
const TRANSPARENT_INTERPRETERS: &[&str] = &["sh", "dash", "ash", "busybox"];

/// Known interactive shells wt has no integration for. One of these stops the
/// walk — it IS the enclosing shell — but reports `shell: None` rather than
/// letting a supported shell further up the tree (or `$SHELL`) claim the
/// session.
const UNSUPPORTED_SHELLS: &[&str] = &[
    "tcsh", "csh", "ksh", "mksh", "oksh", "loksh", "yash", "elvish", "xonsh", "oil", "osh",
];

/// True when `name` is `prefix` optionally followed by a version-ish suffix:
/// "zsh" matches "zsh", "zsh-5.9", "zsh5" — but not "zshx" ("fishd",
/// "bashtop", and "numactl" must not read as shells).
///
/// The dash boundary is deliberately permissive so versioned binaries keep
/// matching (`zsh-5.9`, `pwsh-preview`); the cost is that a dashed tool name
/// starting with a shell name (`bash-language-server`, or its 15-char
/// `/proc` comm truncation `bash-language-s`) also classifies as that shell.
/// Acceptable: such tools don't sit in wt's interactive ancestor chain.
fn name_matches_shell(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// Nearest enclosing shell process, cached for the invocation.
///
/// Walks up from wt's parent, passing through non-shell ancestors (`git` for
/// `git wt`, `sudo`, script runners) until it finds a known shell. Implemented
/// for Linux (`/proc/<pid>/stat`) and macOS (a `ps` snapshot); elsewhere
/// (Windows, BSDs) returns `None` and callers fall back to `$SHELL` /
/// `PSModulePath` — Git Bash sets `$SHELL` itself, so Windows loses little.
///
/// `WORKTRUNK_TEST_PARENT_SHELL` overrides the walk for tests: empty means "no
/// shell ancestor found" (integration tests run wt under a test harness whose
/// real ancestry would nondeterministically include the developer's or CI
/// runner's shell); a process name simulates finding that ancestor.
pub fn ancestor_shell() -> Option<&'static AncestorShell> {
    static ANCESTOR: std::sync::OnceLock<Option<AncestorShell>> = std::sync::OnceLock::new();
    ANCESTOR.get_or_init(detect_ancestor_shell).as_ref()
}

fn detect_ancestor_shell() -> Option<AncestorShell> {
    if let Some(value) = std::env::var_os("WORKTRUNK_TEST_PARENT_SHELL") {
        let name = value.to_str()?.trim();
        if name.is_empty() {
            return None;
        }
        return ancestor_from_name(name);
    }

    #[cfg(unix)]
    {
        walk_ancestors(std::os::unix::process::parent_id(), process_name_and_ppid)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Walk up the process tree from `pid`, looking each ancestor up in
/// `lookup`, until a shell stops the walk. Bounded: deep enough for wrappers
/// (git, sudo, script runners), small enough that a cycle or exotic tree
/// can't stall the warning path. An unreadable hop ends the walk — without a
/// parent pid there is nothing to continue from — and callers fall back to
/// `$SHELL`.
#[cfg(unix)]
fn walk_ancestors(
    mut pid: u32,
    lookup: impl Fn(u32) -> Option<(String, u32)>,
) -> Option<AncestorShell> {
    for _ in 0..16 {
        if pid <= 1 {
            return None;
        }
        let (name, ppid) = lookup(pid)?;
        tracing::debug!(pid, ppid, name, "shell ancestry hop");
        if let Some(found) = ancestor_from_name(&name) {
            return Some(found);
        }
        if ppid == pid {
            return None;
        }
        pid = ppid;
    }
    None
}

/// Classify a process name, returning the walk's result if it's a stop:
/// a supported or known-unsupported shell stops the walk; transparent
/// interpreters and non-shells return `None` (keep walking).
fn ancestor_from_name(name: &str) -> Option<AncestorShell> {
    // Login shells report argv[0] with a leading dash ("-zsh").
    let name = name.strip_prefix('-').unwrap_or(name);
    let lower = name.to_ascii_lowercase();
    if TRANSPARENT_INTERPRETERS.contains(&lower.as_str()) {
        return None;
    }
    let shell = if UNSUPPORTED_SHELLS
        .iter()
        .any(|s| name_matches_shell(&lower, s))
    {
        None
    } else {
        Some(shell_from_name(&lower)?)
    };
    Some(AncestorShell {
        name: name.to_string(),
        shell,
    })
}

/// Read (process name, parent pid) for `pid` from the OS process table.
///
/// `/proc/<pid>/stat` is `pid (comm) state ppid …`; comm may itself contain
/// spaces or parens, so parse from the last `)`.
#[cfg(target_os = "linux")]
fn process_name_and_ppid(pid: u32) -> Option<(String, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    // After ")": state, then ppid.
    let ppid = stat
        .get(close + 1..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((name, ppid))
}

/// Read (process name, parent pid) for `pid` from a one-shot `ps` snapshot.
///
/// macOS has no `/proc`, and the kernel's per-process `p_comm` carries the
/// executable image's name rather than the invoked name — `/bin/sh` re-execs
/// bash (via `/var/select/sh`), so an sh script's `p_comm` reads "bash" and
/// the sh-transparency rule would never fire. `ps`'s `comm` column preserves
/// argv\[0\] ("sh", "-zsh", "/bin/zsh"), which is the invoked identity the
/// classifier needs. One snapshot (a few ms, cold warning paths only) serves
/// the whole walk.
#[cfg(target_os = "macos")]
fn process_name_and_ppid(pid: u32) -> Option<(String, u32)> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<u32, (String, u32)>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(ps_snapshot).get(&pid).cloned()
}

/// Build the pid → (name, parent pid) table from one `ps` invocation.
#[cfg(target_os = "macos")]
fn ps_snapshot() -> std::collections::HashMap<u32, (String, u32)> {
    let Ok(output) = crate::shell_exec::Cmd::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .run()
    else {
        return std::collections::HashMap::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            // comm is the remainder — argv[0] may be a path and may
            // contain spaces; keep the basename.
            let comm = fields.collect::<Vec<_>>().join(" ");
            let name = extract_filename_from_path(&comm)?.to_string();
            Some((pid, (name, ppid)))
        })
        .collect()
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_name_and_ppid(_pid: u32) -> Option<(String, u32)> {
    None
}

/// Leading flags for the interactive zsh compinit probe.
///
/// `+m` (disable job control) is load-bearing — see the call site and issue
/// #3322. It must precede `-ic` so zsh parses it as an option, not a command
/// argument.
const ZSH_PROBE_FLAGS: [&str; 2] = ["+m", "-ic"];

/// Detect if user's zsh has compinit enabled by probing for the compdef function.
///
/// Zsh's completion system (compinit) must be explicitly enabled - it's not on by default.
/// When compinit runs, it defines the `compdef` function. We probe for this function
/// by spawning an interactive zsh that sources the user's config, then checking if
/// compdef exists.
///
/// This approach matches what other CLI tools (hugo, podman, dvc) recommend: detect
/// the state and advise users, rather than trying to auto-enable compinit.
///
/// Returns:
/// - `Some(true)` if compinit is enabled (compdef function exists)
/// - `Some(false)` if compinit is NOT enabled
/// - `None` if detection failed (zsh not installed, timeout, error)
///
// TODO(zsh-compinit-probe-unify): see `config::show::check_zsh_compinit_missing`
// for the matching probe and why the two haven't been merged (intentional
// `--no-globalrcs` divergence, different result types and runners). #3322 had to
// apply the `+m` job-control fix in both places.
pub fn detect_zsh_compinit() -> Option<bool> {
    // Allow tests to bypass this check since zsh subprocess behavior varies across CI envs
    if std::env::var("WORKTRUNK_TEST_COMPINIT_CONFIGURED").is_ok() {
        return Some(true); // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WORKTRUNK_TEST_COMPINIT_MISSING").is_ok() {
        return Some(false); // Force warning to appear
    }

    // Probe command: check if compdef function exists (proof compinit ran).
    // We use unique markers (__WT_COMPINIT_*) to avoid false matches from any
    // output the user's zshrc might produce during startup.
    let probe_cmd =
        r#"(( $+functions[compdef] )) && echo __WT_COMPINIT_YES__ || echo __WT_COMPINIT_NO__"#;

    tracing::debug!(command = %probe_cmd, "$ zsh -ic '{}' (probe)", probe_cmd);

    let mut cmd = Command::new("zsh");
    // `+m` disables job control so the interactive probe doesn't grab wt's
    // controlling terminal. An interactive zsh with job control on `tcsetpgrp`s
    // to claim the terminal foreground; if the 2s timeout kills it before it
    // restores that, wt is left in a background process group and the next
    // terminal write raises SIGTTOU. See issue #3322. `+m` must precede `-ic`
    // so it's parsed as an option rather than a command argument.
    cmd.args(ZSH_PROBE_FLAGS)
        .arg(probe_cmd)
        .stdin(Stdio::null()) // Prevent compinit from prompting interactively
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // Suppress user's zsh startup messages
        // Suppress zsh's "insecure directories" warning from compinit.
        //
        // When fpath contains directories with insecure permissions, compinit prompts:
        //   "zsh compinit: insecure directories, run compaudit for list."
        //   "Ignore insecure directories and continue [y] or abort compinit [n]?"
        //
        // This prompt goes to /dev/tty (not stderr), bypassing our stderr redirect.
        //
        // Worktrunk does NOT cause this warning - our shell init script doesn't modify
        // fpath or call compinit. It only registers completions with `compdef` if the
        // user has already set up compinit themselves. The warning appears because:
        // 1. This probe runs `zsh -ic` which sources global configs like /etc/zsh/zshrc
        // 2. Some environments (notably Ubuntu CI) have global configs that call compinit
        // 3. Those environments may have insecure fpath directories
        //
        // Safe to suppress because we're only probing shell state, not doing anything
        // security-sensitive, and this only affects our subprocess.
        .env("ZSH_DISABLE_COMPFIX", "true");
    crate::shell_exec::scrub_directive_env_vars(&mut cmd);
    let mut child = cmd.spawn().ok()?;

    let timeout = Duration::from_secs(2);

    match child.wait_timeout(timeout) {
        Ok(Some(_status)) => {
            // Child exited: pipe write ends are closed, safe to read sequentially.
            use std::io::Read;
            let mut buf = Vec::new();
            child.stdout.as_mut()?.read_to_end(&mut buf).ok()?;
            let stdout = String::from_utf8_lossy(&buf);
            Some(stdout.contains("__WT_COMPINIT_YES__"))
        }
        Ok(None) => {
            // Timed out - kill and clean up
            let _ = child.kill();
            let _ = child.wait();
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Regression guard for #3322: the interactive zsh compinit probe must
    /// disable job control (`+m`) so it can't grab wt's controlling terminal —
    /// otherwise a timeout-kill leaves the parent in a background process group
    /// and the next terminal write raises SIGTTOU.
    #[test]
    fn test_zsh_probe_disables_job_control() {
        assert_eq!(
            ZSH_PROBE_FLAGS[0], "+m",
            "job control must be disabled before -ic (#3322)"
        );
        assert!(ZSH_PROBE_FLAGS.contains(&"-ic"));
    }

    // ==========================================================================
    // Path extraction tests (Issue #348)
    // ==========================================================================

    #[rstest]
    #[case::just_name("bash", Some("bash"))]
    #[case::just_name_exe("bash.exe", Some("bash"))]
    #[case::mixed_case_exe_title("bash.Exe", Some("bash"))]
    #[case::mixed_case_exe_upper("bash.EXE", Some("bash"))]
    #[case::mixed_case_exe_camel("bash.eXe", Some("bash"))]
    #[case::empty("", None)]
    fn test_extract_filename_from_path_common(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(unix)]
    #[rstest]
    #[case::unix_bash("/usr/bin/bash", Some("bash"))]
    #[case::unix_zsh("/bin/zsh", Some("zsh"))]
    #[case::unix_fish("/usr/local/bin/fish", Some("fish"))]
    #[case::nix_versioned("/nix/store/abc123/zsh-5.9", Some("zsh-5.9"))]
    fn test_extract_filename_from_path_unix(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(windows)]
    #[rstest]
    #[case::windows_git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Some("bash"))]
    #[case::windows_powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Some("powershell")
    )]
    #[case::windows_pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Some("pwsh"))]
    #[case::windows_zsh(r"C:\msys64\usr\bin\zsh.exe", Some("zsh"))]
    #[case::uppercase_exe(r"C:\WINDOWS\SYSTEM32\BASH.EXE", Some("BASH"))]
    fn test_extract_filename_from_path_windows(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    /// Issue #348: Windows Git Bash shell detection
    ///
    /// Git Bash sets $SHELL to Windows-style paths like:
    /// `C:\Program Files\Git\usr\bin\bash.exe`
    ///
    /// This test verifies the full path-to-shell detection flow works on Windows.
    #[cfg(windows)]
    #[rstest]
    #[case::git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Shell::Bash)]
    #[case::msys2_zsh(r"C:\msys64\usr\bin\zsh.exe", Shell::Zsh)]
    #[case::powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Shell::PowerShell
    )]
    #[case::pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Shell::PowerShell)]
    fn test_issue_348_windows_shell_detection(#[case] shell_path: &str, #[case] expected: Shell) {
        // This is the exact flow that failed before the fix:
        // 1. extract_filename_from_path() extracts "bash" from Windows path
        // 2. shell_from_name() maps "bash" to Shell::Bash
        let shell_name = extract_filename_from_path(shell_path)
            .expect("should extract filename from Windows path");
        let detected =
            shell_from_name(shell_name).expect("should detect shell from extracted name");
        assert_eq!(detected, expected);
    }

    #[rstest]
    #[case::bash("bash", Some(Shell::Bash))]
    #[case::bash_versioned("bash5", Some(Shell::Bash))]
    #[case::zsh("zsh", Some(Shell::Zsh))]
    #[case::zsh_versioned("zsh-5.9", Some(Shell::Zsh))]
    #[case::fish("fish", Some(Shell::Fish))]
    #[case::nu("nu", Some(Shell::Nushell))]
    #[case::nushell("nushell", Some(Shell::Nushell))]
    #[case::powershell("powershell", Some(Shell::PowerShell))]
    #[case::pwsh("pwsh", Some(Shell::PowerShell))]
    #[case::pwsh_preview("pwsh-preview", Some(Shell::PowerShell))]
    #[case::unknown("tcsh", None)]
    #[case::unknown_csh("csh", None)]
    // Names that merely start with a shell name must not match — the
    // process-tree walk feeds arbitrary ancestor names through here.
    #[case::fish_daemon("fishd", None)]
    #[case::bashtop("bashtop", None)]
    #[case::numactl("numactl", None)]
    fn test_shell_from_name(#[case] name: &str, #[case] expected: Option<Shell>) {
        assert_eq!(shell_from_name(name), expected);
    }

    /// The walk stops at the nearest shell: supported shells parse, known
    /// unsupported shells stop the walk with `shell: None`, and plumbing
    /// (script interpreters, non-shells) is transparent.
    #[rstest]
    #[case::zsh("zsh", Some(Some(Shell::Zsh)))]
    #[case::login_zsh("-zsh", Some(Some(Shell::Zsh)))]
    #[case::login_bash("-bash", Some(Some(Shell::Bash)))]
    #[case::nix_versioned("zsh-5.9", Some(Some(Shell::Zsh)))]
    #[case::tcsh("tcsh", Some(None))]
    #[case::ksh("ksh", Some(None))]
    #[case::ksh93("ksh93", Some(None))]
    #[case::sh_transparent("sh", None)]
    #[case::dash_transparent("dash", None)]
    #[case::git_transparent("git", None)]
    #[case::terminal_transparent("iTerm2", None)]
    fn test_ancestor_from_name(#[case] name: &str, #[case] expected: Option<Option<Shell>>) {
        let result = ancestor_from_name(name);
        assert_eq!(result.as_ref().map(|a| a.shell), expected, "name: {name}");
        if let Some(ancestor) = result {
            assert!(
                !ancestor.name.starts_with('-'),
                "login-shell dash must be stripped: {}",
                ancestor.name
            );
        }
    }

    /// The ancestry walk stops at the nearest shell, passes through wrappers
    /// and transparent interpreters, and terminates on init, cycles,
    /// unreadable hops, and depth exhaustion.
    #[cfg(unix)]
    #[test]
    fn test_walk_ancestors() {
        use std::collections::HashMap;

        // wt's parent chain: sh (transparent) ← git (wrapper) ← -zsh (login)
        let table: HashMap<u32, (String, u32)> = HashMap::from([
            (10, ("sh".to_string(), 9)),
            (9, ("git".to_string(), 8)),
            (8, ("-zsh".to_string(), 1)),
        ]);
        let found = walk_ancestors(10, |pid| table.get(&pid).cloned())
            .expect("finds zsh through sh and git");
        assert_eq!(found.shell, Some(Shell::Zsh));
        assert_eq!(found.name, "zsh");

        // An unsupported shell stops the walk even with a supported shell
        // above it — the nearest enclosing shell owns the session.
        let table: HashMap<u32, (String, u32)> =
            HashMap::from([(10, ("tcsh".to_string(), 8)), (8, ("zsh".to_string(), 1))]);
        let found = walk_ancestors(10, |pid| table.get(&pid).cloned()).unwrap();
        assert_eq!(found.shell, None);
        assert_eq!(found.name, "tcsh");

        // Terminations, each with no result: starting at init, a ppid
        // cycle, an unreadable hop, and a >16-deep non-shell chain.
        assert!(walk_ancestors(1, |_| unreachable!("init is never looked up")).is_none());
        assert!(walk_ancestors(10, |pid| Some(("looper".to_string(), pid))).is_none());
        assert!(walk_ancestors(10, |_| None).is_none());
        assert!(walk_ancestors(u32::MAX, |pid| Some(("wrapper".to_string(), pid - 1))).is_none());
    }

    /// The OS probe reads real entries: our own pid resolves to a non-empty
    /// name and our actual parent pid.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_process_name_and_ppid_self() {
        let (name, ppid) = process_name_and_ppid(std::process::id())
            .expect("own pid must be readable from the process table");
        assert!(!name.is_empty());
        assert_eq!(ppid, std::os::unix::process::parent_id());
    }

    /// The sh-transparency rule depends on the OS probe reporting the
    /// *invoked* name for `sh` processes. On macOS the kernel's `p_comm`
    /// reports the image name instead — `/bin/sh` re-execs bash, so `p_comm`
    /// reads "bash" — which is why the probe there uses `ps`'s
    /// argv\[0\]-derived comm. Pin that with a real child: a live `sh` must
    /// probe as "sh", never as its implementation ("bash"/"dash").
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_probe_reports_invoked_name_for_sh() {
        // Compound command so sh doesn't exec-replace itself with `sleep`.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 30; true"])
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sh");

        // The probe can briefly race the child's exec; poll until it settles.
        // (macOS: ps_snapshot() directly — process_name_and_ppid's cached
        // table may predate the child.)
        let probe = |pid: u32| -> Option<(String, u32)> {
            #[cfg(target_os = "macos")]
            return ps_snapshot().get(&pid).cloned();
            #[cfg(target_os = "linux")]
            return process_name_and_ppid(pid);
        };
        let mut last = None;
        for _ in 0..40 {
            last = probe(child.id());
            if last.as_ref().is_some_and(|(name, _)| name == "sh") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();

        let (name, ppid) = last.expect("child sh must be visible to the probe");
        assert_eq!(name, "sh", "probe must report the invoked name");
        assert_eq!(ppid, std::process::id());
    }
}
