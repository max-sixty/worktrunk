//! CI platform detection.
//!
//! Determines whether a repository uses GitHub or GitLab based on
//! project config override or remote URL detection.

use worktrunk::git::Repository;

/// CI platform detected from project config override or remote URL.
///
/// Platform is determined by:
/// 1. Project config `[ci] platform = "github" | "gitlab"` (takes precedence)
/// 2. Remote URL detection (searches for "github" or "gitlab" in URL)
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum CiPlatform {
    GitHub,
    GitLab,
}

/// Detect the CI platform from a remote URL by searching for "github" or "gitlab".
pub fn detect_platform_from_url(url: &str) -> Option<CiPlatform> {
    let url_lower = url.to_ascii_lowercase();
    if url_lower.contains("github") {
        Some(CiPlatform::GitHub)
    } else if url_lower.contains("gitlab") {
        Some(CiPlatform::GitLab)
    } else {
        None
    }
}

/// Get the CI platform for a repository.
///
/// If `platform_override` is provided (from project config `[ci] platform`),
/// uses that value directly. Otherwise, detects platform from the primary
/// remote URL.
pub fn get_platform_for_repo(
    repo: &Repository,
    platform_override: Option<&str>,
) -> Option<CiPlatform> {
    // Config override takes precedence
    if let Some(platform_str) = platform_override {
        if let Ok(platform) = platform_str.parse::<CiPlatform>() {
            log::debug!("Using CI platform from config override: {}", platform);
            return Some(platform);
        }
        log::warn!(
            "Invalid CI platform in config: '{}'. Expected 'github' or 'gitlab'.",
            platform_str
        );
    }

    // Fall back to URL detection
    let url = repo.primary_remote_url()?;
    detect_platform_from_url(&url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_platform_from_url() {
        // GitHub - various URL formats
        assert_eq!(
            detect_platform_from_url("https://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("git@github.com:owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("ssh://git@github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );

        // GitHub Enterprise
        assert_eq!(
            detect_platform_from_url("https://github.mycompany.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );

        // GitLab - various URL formats
        assert_eq!(
            detect_platform_from_url("https://gitlab.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );
        assert_eq!(
            detect_platform_from_url("git@gitlab.com:owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Self-hosted GitLab
        assert_eq!(
            detect_platform_from_url("https://gitlab.example.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Legacy schemes (http://, git://) - common on self-hosted installations
        assert_eq!(
            detect_platform_from_url("http://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("git://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("http://gitlab.example.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );
        assert_eq!(
            detect_platform_from_url("git://gitlab.mycompany.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Unknown platforms
        assert_eq!(
            detect_platform_from_url("https://bitbucket.org/owner/repo.git"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://codeberg.org/owner/repo.git"),
            None
        );
    }

    #[test]
    fn test_platform_override_github() {
        // Config override should take precedence over URL detection
        assert_eq!(
            "github".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitHub)
        );
    }

    #[test]
    fn test_platform_override_gitlab() {
        // Config override should take precedence over URL detection
        assert_eq!(
            "gitlab".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitLab)
        );
    }

    #[test]
    fn test_platform_override_invalid() {
        // Invalid platform strings should not parse
        assert!("invalid".parse::<CiPlatform>().is_err());
        assert!("GITHUB".parse::<CiPlatform>().is_err()); // Case-sensitive
        assert!("GitHub".parse::<CiPlatform>().is_err()); // Case-sensitive
    }
}
