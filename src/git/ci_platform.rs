//! CI platform identification.
//!
//! [`ForgeKind`] names the forge a repository's CI runs on (GitHub, GitLab,
//! Gitea, or Azure DevOps). It comes from project config (`forge.platform`, or
//! the deprecated `ci.platform`) when set, otherwise from the remote URL host —
//! see [`Repository::ci_platform`].

use crate::git::{GitRemoteUrl, RefType, Repository};

/// A known forge.
///
/// This is the canonical identity shared by configuration, remote-host
/// classification, remote-ref providers, and CI dispatch. Unknown hosts stay
/// outside the enum as `None`; callers that expose an explicit `unknown` value
/// add it only at that output boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum ForgeKind {
    GitHub,
    GitLab,
    /// Experimental — Gitea CI status via the `tea` CLI.
    Gitea,
    #[strum(serialize = "azure-devops", serialize = "azuredevops")]
    AzureDevOps,
}

/// A branded SSH host alias that the former substring classifier recognized.
///
/// This is diagnostic-only: callers can explain the compatibility change and
/// suggest `forge.platform`, but must not use it to select a forge provider.
/// Dispatch remains on [`ForgeKind::from_host`]'s exact-label boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyForgeAlias {
    host: String,
    platform: ForgeKind,
}

impl LegacyForgeAlias {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn platform(&self) -> ForgeKind {
        self.platform
    }

    /// Recognize the common multi-account SSH alias form (`github-personal`).
    ///
    /// Only SSH URLs with a single-label, forge-prefixed host qualify. Dotted
    /// hosts stay out even when they contain a forge name: those are exactly
    /// the security-sensitive lookalikes the exact-label classifier rejects.
    fn from_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let is_ssh = url.starts_with("ssh://")
            || url
                .split_once(':')
                .is_some_and(|(authority, _)| !authority.contains('/') && authority.contains('@'));
        if !is_ssh {
            return None;
        }

        let parsed = GitRemoteUrl::parse(url)?;
        let host = normalized_hostname(parsed.host());
        if host.contains('.') || ForgeKind::from_host(&host).is_some() {
            return None;
        }

        let platform = [
            ("github-", ForgeKind::GitHub),
            ("gitlab-", ForgeKind::GitLab),
            ("gitea-", ForgeKind::Gitea),
        ]
        .into_iter()
        .find_map(|(prefix, platform)| {
            host.strip_prefix(prefix)
                .filter(|suffix| !suffix.is_empty())
                .map(|_| platform)
        })?;

        Some(Self { host, platform })
    }
}

impl ForgeKind {
    /// Classify a forge from a remote hostname.
    ///
    /// GitHub, GitLab, and Gitea self-hosted instances commonly put the forge
    /// name in its own DNS label. Azure DevOps has fixed service domains, so it
    /// uses domain-suffix matching and takes precedence over branded labels
    /// inside those domains. Both rules respect label boundaries:
    /// `github-mirror.example` and `dev.azure.com.attacker.example` are not
    /// recognized. A branded hostname can opt in through `[forge].platform`.
    pub fn from_host(host: &str) -> Option<Self> {
        let host = normalized_hostname(host);
        if normalized_host_is_within(&host, "dev.azure.com")
            || normalized_host_is_within(&host, "visualstudio.com")
        {
            Some(Self::AzureDevOps)
        } else if host_has_label(&host, "github") {
            Some(Self::GitHub)
        } else if host_has_label(&host, "gitlab") {
            Some(Self::GitLab)
        } else if host_has_label(&host, "gitea") {
            Some(Self::Gitea)
        } else {
            None
        }
    }

    /// PR/MR vocabulary for change requests on this forge.
    pub const fn ref_type(self) -> RefType {
        match self {
            Self::GitLab => RefType::Mr,
            Self::GitHub | Self::Gitea | Self::AzureDevOps => RefType::Pr,
        }
    }
}

/// Lowercase a hostname and remove transport-only syntax before classifying it.
///
/// `GitRemoteUrl` already strips ports from `ssh://` URLs, while HTTP(S) keeps
/// the authority intact. Treat a numeric suffix as a port so both transports
/// classify identically. A trailing DNS root dot is likewise identity-neutral.
pub(super) fn normalized_hostname(host: &str) -> String {
    let host = host.trim();
    let host = host
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(hostname, _)| hostname);
    host.trim_end_matches('.').to_ascii_lowercase()
}

fn host_has_label(host: &str, label: &str) -> bool {
    host.split('.').any(|candidate| candidate == label)
}

fn normalized_host_is_within(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
}

pub(super) fn host_is_within(host: &str, domain: &str) -> bool {
    normalized_host_is_within(&normalized_hostname(host), domain)
}

