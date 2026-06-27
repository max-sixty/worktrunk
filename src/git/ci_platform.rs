//! CI platform identification.
//!
//! [`CiPlatform`] names the forge a repository's CI runs on (GitHub, GitLab,
//! Gitea, or Azure DevOps). It comes from project config (`forge.platform`, or
//! the deprecated `ci.platform`) when set, otherwise from the remote URL host —
//! see [`Repository::ci_platform`].

use crate::git::{GitRemoteUrl, Repository};

/// The forge a repository's CI runs on.
///
/// Resolved by [`Repository::ci_platform`]: project config (`forge.platform`,
/// or the deprecated `ci.platform`) takes precedence, falling back to the
/// remote URL host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum CiPlatform {
    GitHub,
    GitLab,
    /// Experimental — Gitea CI status via the `tea` CLI.
    Gitea,
    #[strum(serialize = "azure-devops", serialize = "azuredevops")]
    AzureDevOps,
}

/// Identify the CI platform from a remote URL host ("github" / "gitlab" /
/// "gitea" / Azure DevOps).
fn platform_from_url(url: &str) -> Option<CiPlatform> {
    let parsed = GitRemoteUrl::parse(url)?;
    if parsed.is_github() {
        Some(CiPlatform::GitHub)
    } else if parsed.is_gitlab() {
        Some(CiPlatform::GitLab)
    } else if parsed.is_gitea() {
        Some(CiPlatform::Gitea)
    } else if parsed.is_azure_devops() {
        Some(CiPlatform::AzureDevOps)
    } else {
        None
    }
}

impl Repository {
    /// The CI platform for this repository, or `None` if it can't be determined.
    ///
    /// Priority order:
    /// 1. Project config `forge.platform` (or the deprecated `ci.platform`)
    /// 2. `remote_hint`'s effective URL host, when `remote_hint` is given
    /// 3. The primary remote's effective URL host
    ///
    /// For a remote branch, pass its remote as `remote_hint` so the right
    /// platform is picked in mixed-remote repos (e.g. GitHub + GitLab).
    /// Effective URLs are used so `url.insteadOf` aliases resolve.
    pub fn ci_platform(&self, remote_hint: Option<&str>) -> Option<CiPlatform> {
        if let Some(platform) = self.configured_ci_platform() {
            return Some(platform);
        }

        if let Some(remote) = remote_hint
            && let Some(url) = self.effective_remote_url(remote)
            && let Some(platform) = platform_from_url(&url)
        {
            tracing::debug!(platform = %platform, remote = %remote, "Detected CI platform {platform} from remote '{remote}' (hint)");
            return Some(platform);
        }

        if let Ok(remote) = self.primary_remote()
            && let Some(url) = self.effective_remote_url(&remote)
            && let Some(platform) = platform_from_url(&url)
        {
            tracing::debug!(platform = %platform, remote = %remote, "Detected CI platform {platform} from remote '{remote}'");
            return Some(platform);
        }

        None
    }

    /// The CI platform set in project config (`forge.platform` / `ci.platform`).
    ///
    /// `None` when unset or unrecognized. Resolved once per repository handle,
    /// so an unrecognized value warns a single time rather than once per branch
    /// `wt list` probes.
    fn configured_ci_platform(&self) -> Option<CiPlatform> {
        *self.cache.configured_ci_platform.get_or_init(|| {
            let raw = self
                .project_config()
                .ok()
                .flatten()?
                .forge_platform()
                .map(str::to_string)?;
            match raw.parse::<CiPlatform>() {
                Ok(platform) => {
                    tracing::debug!(platform = %platform, "Using CI platform from config: {platform}");
                    Some(platform)
                }
                Err(_) => {
                    tracing::warn!(
                        value = %raw,
                        "Invalid CI platform in config: '{raw}'. Expected 'github', 'gitlab', 'gitea', or 'azure-devops'."
                    );
                    None
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ci_platform_string_roundtrip() {
        assert_eq!(
            "github".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            "gitlab".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitLab)
        );
        assert_eq!("gitea".parse::<CiPlatform>().ok(), Some(CiPlatform::Gitea));
        // Azure DevOps accepts both spellings; `azure-devops` is canonical.
        assert_eq!(
            "azure-devops".parse::<CiPlatform>().ok(),
            Some(CiPlatform::AzureDevOps)
        );
        assert_eq!(
            "azuredevops".parse::<CiPlatform>().ok(),
            Some(CiPlatform::AzureDevOps)
        );
        assert_eq!(CiPlatform::GitHub.to_string(), "github");
        assert_eq!(CiPlatform::GitLab.to_string(), "gitlab");

        // Unrecognized values, including wrong case, must not parse.
        assert!("invalid".parse::<CiPlatform>().is_err());
        assert!("GITHUB".parse::<CiPlatform>().is_err());
        assert!("GitHub".parse::<CiPlatform>().is_err());
    }

    #[test]
    fn test_platform_from_url() {
        // GitHub — various URL formats, plus GitHub Enterprise.
        for url in [
            "https://github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
            "https://github.mycompany.com/owner/repo.git",
            "http://github.com/owner/repo.git",
            "git://github.com/owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), Some(CiPlatform::GitHub), "{url}");
        }

        // GitLab — various URL formats, plus self-hosted instances.
        for url in [
            "https://gitlab.com/owner/repo.git",
            "git@gitlab.com:owner/repo.git",
            "https://gitlab.example.com/owner/repo.git",
            "http://gitlab.example.com/owner/repo.git",
            "git://gitlab.mycompany.com/owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), Some(CiPlatform::GitLab), "{url}");
        }

        // Gitea — gitea.com and self-hosted instances with "gitea" in the host.
        for url in [
            "https://gitea.com/owner/repo.git",
            "git@gitea.example.com:owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), Some(CiPlatform::Gitea), "{url}");
        }

        // Azure DevOps — HTTPS, SSH, and the legacy visualstudio.com host.
        for url in [
            "https://dev.azure.com/myorg/myproject/_git/myrepo",
            "git@ssh.dev.azure.com:v3/myorg/myproject/myrepo",
            "https://myorg.visualstudio.com/myproject/_git/myrepo",
        ] {
            assert_eq!(
                platform_from_url(url),
                Some(CiPlatform::AzureDevOps),
                "{url}"
            );
        }

        // Unknown forges (a Gitea/Forgejo host without "gitea" in the name
        // needs an explicit `forge.platform` override).
        assert_eq!(
            platform_from_url("https://bitbucket.org/owner/repo.git"),
            None
        );
        assert_eq!(
            platform_from_url("https://codeberg.org/owner/repo.git"),
            None
        );
    }
}
