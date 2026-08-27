//! Row ordering for the `wt list` table and the `wt switch` picker.
//!
//! `[list] sort` names columns, most significant first, each optionally
//! prefixed with `-` for descending. Only columns whose value is known before
//! the skeleton paints are accepted — exactly those with no background task
//! ([`ColumnKind::required_tasks`] empty). Sorting on a streamed column would
//! mean either holding the skeleton for data that may be a network round-trip
//! away (`ci`) or reordering rows mid-render, which the progressive table
//! doesn't do.
//!
//! An empty spec keeps the historical order: current worktree first, primary
//! worktree second, then newest commit first. A non-empty spec drops that
//! two-row prefix — pinning rows to the top would defeat the ordering the user
//! asked for — but newest-commit-first survives as the final tiebreak. So the
//! empty spec and the tiebreak are the same code path, and rows a spec can't
//! distinguish (every branch-only row under `sort = ["path"]`, since branch
//! rows have no path) keep the order they have today.
//!
//! Rows are ordered within their group, never across: worktrees, then
//! branch-only rows, then remote rows, as before. `sort` reorders each group.

use std::cmp::Ordering;
use std::path::Path;

use super::columns::{COLUMN_SPECS, ColumnKind};

/// A column `[list] sort` can order rows by.
///
/// The variants are exactly the built-ins that render without a background
/// task, so a new column must declare itself here or opt out in
/// [`SortKey::from_column`]'s exhaustive match.
/// `test_sortable_keys_are_exactly_the_task_free_columns` pins that
/// correspondence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Branch,
    Path,
    Commit,
    /// Time since the head commit — the `age` column, counting *up* from the
    /// committer date.
    Age,
    Message,
}

/// One `[list] sort` entry: a key and its direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortTerm {
    pub key: SortKey,
    pub descending: bool,
}

impl SortKey {
    /// The sort key a built-in column provides, or `None` when its value isn't
    /// known before the skeleton paints.
    pub fn from_column(kind: ColumnKind) -> Option<SortKey> {
        Some(match kind {
            ColumnKind::Branch => SortKey::Branch,
            ColumnKind::Path => SortKey::Path,
            ColumnKind::Commit => SortKey::Commit,
            ColumnKind::Time => SortKey::Age,
            ColumnKind::Message => SortKey::Message,
            // Streamed in after the skeleton, so unavailable when row order is
            // chosen. Gutter and custom columns have no `[list] sort` name to
            // reach them by (custom values expand after the sort, too).
            ColumnKind::Gutter
            | ColumnKind::Status
            | ColumnKind::WorkingDiff
            | ColumnKind::AheadBehind
            | ColumnKind::BranchDiff
            | ColumnKind::Summary
            | ColumnKind::Upstream
            | ColumnKind::CiStatus
            | ColumnKind::Url
            | ColumnKind::Custom(_) => return None,
        })
    }
}

/// Every sortable column name in display order, for error messages and docs.
pub fn sortable_names() -> Vec<&'static str> {
    COLUMN_SPECS
        .iter()
        .filter(|spec| SortKey::from_column(spec.kind).is_some())
        .filter_map(|spec| spec.kind.config_name())
        .collect()
}

/// Parse the `[list] sort` spec into ordered sort terms.
///
/// Each entry is a column's kebab [`ColumnKind::config_name`], optionally
/// prefixed with `-` for descending. An empty input yields an empty spec (the
/// caller reads that as "default order"). Unknown names, unsortable columns,
/// and duplicate keys are hard errors so a typo can't silently render a
/// different order; the error lists every sortable name. Validated at the
/// `wt list` edge for the same reason `[list] columns` is — `ColumnKind` lives
/// in the command layer, out of reach of the config crate.
pub fn parse_sort_spec(names: &[String]) -> anyhow::Result<Vec<SortTerm>> {
    let mut terms: Vec<SortTerm> = Vec::with_capacity(names.len());
    for name in names {
        let (descending, bare) = match name.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, name.as_str()),
        };
        let valid = || sortable_names().join(", ");
        let key = match ColumnKind::from_config_name(bare) {
            Some(kind) => SortKey::from_column(kind).ok_or_else(|| {
                anyhow::anyhow!(
                    "Column {bare:?} in [list] sort is not sortable — its value arrives after the table paints. Sortable columns: {}",
                    valid()
                )
            })?,
            None => anyhow::bail!(
                "Unknown sort key {name:?} in [list] sort. Sortable columns: {} (prefix with `-` for descending)",
                valid()
            ),
        };
        if terms.iter().any(|term| term.key == key) {
            anyhow::bail!("Duplicate sort key {name:?} in [list] sort");
        }
        terms.push(SortTerm { key, descending });
    }
    Ok(terms)
}

