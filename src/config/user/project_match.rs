//! Matching `[projects."<pattern>"]` keys against a repository's project
//! identifier.
//!
//! # What a key matches
//!
//! A key is either a literal project identifier (`github.com/owner/repo`) or a
//! pattern containing `*`, which stands for any run of characters — `/`
//! included, so `git.company.example/*` covers the nested-group identifier
//! `git.company.example/group/team/repo`. `*` is the only metacharacter; every
//! other character, `.` and `?` among them, is literal.
//!
//! The identifier being matched is [`Repository::project_identifier`], built
//! from the raw `remote.<name>.url` value. `url.insteadOf` rewrites are not
//! applied, so a pattern targets the hostname as it is spelled in
//! `.git/config`. This is the same string an exact key has always matched.
//!
//! [`Repository::project_identifier`]: crate::git::Repository::project_identifier
//!
//! # Ordering
//!
//! Several keys can match one repository. [`matching_keys`] returns them
//! least- to most-specific, so a caller that folds them in order lets the most
//! specific win, and the layering reads the same as global → project does
//! elsewhere in the config.
//!
//! Specificity is the number of literal (non-`*`) characters in the pattern:
//! `git.company.example/team/*` beats `git.company.example/*` beats `*`. A
//! literal key is more specific than any pattern that matches the same
//! identifier, because matching every one of its characters literally is what
//! makes it literal. Equal-specificity patterns fall back to lexicographic
//! order so the result never depends on map iteration luck.
//!
//! # Where patterns do not apply
//!
//! Writes are always keyed by the exact identifier: `wt config approvals add`
//! records under `github.com/owner/repo`, never under a pattern that happens
//! to match it. Removals are exact for the same reason — one repository's
//! `wt config approvals clear` must not empty a pattern entry that other
//! repositories rely on.

use std::collections::BTreeMap;

/// Whether `pattern` matches `identifier`, treating `*` as any run of
/// characters and every other character as a literal.
pub(crate) fn matches(pattern: &str, identifier: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == identifier;
    }
    // `(?s)` lets `.` cover a newline, so the path-shaped identifier a
    // remoteless repository falls back to matches on every platform that
    // permits one in a directory name.
    let anchored = format!(
        "(?s)\\A{}\\z",
        pattern
            .split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(".*")
    );
    // The parts are `regex::escape`d and the joiner is a literal `.*`, so the
    // only way to build an invalid pattern here is a bug in this function.
    regex::Regex::new(&anchored).is_ok_and(|re| re.is_match(identifier))
}

/// The number of literal characters in `pattern` — its specificity rank.
fn literal_len(pattern: &str) -> usize {
    pattern.chars().filter(|c| *c != '*').count()
}

/// Every key of `projects` matching `identifier`, least- to most-specific.
///
/// A literal key sorts last, so a caller folding the results applies it over
/// any pattern that also matched.
pub(crate) fn matching_keys<'a, T>(
    projects: &'a BTreeMap<String, T>,
    identifier: &str,
) -> Vec<&'a T> {
    // The common case is a config with no patterns at all: take the exact hit
    // and skip the scan entirely.
    if !projects.keys().any(|key| key.contains('*')) {
        return projects.get(identifier).into_iter().collect();
    }

    let mut matched: Vec<(usize, &str, &T)> = projects
        .iter()
        .filter(|(key, _)| matches(key, identifier))
        .map(|(key, value)| (literal_len(key), key.as_str(), value))
        .collect();
    matched.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    matched.into_iter().map(|(_, _, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_key_matches_only_itself() {
        assert!(matches("github.com/owner/repo", "github.com/owner/repo"));
        assert!(!matches("github.com/owner/repo", "github.com/owner/other"));
        // `.` is literal, not "any character".
        assert!(!matches("github.com/owner/repo", "githubXcom/owner/repo"));
    }

    #[test]
    fn test_star_spans_path_separators() {
        // The motivating case: one key for every repo on a host, including
        // nested GitLab groups.
        assert!(matches(
            "git.company.example/*",
            "git.company.example/owner/repo"
        ));
        assert!(matches(
            "git.company.example/*",
            "git.company.example/group/team/repo"
        ));
        assert!(!matches("git.company.example/*", "github.com/owner/repo"));
    }

    #[test]
    fn test_star_placement() {
        assert!(matches("*", "github.com/owner/repo"));
        assert!(matches("*/owner/*", "github.com/owner/repo"));
        assert!(matches("github.com/owner/repo*", "github.com/owner/repo"));
        // A bare host with no trailing separator is not a prefix match.
        assert!(!matches("git.company.example", "git.company.example/o/r"));
    }

    #[test]
    fn test_matching_keys_orders_least_to_most_specific() {
        let projects = BTreeMap::from([
            ("*".to_string(), "all"),
            ("git.company.example/*".to_string(), "host"),
            ("git.company.example/team/*".to_string(), "team"),
            ("git.company.example/team/repo".to_string(), "exact"),
            ("github.com/*".to_string(), "other-host"),
        ]);
        assert_eq!(
            matching_keys(&projects, "git.company.example/team/repo"),
            vec![&"all", &"host", &"team", &"exact"]
        );
    }

    #[test]
    fn test_matching_keys_without_patterns_is_exact() {
        let projects = BTreeMap::from([
            ("github.com/owner/repo".to_string(), "mine"),
            ("github.com/owner/other".to_string(), "theirs"),
        ]);
        assert_eq!(
            matching_keys(&projects, "github.com/owner/repo"),
            vec![&"mine"]
        );
        assert!(matching_keys(&projects, "github.com/owner/absent").is_empty());
    }

    #[test]
    fn test_equal_specificity_is_lexicographic() {
        // Both patterns have four literal characters, so neither outranks the
        // other and the tie breaks on the key itself — `*/b/c*` before
        // `*a/b/*`, whatever order the map was built in.
        let projects = BTreeMap::from([
            ("*a/b/*".to_string(), "star-a"),
            ("*/b/c*".to_string(), "star-slash"),
        ]);
        assert_eq!(
            matching_keys(&projects, "a/b/c"),
            vec![&"star-slash", &"star-a"]
        );
    }
}
