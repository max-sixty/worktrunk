//! Nested schema-unknown analysis for worktrunk config files.
//!
//! A round-trip through a [`WorktrunkConfig`] type answers "which keys does
//! serde silently drop?": reserializing the parsed config and diffing against
//! the raw TOML identifies every schema-unknown path at any nesting depth.
//!
//! The tree drives unknown-key warnings (`warn_unknown_fields`, `config show`),
//! which emit one message at the shallowest level where a path is unknown.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::WorktrunkConfig;

/// A nested set of schema-unknown paths within a config file.
///
/// `keys` holds unknown keys at the current level, each covering its whole
/// subtree. Entries in `nested` are for keys that are themselves *known* but
/// contain unknown children.
#[derive(Default, Debug, Clone)]
pub struct UnknownTree {
    pub keys: BTreeSet<String>,
    pub nested: BTreeMap<String, UnknownTree>,
}

impl UnknownTree {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.nested.is_empty()
    }
}

/// Analyze `contents` against config type `C` by round-tripping through serde.
///
/// `None` when `contents` is unparsable or fails type-checking against `C`
/// (e.g., a hand edit like `commit = "scalar"`): schema-unknown paths can't be
/// told from schema-known-but-wrong-type ones, so warning callers stay silent
/// and leave the error to the regular load path, which reports it with line
/// and column.
///
/// Otherwise the returned tree captures every path in `contents` that
/// reserialization drops — i.e., every schema-unknown path. Top-level keys
/// that serialize away when empty (e.g., `[merge]` with only unknown children
/// leaves `MergeConfig::default()`, which `skip_serializing_if` omits) are
/// rescued by seeding the comparison with the JsonSchema key list: a known
/// section that isn't in the reserialized form is treated as present-but-empty
/// so only its unknown *children* get flagged, not the section itself.
pub fn compute_unknown_tree<C>(contents: &str) -> Option<UnknownTree>
where
    C: WorktrunkConfig,
{
    let raw = contents.parse::<toml::Table>().ok()?;
    let config: C = toml::Value::Table(raw.clone()).try_into().ok()?;

    let mut reserialized: toml::Table = toml::to_string(&config)
        .expect("config type is serializable")
        .parse()
        .expect("serialized config is valid TOML");
    seed_schema_skeleton::<C>(&mut reserialized);
    Some(diff_tables(&raw, &reserialized))
}

/// Seed `reserialized` with every schema-valid top-level key as an empty
/// table so `diff_tables` treats valid-but-omitted sections as known.
fn seed_schema_skeleton<C: WorktrunkConfig>(reserialized: &mut toml::Table) {
    for key in C::valid_top_level_keys() {
        reserialized
            .entry(key.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    }
}

/// Walk `raw` against `known` (the schema-projected view) and record keys
/// that exist only in `raw`. Recurses into nested tables so deeply-nested
/// unknown keys are captured at the right level.
fn diff_tables(raw: &toml::Table, known: &toml::Table) -> UnknownTree {
    let mut tree = UnknownTree::default();
    for (key, raw_val) in raw {
        match (known.get(key), raw_val) {
            (Some(toml::Value::Table(known_t)), toml::Value::Table(raw_t)) => {
                let nested = diff_tables(raw_t, known_t);
                if !nested.is_empty() {
                    tree.nested.insert(key.clone(), nested);
                }
            }
            (Some(_), _) => {}
            (None, _) => {
                tree.keys.insert(key.clone());
            }
        }
    }
    tree
}

/// Structured description of a single unknown-key finding. Callers format
/// these into warning strings — the `deprecation` and `config show` paths
/// use different wording, so classification stays here and presentation
/// stays at the call site.
#[derive(Debug)]
pub enum UnknownWarning {
    /// A top-level key that's not in any schema. Fully unknown.
    TopLevelUnknown { key: String },
    /// A top-level key that's valid in the *other* config type (e.g.,
    /// `forge` appearing in user config).
    TopLevelWrongConfig {
        key: String,
        other_description: &'static str,
    },
    /// A top-level key that's deprecated and whose canonical form belongs in
    /// the other config (e.g., `[commit-generation]` in project config).
    TopLevelDeprecatedWrongConfig {
        key: String,
        other_description: &'static str,
        canonical_display: &'static str,
    },
    /// A nested key, valid in the *other* config type, found below a
    /// schema-valid shared section (e.g. `commit.generation.command` in
    /// project config — the `[commit.generation]` section is valid there for
    /// `template-append`, but the LLM command/templates belong in user
    /// config).
    NestedWrongConfig {
        path: String,
        other_description: &'static str,
    },
    /// An unknown path below a schema-valid top-level key — a typo, e.g.
    /// `merge.squas`.
    NestedUnknown { path: String },
}

/// Position within `C::Other`'s unknown tree while walking `C`'s nested
/// unknowns. A nested key that's schema-unknown in `C` but *valid* in
/// `C::Other` (e.g. `list.columns` — a user-config display setting — placed in
/// project config) should redirect there rather than read "unknown field". We
/// answer "is this leaf valid in the other config?" by walking the other
/// config's unknown tree in lockstep: a key valid there never appears in that
/// tree, so its absence is the signal.
#[derive(Clone, Copy)]
enum OtherStatus<'a> {
    /// An ancestor section is absent or wholly unknown in `C::Other`, so
    /// nothing at or below this point is valid there.
    UnknownSection,
    /// Within a section that exists in `C::Other`. Carries the corresponding
    /// node in the other tree, or `None` once no further unknowns are recorded
    /// — i.e. every key at or below here is valid in the other config.
    Known(Option<&'a UnknownTree>),
}

impl<'a> OtherStatus<'a> {
    /// Whether `key` at the current level is a valid key in `C::Other`.
    fn key_is_valid_in_other(self, key: &str) -> bool {
        match self {
            OtherStatus::UnknownSection => false,
            OtherStatus::Known(None) => true,
            OtherStatus::Known(Some(node)) => !node.keys.contains(key),
        }
    }

    /// Descend into `key`, returning the status for its children.
    fn descend(self, key: &str) -> OtherStatus<'a> {
        match self {
            OtherStatus::UnknownSection => OtherStatus::UnknownSection,
            OtherStatus::Known(None) => OtherStatus::Known(None),
            OtherStatus::Known(Some(node)) => {
                if node.keys.contains(key) {
                    // Whole subtree is unknown in the other config too.
                    OtherStatus::UnknownSection
                } else {
                    OtherStatus::Known(node.nested.get(key))
                }
            }
        }
    }
}