/// The per-row values a sort term reads, all known before the skeleton paints.
///
/// A missing value compares as empty: branch-only and remote rows carry no
/// path, and a detached worktree carries no branch. Rows only ever compare
/// within their own group, so the only mixed case is a detached worktree among
/// named ones, which sorts first under ascending `branch`.
pub struct SortFacts<'a> {
    pub path: Option<&'a Path>,
    pub branch: Option<&'a str>,
    pub short_sha: &'a str,
    pub timestamp: i64,
    pub message: &'a str,
}

impl<'a> SortFacts<'a> {
    /// Build a row's facts, reading the commit fields from the batched
    /// commit-details map keyed by `head`. A SHA the batch didn't cover (an
    /// unborn branch, a failed batch) compares as empty/epoch, exactly as its
    /// cells render.
    pub fn new(
        path: Option<&'a Path>,
        branch: Option<&'a str>,
        head: &str,
        commit_details: &'a std::collections::HashMap<String, (String, i64, String)>,
    ) -> Self {
        let detail = commit_details.get(head);
        Self {
            path,
            branch,
            short_sha: detail.map_or("", |(short, _, _)| short.as_str()),
            timestamp: detail.map_or(0, |(_, ts, _)| *ts),
            message: detail.map_or("", |(_, _, subject)| subject.as_str()),
        }
    }
}

