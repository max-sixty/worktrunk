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
//! literal key outranks any pattern outright — a pattern whose stars all match
//! empty (`github.com/owner/repo*`) ties the exact key on literal count, and
//! the exact key must still win. Equal-specificity patterns fall back to
//! lexicographic order so the result never depends on map iteration luck.
//!
//! # Where patterns do not apply
//!
//! Writes are always keyed by the exact identifier: `wt config approvals add`
//! records under `github.com/owner/repo`, never under a pattern that happens
//! to match it. Removals are exact for the same reason — one repository's
//! `wt config approvals clear` must not empty a pattern entry that other
//! repositories rely on. An identifier that itself contains `*` (a starred
//! remote URL, or a no-remote path fallback with a star in it) would turn a
//! verbatim write into a pattern entry, so `Approvals::approve_commands`
//! refuses it; pattern entries are hand-written, with no exception.

use std::collections::BTreeMap;

/// Whether `pattern` matches `identifier`, treating `*` as any run of
/// characters and every other character as a literal.
///
/// A hand-rolled glob rather than a compiled regex because accessors call
/// this per pattern key per invocation — `wt list` reaches it a few hundred
/// times — and regex compilation there measured ~0.7 ms per pattern key per
/// command.
pub(crate) fn matches(pattern: &str, identifier: &str) -> bool {
    // Greedy two-pointer glob: `*` first matches empty, then extends one byte
    // on each later mismatch. Only the most recent star ever needs revisiting,
    // because extending an earlier star can't let the identifier end any
    // sooner. Bytes are UTF-8-safe here: `*` never occurs inside a multibyte
    // sequence, and literal runs compare byte-for-byte. Every other byte —
    // `/` and newline included — is matchable, so the path-shaped identifier
    // a remoteless repository falls back to works on every platform that
    // permits one in a directory name.
    let (pattern, identifier) = (pattern.as_bytes(), identifier.as_bytes());
    let (mut p, mut i) = (0, 0);
    let mut retry: Option<(usize, usize)> = None;
    while i < identifier.len() {
        if pattern.get(p) == Some(&b'*') {
            retry = Some((p, i));
            p += 1;
        } else if pattern.get(p) == Some(&identifier[i]) {
            p += 1;
            i += 1;
        } else if let Some((star, matched)) = retry {
            p = star + 1;
            i = matched + 1;
            retry = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|b| *b == b'*')
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

    // The bool ranks a literal key above any pattern it ties with on literal
    // count (one whose stars all matched empty).
    let mut matched: Vec<(usize, bool, &str, &T)> = projects
        .iter()
        .filter(|(key, _)| matches(key, identifier))
        .map(|(key, value)| (literal_len(key), !key.contains('*'), key.as_str(), value))
        .collect();
    matched.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    matched.into_iter().map(|(_, _, _, value)| value).collect()
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
    fn test_multibyte_literals_and_identifiers() {
        // The glob walks bytes; multibyte literals still compare whole.
        assert!(matches("git.über.example/*", "git.über.example/owner/repo"));
        assert!(!matches(
            "git.über.example/*",
            "git.uber.example/owner/repo"
        ));
        assert!(matches("*/naïve", "github.com/owner/naïve"));
    }

    #[test]
    fn test_star_in_the_identifier() {
        // An identifier containing `*` is matched by a wildcard like any other
        // byte, and a starred key still covers the identical starred
        // identifier (its own star matches the literal `*`).
        assert!(matches("git.example/*", "git.example/owner/re*po"));
        assert!(matches(
            "git.example/owner/re*po",
            "git.example/owner/re*po"
        ));
        assert!(!matches(
            "git.example/owner/re*po",
            "git.example/other/repo"
        ));
    }

    /// Obviously-correct exponential reference for the differential test:
    /// a star matches empty or absorbs one more byte.
    fn matches_ref(pattern: &[u8], identifier: &[u8]) -> bool {
        match pattern.split_first() {
            None => identifier.is_empty(),
            Some((b'*', rest)) => {
                matches_ref(rest, identifier)
                    || !identifier.is_empty() && matches_ref(pattern, &identifier[1..])
            }
            Some((byte, rest)) => match identifier.split_first() {
                Some((first, tail)) => first == byte && matches_ref(rest, tail),
                None => false,
            },
        }
    }

    #[test]
    fn test_differential_against_reference_matcher() {
        // Exhaustive over every pattern (≤5 chars of `a`/`b`/`*`) against
        // every identifier (≤6 of the same), pinning the backtracking glob to
        // the spec wholesale — multiple stars, star-in-identifier, and every
        // truncation case included. `/` needs no seat: the matcher treats all
        // non-`*` bytes alike.
        fn strings(alphabet: &[u8], max_len: usize) -> Vec<Vec<u8>> {
            let mut all: Vec<Vec<u8>> = vec![Vec::new()];
            let mut last = all.clone();
            for _ in 0..max_len {
                last = last
                    .iter()
                    .flat_map(|s| {
                        alphabet.iter().map(move |c| {
                            let mut s = s.clone();
                            s.push(*c);
                            s
                        })
                    })
                    .collect();
                all.extend(last.iter().cloned());
            }
            all
        }

        for pattern in strings(b"ab*", 5) {
            let pattern = String::from_utf8(pattern).unwrap();
            for identifier in strings(b"ab*", 6) {
                let identifier = String::from_utf8(identifier).unwrap();
                assert_eq!(
                    matches(&pattern, &identifier),
                    matches_ref(pattern.as_bytes(), identifier.as_bytes()),
                    "pattern {pattern:?} vs identifier {identifier:?}"
                );
            }
        }
    }

    #[test]
    fn test_literal_key_outranks_a_pattern_that_ties() {
        // `github.com/owner/repo*` matches the identifier with its star
        // matching empty, tying the exact key at 21 literal characters — and
        // sorting after it lexicographically. Specificity alone would let the
        // pattern win the fold; the literal-over-pattern rank keeps the exact
        // entry on top.
        let projects = BTreeMap::from([
            ("github.com/owner/repo".to_string(), "exact"),
            ("github.com/owner/repo*".to_string(), "pattern"),
        ]);
        assert_eq!(
            matching_keys(&projects, "github.com/owner/repo"),
            vec![&"pattern", &"exact"]
        );
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