/// Collect structured warnings for `raw_contents` under config type `C`.
///
/// Top-level classification reads the *raw* tree (so deprecated top-level
/// sections surface informative messages like "belongs in user config as
/// `[commit.generation]`"). Nested classification reads the *migrated* tree,
/// so patterns the deprecation system already warns about (e.g.,
/// `switch.no-cd`, `merge.no-ff`) don't double-warn here.
///
/// A misplaced *nested* key is redirected to `C::Other` when it's valid there
/// — determined by walking `C::Other`'s own unknown tree for the same content
/// (see the private `OtherStatus` helper). If the content doesn't parse as
/// `C::Other`, the walk falls back to treating nested keys as unknown, so only
/// the hard-coded
/// [`nested_key_belongs_in`](crate::config::nested_key_belongs_in) redirects
/// still fire.
///
/// A deprecated top-level section is left to the deprecation channel only
/// when migration removed it; one that survived non-empty is reported here
/// (see the private `deprecated_key_declined` helper).
///
/// Returns an empty vec if the raw or migrated content doesn't parse as `C` —
/// the load path surfaces parse/type errors elsewhere.
pub fn collect_unknown_warnings<C: WorktrunkConfig>(raw_contents: &str) -> Vec<UnknownWarning> {
    let Some(raw_tree) = compute_unknown_tree::<C>(raw_contents) else {
        return Vec::new();
    };
    let migrated = crate::config::migrate_content(raw_contents);
    let Some(migrated_tree) = compute_unknown_tree::<C>(&migrated) else {
        return Vec::new();
    };
    // `compute_unknown_tree` returned a tree above, so this parse cannot
    // fail; the fallback just keeps the path panic-free.
    let migrated_root = migrated.parse::<toml::Table>().unwrap_or_default();
    // The same content viewed as the *other* config type: a nested key absent
    // from this tree is valid there. No tree → no generalized redirect.
    let other_tree = compute_unknown_tree::<C::Other>(&migrated);
    let other_root = match &other_tree {
        Some(t) => OtherStatus::Known(Some(t)),
        None => OtherStatus::UnknownSection,
    };

    let mut out = Vec::new();
    for key in &raw_tree.keys {
        use crate::config::UnknownKeyKind;
        let warning = match crate::config::classify_unknown_key::<C>(key) {
            UnknownKeyKind::DeprecatedHandled if deprecated_key_declined(&migrated_root, key) => {
                UnknownWarning::TopLevelUnknown { key: key.clone() }
            }
            UnknownKeyKind::DeprecatedHandled => continue,
            UnknownKeyKind::DeprecatedWrongConfig {
                other_description,
                canonical_display,
            } => UnknownWarning::TopLevelDeprecatedWrongConfig {
                key: key.clone(),
                other_description,
                canonical_display,
            },
            UnknownKeyKind::WrongConfig { other_description } => {
                UnknownWarning::TopLevelWrongConfig {
                    key: key.clone(),
                    other_description,
                }
            }
            UnknownKeyKind::Unknown => UnknownWarning::TopLevelUnknown { key: key.clone() },
        };
        out.push(warning);
    }
    for (key, sub) in &migrated_tree.nested {
        if !C::is_valid_key(key) {
            continue; // top-level unknowns were classified above against raw
        }
        walk_nested::<C>(sub, key, other_root.descend(key), &mut out);
    }
    out
}

