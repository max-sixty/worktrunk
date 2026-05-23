//! Verify Nushell's actual `$nu.default-config-dir` behavior on Unix.
//!
//! Backs the docstring claim in `src/shell/paths.rs::nushell_config_dir` (PR
//! #2881): `$nu.default-config-dir` honors `XDG_CONFIG_HOME` when it is set
//! (non-empty, absolute), and on macOS falls back to
//! `~/Library/Application Support/nushell` otherwise. Source of truth:
//! `nu_path::nu_config_dir` →
//! <https://github.com/nushell/nushell/blob/0.112.2/crates/nu-path/src/helpers.rs#L22-L44>.
//!
//! Gated on `shell-integration-tests` so the test only compiles when the CI
//! `test-setup` action has installed `nu` (it is installed unconditionally on
//! non-Windows runners, alongside the other shells these tests need).

#![cfg(all(unix, feature = "shell-integration-tests"))]

use std::process::Command;

fn run_nu_default_config_dir(configure_env: impl FnOnce(&mut Command)) -> String {
    let mut cmd = Command::new("nu");
    cmd.args(["-c", "echo $nu.default-config-dir"]);
    configure_env(&mut cmd);
    let output = cmd
        .output()
        .expect("nu must be on PATH (installed by .github/actions/test-setup)");
    assert!(
        output.status.success(),
        "nu exited non-zero:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// `XDG_CONFIG_HOME`, when set to an absolute path, wins on every Unix
/// (including macOS).
#[test]
fn nu_default_config_dir_uses_xdg_when_set() {
    let temp = tempfile::tempdir().unwrap();
    let xdg = temp.path().join("custom-xdg");
    let actual = run_nu_default_config_dir(|cmd| {
        cmd.env("XDG_CONFIG_HOME", &xdg);
    });
    assert_eq!(actual, xdg.join("nushell").to_string_lossy());
}

/// Without `XDG_CONFIG_HOME`, macOS falls back to
/// `~/Library/Application Support/nushell` (via `dirs::config_dir()`), not
/// `~/.config/nushell`.
#[cfg(target_os = "macos")]
#[test]
fn nu_default_config_dir_macos_fallback_is_library_application_support() {
    let home = std::env::var("HOME").expect("HOME is set on macOS");
    let actual = run_nu_default_config_dir(|cmd| {
        cmd.env_remove("XDG_CONFIG_HOME");
    });
    let expected = format!("{home}/Library/Application Support/nushell");
    assert_eq!(actual, expected);
}
