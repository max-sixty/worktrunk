//! CI platform identification.
//!
//! [`ForgeKind`] names the forge a repository's CI runs on (GitHub, GitLab,
//! Gitea, or Azure DevOps). It comes from the configured forge platform when
//! set — the repository's own `[forge].platform` (or the deprecated
//! `ci.platform`), else a matching user-config `[projects."…"].forge` entry —
//! otherwise from the remote URL host. See [`Repository::ci_platform`].

use crate::git::{GitRemoteUrl, RefType, Repository};

/// A known forge.
///
/// This is the canonical identity shared by configuration, remote-host
/// classification, remote-ref dispatch, and CI dispatch. Unknown hosts stay
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

impl ForgeKind {
    /// Classify a forge from a remote hostname.
    ///
    /// GitHub, GitLab, and Gitea are brand names, and a self-hosted instance
    /// puts its brand in the hostname however it likes: `github.mycompany.com`,
    /// `github-enterprise.acme.com`, `mygithub.com`, the `github-personal` SSH
    /// alias. So any host carrying the name matches, first match winning. Azure
    /// DevOps is not a brand in the hostname but two fixed service domains, so
    /// it matches by domain suffix and takes precedence inside those domains —
    /// `evil-visualstudio.com` and `dev.azure.com.attacker.example` are outside
    /// them and do not match.
    ///
    /// Recall is what this optimizes, not resistance to a lookalike name. The
    /// hostname comes out of the user's own `.git/config`, and whoever can put
    /// a host there can put code there too, so the trust decision was made at
    /// clone time; all this picks is which forge CLI runs against a remote the
    /// user already builds from. A stricter name test would not hold anyway —
    /// an attacker controls their own DNS, so `github.attacker.example`
    /// satisfies one — while it does shut out the self-hoster who named a box
    /// `github-enterprise.acme.com` years ago. `[forge].platform` overrides the
    /// guess, and names the forge for a host that carries no brand at all.
    pub fn from_host(host: &str) -> Option<Self> {
        let host = normalized_hostname(host);
        if normalized_host_is_within(&host, "dev.azure.com")
            || normalized_host_is_within(&host, "visualstudio.com")
        {
            Some(Self::AzureDevOps)
        } else if host.contains("github") {
            Some(Self::GitHub)
        } else if host.contains("gitlab") {
            Some(Self::GitLab)
        } else if host.contains("gitea") {
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
    /// 1. The configured forge platform — the repository's `[forge].platform`
    ///    (or the deprecated `ci.platform`), else a matching user-config
    ///    `[projects."…"].forge.platform`
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

    /// The configured CI platform: the repository's `[forge].platform` (or the
    /// deprecated `ci.platform`), else a matching user-config
    /// `[projects."…"].forge.platform`.
    ///
    /// The repository's own block wins because it is the more specific of the
    /// two — a user entry keyed `git.company.example/*` states what the host
    /// is, and a repository that disagrees knows better.
    ///
    /// `None` when unset or unrecognized. Resolved once per repository handle,
    /// so an unrecognized value warns a single time rather than once per branch
    /// `wt list` probes.
    fn configured_ci_platform(&self) -> Option<ForgeKind> {
        *self.cache.configured_ci_platform.get_or_init(|| {
            let raw = self.configured_forge_platform()?;
            match raw.parse::<ForgeKind>() {
                Ok(platform) => {
                    tracing::debug!(platform = %platform, "Using CI platform from config: {platform}");
                    Some(platform)
                }
                Err(_) => {
                    tracing::warn!(
                        value = %raw,
                        "Invalid CI platform '{raw}' (from `[forge]` in project config or a `[projects]` entry in user config). Expected 'github', 'gitlab', 'gitea', or 'azure-devops'."
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
    fn test_fixed_azure_domains_take_precedence_over_brand_names() {
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
    fn test_branded_hosts_classify_however_the_instance_is_named() {
        // A self-hosted instance carries its brand wherever it likes: its own
        // label, a hyphenated label, inside a word, or a single-label SSH
        // alias. All of them resolve without config.
        for (url, platform) in [
            (
                "https://github-enterprise.acme.com/owner/repo.git",
                ForgeKind::GitHub,
            ),
            ("https://mygithub.com/owner/repo.git", ForgeKind::GitHub),
            ("git@github-personal:owner/repo.git", ForgeKind::GitHub),
            (
                "https://gitlab-internal.company.com/owner/repo.git",
                ForgeKind::GitLab,
            ),
            ("ssh://git@gitlab-work/owner/repo.git", ForgeKind::GitLab),
            (
                "git@gitea-mirror.example.com:owner/repo.git",
                ForgeKind::Gitea,
            ),
            // Case and port are normalized away before the name is read.
            (
                "https://GitHub-Enterprise.ACME.com:8443/owner/repo.git",
                ForgeKind::GitHub,
            ),
        ] {
            assert_eq!(platform_from_url(url), Some(platform), "{url}");
        }

        // A host carrying no brand still needs `forge.platform`.
        for url in [
            "https://bitbucket.org/owner/repo.git",
            "https://codeberg.org/owner/repo.git",
            "https://git.example.com/owner/repo.git",
            "git@work:owner/repo.git",
        ] {
            assert_eq!(platform_from_url(url), None, "{url}");
        }
    }

    #[test]
    fn test_configured_platform_overrides_host_inference() {
        // The remote's own name says GitLab; config says GitHub and wins.
        let test = crate::testing::TestRepo::new();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://gitlab-internal.company.com/owner/repo.git",
        ]);
        test.write_project_config("[forge]\nplatform = \"github\"\n");

        let repo = Repository::at(test.root_path().to_path_buf()).unwrap();
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitHub));
    }

    /// Build a repo with `remote_url` and a user config carrying the given
    /// `[projects."<key>"].forge.platform` entries.
    ///
    /// Returns the `TestRepo` alongside the `Repository` so the caller keeps
    /// the checkout's tempdir alive for the duration of the test.
    fn repo_with_user_forge(
        remote_url: &str,
        entries: &[(&str, &str)],
    ) -> (crate::testing::TestRepo, Repository) {
        let test = crate::testing::TestRepo::new();
        test.run_git(&["remote", "add", "origin", remote_url]);

        let mut user_config = crate::config::UserConfig::default();
        for (key, platform) in entries {
            user_config
                .projects
                .entry((*key).to_string())
                .or_default()
                .forge = crate::config::ProjectForgeConfig {
                platform: Some((*platform).to_string()),
                hostname: None,
            };
        }

        let repo = Repository::at(test.root_path().to_path_buf()).unwrap();
        repo.cache
            .user_config
            .set(user_config)
            .expect("user config not yet initialized");
        (test, repo)
    }

    #[test]
    fn test_user_project_pattern_names_platform_for_a_whole_host() {
        // The motivating case: a self-hosted forge whose hostname carries no
        // brand resolves for every repo on it, with no `[forge]` block in any
        // of them. Built-in inference alone gives `None` here.
        let (_test, repo) = repo_with_user_forge(
            "https://git.company.example/owner/repo.git",
            &[("git.company.example/*", "gitlab")],
        );
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitLab));
    }

    #[test]
    fn test_user_project_pattern_covers_nested_groups() {
        let (_test, repo) = repo_with_user_forge(
            "https://git.company.example/group/team/repo.git",
            &[("git.company.example/*", "gitlab")],
        );
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitLab));
    }

    #[test]
    fn test_more_specific_user_pattern_wins() {
        // The Gitea instance lives under one namespace on an otherwise-GitLab
        // host, and the narrower entry is the one that applies.
        let (_test, repo) = repo_with_user_forge(
            "https://git.company.example/tools/repo.git",
            &[
                ("git.company.example/*", "gitlab"),
                ("git.company.example/tools/*", "gitea"),
            ],
        );
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::Gitea));
    }

    #[test]
    fn test_project_forge_overrides_user_pattern() {
        // A repository that names its own forge is more specific than an entry
        // describing the host.
        let test = crate::testing::TestRepo::new();
        test.run_git(&[
            "remote",
            "add",
            "origin",
            "https://git.company.example/owner/repo.git",
        ]);
        test.write_project_config("[forge]\nplatform = \"github\"\n");

        let mut user_config = crate::config::UserConfig::default();
        user_config
            .projects
            .entry("git.company.example/*".to_string())
            .or_default()
            .forge
            .platform = Some("gitlab".to_string());

        let repo = Repository::at(test.root_path().to_path_buf()).unwrap();
        repo.cache.user_config.set(user_config).unwrap();
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitHub));
    }

    #[test]
    fn test_unmatched_user_pattern_falls_through_to_inference() {
        let (_test, repo) = repo_with_user_forge(
            "https://gitlab.com/owner/repo.git",
            &[("git.company.example/*", "github")],
        );
        assert_eq!(repo.ci_platform(None), Some(ForgeKind::GitLab));
    }

    #[test]
    fn test_invalid_user_platform_leaves_the_host_unresolved() {
        // An unrecognized value warns once and is dropped, rather than
        // resolving the host to a forge that doesn't exist.
        let (_test, repo) = repo_with_user_forge(
            "https://git.company.example/owner/repo.git",
            &[("git.company.example/*", "bitbucket")],
        );
        assert_eq!(repo.ci_platform(None), None);
    }
}