/// Whether `key` — a registered deprecated section — still carries config
/// after migration.
///
/// [`classify_unknown_key`](crate::config::classify_unknown_key) answers from
/// the registry alone: the name is deprecated, so the deprecation channel owns
/// the message. That holds only when the rewrite actually ran. A migration
/// that declines — a malformed scalar (`select = "x"`), a destination already
/// occupied — deliberately leaves the key in place and emits nothing, and the
/// documented handoff is for the unknown-field message to speak instead. Left
/// suppressed, the setting is read by nobody and reported by no one.
///
/// An empty deprecated section is the one survivor that stays silent: it
/// contributes no config, which is why the migration leaves it alone in the
/// first place.
fn deprecated_key_declined(migrated: &toml::Table, key: &str) -> bool {
    match migrated.get(key) {
        Some(toml::Value::Table(t)) => !t.is_empty(),
        Some(_) => true,
        None => false,
    }
}

fn walk_nested<C: WorktrunkConfig>(
    tree: &UnknownTree,
    prefix: &str,
    other: OtherStatus,
    out: &mut Vec<UnknownWarning>,
) {
    for key in &tree.keys {
        let path = format!("{prefix}.{key}");
        // Hard-coded redirects (commit.generation leaves) take precedence and
        // fire even when the content doesn't parse as the other config; otherwise
        // fall back to the general "valid in the other config" check.
        let belongs = crate::config::nested_key_belongs_in::<C>(&path)
            .or_else(|| other.key_is_valid_in_other(key).then(C::Other::description));
        out.push(match belongs {
            Some(other_description) => UnknownWarning::NestedWrongConfig {
                path,
                other_description,
            },
            None => UnknownWarning::NestedUnknown { path },
        });
    }
    for (key, sub) in &tree.nested {
        let path = format!("{prefix}.{key}");
        walk_nested::<C>(sub, &path, other.descend(key), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, UserConfig};

    fn parsed<C: WorktrunkConfig>(contents: &str) -> UnknownTree {
        compute_unknown_tree::<C>(contents).expect("contents should parse as C")
    }

    #[test]
    fn empty_input_has_no_unknowns() {
        let tree = parsed::<UserConfig>("");
        assert!(tree.is_empty());
    }

    #[test]
    fn known_keys_are_not_flagged() {
        let tree = parsed::<UserConfig>(
            r#"
worktree-path = "../test"

[list]
full = true

[commit.generation]
command = "llm"
"#,
        );
        assert!(tree.is_empty(), "tree should be empty, got {tree:?}");
    }

    #[test]
    fn unknown_top_level_key() {
        let tree = parsed::<UserConfig>("unknown-key = \"value\"\n");
        assert!(tree.keys.contains("unknown-key"));
        assert!(tree.nested.is_empty());
    }

    #[test]
    fn nested_unknown_key_under_known_section() {
        let tree = parsed::<UserConfig>(
            r#"
[merge]
future-option = true
"#,
        );
        assert!(tree.keys.is_empty());
        let merge = tree.nested.get("merge").expect("merge subtree");
        assert!(merge.keys.contains("future-option"));
    }

    #[test]
    fn deeply_nested_unknown_key() {
        let tree = parsed::<UserConfig>(
            r#"
[commit.generation]
command = "llm"
future-knob = "x"
"#,
        );
        let commit = tree.nested.get("commit").expect("commit subtree");
        let generation = commit.nested.get("generation").expect("generation subtree");
        assert!(generation.keys.contains("future-knob"));
    }

    #[test]
    fn unknown_whole_subtree_is_marked_at_top_level() {
        // A wholly-unknown section records the key at its parent level,
        // which is what warning emitters want — one message for the whole
        // subtree, not one per descendant.
        let tree = parsed::<UserConfig>(
            r#"
[unknown-section]
a = 1
b = 2
"#,
        );
        assert!(tree.keys.contains("unknown-section"));
    }

    #[test]
    fn project_config_detects_user_only_key() {
        let tree = parsed::<ProjectConfig>("skip-shell-integration-prompt = true\n");
        assert!(tree.keys.contains("skip-shell-integration-prompt"));
    }

    #[test]
    fn syntax_error_yields_no_tree() {
        assert!(compute_unknown_tree::<UserConfig>("not valid {{{").is_none());
    }

    #[test]
    fn type_mismatch_yields_no_tree() {
        // A hand-edit like `commit = "scalar"` can't round-trip through
        // UserConfig; the parse error is surfaced elsewhere.
        let tree = compute_unknown_tree::<UserConfig>(
            r#"
commit = "scalar"
skip-shell-integration-prompt = true
"#,
        );
        assert!(tree.is_none());
    }
}
