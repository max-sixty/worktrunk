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

/// A remote host that names a forge without classifying as one.
///
/// The former classifier matched a forge name anywhere in the hostname, so
/// `github-personal`, `github-enterprise.acme.com`, and `mygithub.com` all
/// resolved to GitHub. Classification now requires the name to be a whole DNS
/// label — the boundary that rejects `evil-github.com` — which leaves those
/// hosts without CI status until `forge.platform` names the forge.
///
/// This is diagnostic-only: callers can explain the compatibility change and
/// suggest `forge.platform`, but must not use it to select a forge provider.
/// Dispatch remains on [`ForgeKind::from_host`]'s exact-label boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyForgeHost {
    host: String,
    platform: ForgeKind,
}

impl LegacyForgeHost {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn platform(&self) -> ForgeKind {
        self.platform
    }

    /// Recognize a host the former substring classifier matched and the
    /// exact-label boundary no longer does.
    ///
    /// Every URL shape qualifies. The branded multi-account SSH alias
    /// (`git@github-personal:owner/repo`) is one instance of the general case,
    /// not a rule of its own: what the two shapes share is a hostname naming a
    /// forge that nothing will dispatch to.
    fn from_url(url: &str) -> Option<Self> {
        let host = normalized_hostname(GitRemoteUrl::parse(url)?.host());
        if ForgeKind::from_host(&host).is_some() {
            return None;
        }
        let platform = branded_forge_name(&host)?;
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

/// The forge whose name appears anywhere in `host`, in [`ForgeKind::from_host`]'s
/// precedence order. `host` must already be [`normalized_hostname`]d, so these
/// lowercase literals compare case-insensitively.
///
/// Diagnostic use only — this is deliberately the boundary
/// [`ForgeKind::from_host`] does not draw.
///
/// Azure DevOps is absent, and the asymmetry is the point. A brand name is a
/// naming convention — companies do run `github-enterprise.acme.com` — and no
/// rule can tell that from `evil-github.com`, so classification declines both
/// and this hint offers both the same opt-in. The two Azure service domains are
/// matched by suffix instead, which is an ownership check: a host containing
/// `dev.azure.com` or `visualstudio.com` without ending in one is outside those
/// domains by construction, and Azure DevOps Server, the self-hosted edition,
/// runs on corporate hostnames carrying neither string. Widening here would
/// reach no real installation while telling the owner of a lookalike to
/// configure it as a forge.
fn branded_forge_name(host: &str) -> Option<ForgeKind> {
    [
        ("github", ForgeKind::GitHub),
        ("gitlab", ForgeKind::GitLab),
        ("gitea", ForgeKind::Gitea),
    ]
    .into_iter()
    .find_map(|(name, platform)| host.contains(name).then_some(platform))
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

    /// Return a diagnostic for a legacy forge host on the primary remote.
    ///
    /// A configured platform resolves the ambiguity and suppresses the
    /// diagnostic. The returned value never participates in provider dispatch.
    pub fn legacy_forge_host(&self) -> Option<LegacyForgeHost> {
        if self.configured_ci_platform().is_some() {
            return None;
        }

        let remote = self.primary_remote().ok()?;
        let url = self.effective_remote_url(&remote)?;
        LegacyForgeHost::from_url(&url)
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
    fn test_legacy_forge_host_covers_every_shape_the_substring_rule_matched() {
        // The branded multi-account SSH alias, a hyphenated self-hosted
        // instance, and a forge name buried in a label — one rule, every
        // transport. The host is the one the message prints.
        for (url, host, platform) in [
            (
                "git@github-personal:owner/repo.git",
                "github-personal",
                ForgeKind::GitHub,
            ),
            (
                "ssh://git@gitlab-work/owner/repo.git",
                "gitlab-work",
                ForgeKind::GitLab,
            ),
            (
                "git@gitea-local:owner/repo.git",
                "gitea-local",
                ForgeKind::Gitea,
            ),
            (
                "https://gitlab-internal.company.com/owner/repo.git",
                "gitlab-internal.company.com",
                ForgeKind::GitLab,
            ),
            (
                "git@gitea-mirror.example.com:owner/repo.git",
                "gitea-mirror.example.com",
                ForgeKind::Gitea,
            ),
            (
                "https://mygithub.com/owner/repo.git",
                "mygithub.com",
                ForgeKind::GitHub,
            ),
            // Case and port are normalized away before the name is read.
            (
                "https://GitHub-Enterprise.ACME.com:8443/owner/repo.git",
                "github-enterprise.acme.com",
                ForgeKind::GitHub,
            ),
            // A lookalike gets the same hint. Nothing distinguishes it from a
            // legitimately branded instance by name, and the hint only offers
            // an opt-in the user must take deliberately.
            (
                "git@github-personal.attacker.example:owner/repo.git",
                "github-personal.attacker.example",
                ForgeKind::GitHub,
            ),
        ] {
            let legacy = LegacyForgeHost::from_url(url).expect(url);
            assert_eq!(legacy.host(), host, "{url}");
            assert_eq!(legacy.platform(), platform, "{url}");
            // Diagnostic only: the exact-label boundary still rejects the host.
            assert_eq!(ForgeKind::from_host(legacy.host()), None, "{url}");
        }
    }

    #[test]
    fn test_legacy_forge_host_is_silent_when_the_host_classifies_or_names_no_forge() {
        for url in [
            // Classifies on the exact label — there is nothing to explain.
            "git@github.com:owner/repo.git",
            "https://gitlab.example.com/owner/repo.git",
            "https://myorg.visualstudio.com/proj/_git/repo",
            // Names no forge at all.
            "git@work:owner/repo.git",
            "https://bitbucket.org/owner/repo.git",
            "https://codeberg.org/owner/repo.git",
            // Userinfo resolves to the network host, which names no forge.
            "https://github.com@attacker.example/owner/repo.git",
            // Azure DevOps stays out: a host carrying a service domain without
            // ending in it is outside those domains, and the self-hosted
            // edition carries neither string. See `branded_forge_name`.
            "https://dev.azure.com.attacker.example/org/proj/_git/repo",
            "https://tfs.contoso.com/DefaultCollection/proj/_git/repo",
            // Not a remote URL — a local path naming a forge stays silent.
            "/srv/git/github-mirror.git",
        ] {
            assert_eq!(LegacyForgeHost::from_url(url), None, "{url}");
        }
    }

    #[test]
    fn test_configured_platform_suppresses_legacy_forge_host_diagnostic() {
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
        assert_eq!(repo.legacy_forge_host(), None);
    }
}