/// Identify the CI platform from a remote URL host ("github" / "gitlab" /
/// "gitea" / Azure DevOps).
fn platform_from_url(url: &str) -> Option<ForgeKind> {
    GitRemoteUrl::parse(url)?.forge_kind()
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
    pub fn ci_platform(&self, remote_hint: Option<&str>) -> Option<ForgeKind> {
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

    /// Return a diagnostic for a legacy branded SSH alias on the primary remote.
    ///
    /// A configured platform resolves the ambiguity and suppresses the
    /// diagnostic. The returned value never participates in provider dispatch.
    pub fn legacy_forge_alias(&self) -> Option<LegacyForgeAlias> {
        if self.configured_ci_platform().is_some() {
            return None;
        }

        let remote = self.primary_remote().ok()?;
        let url = self.effective_remote_url(&remote)?;
        LegacyForgeAlias::from_url(&url)
    }

    /// The CI platform set in project config (`forge.platform` / `ci.platform`).
    ///
    /// `None` when unset or unrecognized. Resolved once per repository handle,
    /// so an unrecognized value warns a single time rather than once per branch
    /// `wt list` probes.
    fn configured_ci_platform(&self) -> Option<ForgeKind> {
        *self.cache.configured_ci_platform.get_or_init(|| {
            let raw = self
                .project_config()
                .ok()
                .flatten()?
                .forge_platform()
                .map(str::to_string)?;
            match raw.parse::<ForgeKind>() {
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
        for (forge, spelling) in [
            (ForgeKind::GitHub, "github"),
            (ForgeKind::GitLab, "gitlab"),
            (ForgeKind::Gitea, "gitea"),
            (ForgeKind::AzureDevOps, "azure-devops"),
        ] {
            assert_eq!(forge.to_string(), spelling);
            assert_eq!(spelling.parse::<ForgeKind>().ok(), Some(forge));
        }

        // Azure DevOps accepts both spellings; `azure-devops` is canonical.
        assert_eq!(
            "azuredevops".parse::<ForgeKind>().ok(),
            Some(ForgeKind::AzureDevOps)
        );

        // Unrecognized values, including wrong case, must not parse.
        assert!("invalid".parse::<ForgeKind>().is_err());
        assert!("GITHUB".parse::<ForgeKind>().is_err());
        assert!("GitHub".parse::<ForgeKind>().is_err());
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
            assert_eq!(platform_from_url(url), Some(ForgeKind::GitHub), "{url}");
        }

        // GitLab — various URL formats, plus self-hosted instances.
        for url in [
            "https://gitlab.com/owner/repo.git",
            "git@gitlab.com:owner/repo.git",
            "https://gitlab.example.com/owner/repo.git",
            "http://gitlab.example.com/owner/repo.git",
            "git://gitlab.mycompany.com/owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), Some(ForgeKind::GitLab), "{url}");
        }

        // Gitea — gitea.com and self-hosted instances with "gitea" in the host.
        for url in [
            "https://gitea.com/owner/repo.git",
            "git@gitea.example.com:owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), Some(ForgeKind::Gitea), "{url}");
        }

        // Azure DevOps — HTTPS, SSH, and the legacy visualstudio.com host.
        for url in [
            "https://dev.azure.com/myorg/myproject/_git/myrepo",
            "git@ssh.dev.azure.com:v3/myorg/myproject/myrepo",
            "https://myorg.visualstudio.com/myproject/_git/myrepo",
        ] {
            assert_eq!(
                platform_from_url(url),
                Some(ForgeKind::AzureDevOps),
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

    #[test]
    fn test_platform_from_url_uses_network_host_after_userinfo() {
        for url in [
            "https://github.com@attacker.example/owner/repo.git",
            "http://gitlab.com@attacker.example/owner/repo.git",
            "git://gitea.com@attacker.example/owner/repo.git",
            "ssh://dev.azure.com@attacker.example/owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), None, "{url}");
        }
    }

    #[test]
    fn test_fixed_azure_domains_take_precedence_over_forge_labels() {
        for host in [
            "github.dev.azure.com",
            "gitlab.visualstudio.com",
            "gitea.visualstudio.com:443",
            "GITHUB.DEV.AZURE.COM.",
        ] {
            assert_eq!(
                ForgeKind::from_host(host),
                Some(ForgeKind::AzureDevOps),
                "{host}"
            );
        }
    }

    #[test]
    fn test_legacy_forge_alias_is_diagnostic_only_for_branded_ssh_aliases() {
        for (url, platform) in [
            ("git@github-personal:owner/repo.git", ForgeKind::GitHub),
            ("ssh://git@gitlab-work/owner/repo.git", ForgeKind::GitLab),
            ("git@gitea-local:owner/repo.git", ForgeKind::Gitea),
        ] {
            let alias = LegacyForgeAlias::from_url(url).expect(url);
            assert_eq!(alias.platform(), platform, "{url}");
            assert_eq!(ForgeKind::from_host(alias.host()), None, "{url}");
        }

        for url in [
            "git@github.com:owner/repo.git",
            "git@work:owner/repo.git",
            "git@notgithub-personal:owner/repo.git",
            "git@github-mirror.example:owner/repo.git",
            "git@github-personal.attacker.example:owner/repo.git",
            "https://github-personal/owner/repo.git",
        ] {
            assert_eq!(LegacyForgeAlias::from_url(url), None, "{url}");
        }
    }

    #[test]
    fn test_configured_platform_suppresses_legacy_alias_diagnostic() {
        let test = crate::testing::TestRepo::new();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "git@github-personal:owner/repo.git",
        ]);
        test.write_project_config("[forge]\nplatform = \"github\"\n");

        let repo = Repository::at(test.root_path().to_path_buf()).unwrap();
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitHub));
        assert_eq!(repo.legacy_forge_alias(), None);
    }
}
