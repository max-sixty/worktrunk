//! Detection and validation of the Git executable used by Worktrunk.

use anyhow::Context;
use once_cell::sync::OnceCell;

use crate::shell_exec::Cmd;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GitVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl GitVersion {
    const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

const MINIMUM_GIT_VERSION: GitVersion = GitVersion::new(2, 43, 0);

struct GitInfo {
    version: GitVersion,
    reported_version: String,
}

static GIT_INFO: OnceCell<GitInfo> = OnceCell::new();

fn current_git() -> anyhow::Result<&'static GitInfo> {
    GIT_INFO.get_or_try_init(|| {
        let output = Cmd::new("git")
            .arg("--version")
            .run()
            .context("Failed to run git --version")?;
        anyhow::ensure!(output.status.success(), "git --version failed");
        parse_git_version(&output.stdout)
    })
}

/// Return the version string reported by the Git executable on `PATH`.
pub fn git_version() -> anyhow::Result<String> {
    Ok(current_git()?.reported_version.clone())
}

/// Reject Git versions older than Worktrunk's supported minimum.
pub fn require_minimum_git() -> anyhow::Result<()> {
    let current = current_git()?;
    anyhow::ensure!(
        current.version >= MINIMUM_GIT_VERSION,
        "Git {} is unsupported; Worktrunk requires Git {} or newer",
        current.version,
        MINIMUM_GIT_VERSION
    );
    Ok(())
}

fn parse_git_version(stdout: &[u8]) -> anyhow::Result<GitInfo> {
    let stdout = std::str::from_utf8(stdout)
        .context("git --version returned invalid UTF-8")?
        .trim();
    let reported_version = stdout
        .strip_prefix("git version ")
        .context("git --version returned an unexpected response")?;
    anyhow::ensure!(
        !reported_version.is_empty() && !reported_version.contains(['\r', '\n']),
        "git --version returned an unexpected response"
    );

    let numeric_version = reported_version
        .split_whitespace()
        .next()
        .context("git --version returned an unexpected response")?;
    let mut components = numeric_version.split('.');
    let mut parse_component = |name: &str| -> anyhow::Result<u32> {
        let component = components.next().unwrap_or_default();
        let digits = component
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        anyhow::ensure!(
            !digits.is_empty(),
            "could not parse Git {name} version from {reported_version:?}"
        );
        digits.parse().with_context(|| {
            format!("could not parse Git {name} version from {reported_version:?}")
        })
    };

    Ok(GitInfo {
        version: GitVersion {
            major: parse_component("major")?,
            minor: parse_component("minor")?,
            patch: parse_component("patch").unwrap_or(0),
        },
        reported_version: reported_version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_platform_version_formats() {
        for (input, expected, reported) in [
            ("git version 2.43.0\n", GitVersion::new(2, 43, 0), "2.43.0"),
            ("git version 2.43\n", GitVersion::new(2, 43, 0), "2.43"),
            (
                "git version 2.45.GIT\n",
                GitVersion::new(2, 45, 0),
                "2.45.GIT",
            ),
            (
                "git version 2.50.1 (Apple Git-155)\n",
                GitVersion::new(2, 50, 1),
                "2.50.1 (Apple Git-155)",
            ),
            (
                "git version 2.47.1.windows.1\n",
                GitVersion::new(2, 47, 1),
                "2.47.1.windows.1",
            ),
        ] {
            let parsed = parse_git_version(input.as_bytes()).unwrap();
            assert_eq!(parsed.version, expected);
            assert_eq!(parsed.reported_version, reported);
        }
    }

    #[test]
    fn rejects_malformed_version_output() {
        for malformed in [
            b"2.43.0".as_slice(),
            b"git version \n",
            b"git version 2.43.0\nextra",
            b"git version 2.x.0\n",
            b"git version 4294967296.43.0\n",
            b"git version 2.43.0\xff\n",
        ] {
            assert!(parse_git_version(malformed).is_err());
        }
    }
}