/// Order two rows by `terms`, falling back to newest commit first.
///
/// With `terms` empty this *is* newest-first, which is why the default order
/// and the tiebreak need no separate code path.
pub fn compare(terms: &[SortTerm], a: &SortFacts<'_>, b: &SortFacts<'_>) -> Ordering {
    for term in terms {
        let ordering = match term.key {
            SortKey::Branch => a.branch.unwrap_or("").cmp(b.branch.unwrap_or("")),
            SortKey::Path => a
                .path
                .unwrap_or(Path::new(""))
                .cmp(b.path.unwrap_or(Path::new(""))),
            SortKey::Commit => a.short_sha.cmp(b.short_sha),
            // Age counts up from the committer date, so ascending age is
            // newest first — the same direction as the default order — and
            // `-age` puts the oldest commits on top.
            SortKey::Age => b.timestamp.cmp(&a.timestamp),
            SortKey::Message => a.message.cmp(b.message),
        };
        let ordering = if term.descending {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    b.timestamp.cmp(&a.timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn facts<'a>(
        path: Option<&'a str>,
        branch: Option<&'a str>,
        short_sha: &'a str,
        timestamp: i64,
        message: &'a str,
    ) -> SortFacts<'a> {
        SortFacts {
            path: path.map(Path::new),
            branch,
            short_sha,
            timestamp,
            message,
        }
    }

    #[test]
    fn test_sortable_keys_are_exactly_the_task_free_columns() {
        // The promise `[list] sort` makes is "skeleton-time columns only", and
        // a column is skeleton-time exactly when it consumes no background
        // task. Adding a task-free column without a `SortKey` arm (or giving a
        // streamed column one) breaks that promise silently; this catches it.
        for spec in COLUMN_SPECS {
            let kind = spec.kind;
            let sortable = SortKey::from_column(kind).is_some();
            let skeleton_time = kind.required_tasks().is_empty() && kind.config_name().is_some();
            assert_eq!(
                sortable, skeleton_time,
                "{kind:?}: sortable={sortable} but skeleton-time={skeleton_time}"
            );
        }
        // Custom columns expand after the sort and are addressed by header, so
        // they are unreachable from a sort spec.
        assert_eq!(SortKey::from_column(ColumnKind::Custom(0)), None);
    }

    #[test]
    fn test_parse_sort_spec() {
        let terms = parse_sort_spec(&["path".into(), "-age".into()]).unwrap();
        assert_eq!(
            terms,
            vec![
                SortTerm {
                    key: SortKey::Path,
                    descending: false
                },
                SortTerm {
                    key: SortKey::Age,
                    descending: true
                },
            ],
            "terms keep configured order; `-` marks descending"
        );

        assert!(parse_sort_spec(&[]).unwrap().is_empty());

        // A streamed column names a real column but can't order the skeleton.
        let streamed = parse_sort_spec(&["ci".into()]).unwrap_err().to_string();
        assert!(streamed.contains("not sortable"), "{streamed}");
        assert!(streamed.contains("path"), "lists valid keys: {streamed}");

        // A name that isn't a column at all is a different error, and still
        // lists the valid keys.
        let unknown = parse_sort_spec(&["bogus".into()]).unwrap_err().to_string();
        assert!(unknown.contains("Unknown sort key"), "{unknown}");
        assert!(unknown.contains("bogus"), "{unknown}");
        assert!(unknown.contains("branch"), "{unknown}");

        // The `-` prefix is stripped before the name is resolved, so a bad name
        // reports with its prefix intact rather than as a mystery.
        let prefixed = parse_sort_spec(&["-nope".into()]).unwrap_err().to_string();
        assert!(prefixed.contains("\"-nope\""), "{prefixed}");

        // The same key twice is a contradiction, whichever direction each carries.
        let dup = parse_sort_spec(&["age".into(), "-age".into()])
            .unwrap_err()
            .to_string();
        assert!(dup.contains("Duplicate sort key"), "{dup}");

        // Gutter has no name, and matching is exact.
        assert!(parse_sort_spec(&["gutter".into()]).is_err());
        assert!(parse_sort_spec(&["Path".into()]).is_err());
    }

    #[test]
    fn test_compare_defaults_to_newest_first() {
        let older = facts(Some("/a"), Some("a"), "aaa", 100, "a");
        let newer = facts(Some("/b"), Some("b"), "bbb", 200, "b");
        assert_eq!(compare(&[], &newer, &older), Ordering::Less);
        assert_eq!(compare(&[], &older, &newer), Ordering::Greater);
    }

    #[test]
    fn test_compare_orders_by_terms_then_falls_back() {
        let path_asc = parse_sort_spec(&["path".into()]).unwrap();
        // `/a` sorts before `/b` even though `/b` is newer.
        let a = facts(Some("/a"), Some("z"), "aaa", 100, "z");
        let b = facts(Some("/b"), Some("a"), "bbb", 200, "a");
        assert_eq!(compare(&path_asc, &a, &b), Ordering::Less);

        // Descending flips it.
        let path_desc = parse_sort_spec(&["-path".into()]).unwrap();
        assert_eq!(compare(&path_desc, &a, &b), Ordering::Greater);

        // Rows the spec can't tell apart (branch rows have no path) fall back to
        // newest first, so they keep the order they have without a spec.
        let no_path_old = facts(None, Some("old"), "ccc", 100, "c");
        let no_path_new = facts(None, Some("new"), "ddd", 200, "d");
        assert_eq!(
            compare(&path_asc, &no_path_new, &no_path_old),
            Ordering::Less
        );
    }

    #[test]
    fn test_compare_age_direction() {
        let old = facts(Some("/a"), Some("a"), "aaa", 100, "a");
        let new = facts(Some("/b"), Some("b"), "bbb", 200, "b");
        // Ascending age = smallest age = newest commit, matching the default.
        let age_asc = parse_sort_spec(&["age".into()]).unwrap();
        assert_eq!(compare(&age_asc, &new, &old), Ordering::Less);
        // Descending age = oldest commit first.
        let age_desc = parse_sort_spec(&["-age".into()]).unwrap();
        assert_eq!(compare(&age_desc, &old, &new), Ordering::Less);
    }

    #[test]
    fn test_compare_uses_later_terms_to_break_ties() {
        let terms = parse_sort_spec(&["branch".into(), "message".into()]).unwrap();
        let a = facts(Some("/a"), Some("same"), "aaa", 200, "aardvark");
        let b = facts(Some("/b"), Some("same"), "bbb", 100, "zebra");
        assert_eq!(
            compare(&terms, &a, &b),
            Ordering::Less,
            "equal branches fall through to the message term, not to the date"
        );
    }

    #[test]
    fn test_compare_by_commit() {
        // The abbreviated SHA orders lexicographically. Not a meaningful
        // ranking, but it groups a repeated commit together, and every
        // skeleton-time column is offered rather than a hand-picked subset.
        let terms = parse_sort_spec(&["commit".into()]).unwrap();
        let a = facts(Some("/a"), Some("a"), "abc1234", 100, "a");
        let b = facts(Some("/b"), Some("b"), "def5678", 200, "b");
        assert_eq!(compare(&terms, &a, &b), Ordering::Less);
        assert_eq!(compare(&terms, &b, &a), Ordering::Greater);
    }

    #[test]
    fn test_facts_from_commit_details() {
        let mut details = HashMap::new();
        details.insert(
            "full-sha".to_string(),
            ("abc1234".to_string(), 42_i64, "subject".to_string()),
        );
        let known = SortFacts::new(None, Some("br"), "full-sha", &details);
        assert_eq!(known.short_sha, "abc1234");
        assert_eq!(known.timestamp, 42);
        assert_eq!(known.message, "subject");

        // A SHA the batch didn't cover compares as empty/epoch, mirroring the
        // placeholder cells its row renders.
        let missing = SortFacts::new(None, Some("br"), "unknown", &details);
        assert_eq!(missing.short_sha, "");
        assert_eq!(missing.timestamp, 0);
        assert_eq!(missing.message, "");
    }
}
