//! Nested schema-unknown analysis for worktrunk config files.
//!
//! A single round-trip through a [`WorktrunkConfig`] type answers both
//! load-time questions ("which keys does serde silently drop?") and save-time
//! questions ("which keys must survive the diff-based merge?"). Reserializing
//! the parsed config and diffing against the raw TOML identifies every
//! schema-unknown path at any nesting depth.
//!
//! The same tree drives:
//! - Unknown-key warnings (`warn_unknown_fields`, `config show`) — emits one
//!   message at the shallowest level where a path is unknown.
//! - Save-path preservation (`UserConfig::save_to`) — prevents the merge from
//!   dropping hand-edited or forward-compat fields.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::WorktrunkConfig;

/// A nested set of schema-unknown paths within a config file.
///
/// `keys` holds unknown keys at the current level. Entries in `nested` are for
/// keys that are themselves *known* but contain unknown children. A key may
/// appear in both when the entire subtree is unknown — `keys` captures the top
/// of the unknown subtree, `nested` mirrors it so save-path merges still
/// preserve individual descendants if a mutation later introduces the table.
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

/// Outcome of analyzing a config file for schema-unknown paths.
///
/// The `Unreliable` variant covers both syntax errors and type mismatches
/// (e.g., a hand edit like `commit = "scalar"`). In those cases we can't tell
/// schema-unknown paths from schema-known-but-wrong-type ones, so:
/// - Save paths must preserve every on-disk key (the tree marks everything).
/// - Warning paths must stay silent (the parse/type error is surfaced by the
///   regular load path with accurate line/column info).
#[derive(Debug)]
pub enum UnknownAnalysis {
    /// `try_into<C>` succeeded; the tree lists schema-unknown paths only.
    Parsed(UnknownTree),
    /// Raw TOML was unparsable or failed type-checking against `C`. The tree
    /// still marks every on-disk key so save-path merges preserve data.
    Unreliable(UnknownTree),
}

impl UnknownAnalysis {
    /// Tree suitable for the save-path merge (preserves unknowns, and
    /// preserves everything on unreliable input).
    pub fn preserve_tree(&self) -> &UnknownTree {
        match self {
            Self::Parsed(t) | Self::Unreliable(t) => t,
        }
    }

    /// Tree suitable for unknown-key warnings (empty on unreliable input).
    pub fn warn_tree(&self) -> Option<&UnknownTree> {
        match self {
            Self::Parsed(t) => Some(t),
            Self::Unreliable(_) => None,
        }
    }
}

/// Analyze `contents` against config type `C` by round-tripping through serde.
///
/// On success, the returned tree captures every path in `contents` that
/// reserialization drops — i.e., every schema-unknown path. Top-level keys
/// that serialize away when empty (e.g., `[merge]` with only unknown children
/// leaves `MergeConfig::default()`, which `skip_serializing_if` omits) are
/// rescued by seeding the comparison with the JsonSchema key list: a known
/// section that isn't in the reserialized form is treated as present-but-empty
/// so only its unknown *children* get flagged, not the section itself.
pub fn compute_unknown_tree<C>(contents: &str) -> UnknownAnalysis
where
    C: WorktrunkConfig,
{
    let Ok(raw) = contents.parse::<toml::Table>() else {
        return UnknownAnalysis::Unreliable(UnknownTree::default());
    };

    let parsed: Result<C, _> = toml::Value::Table(raw.clone()).try_into();
    let Ok(config) = parsed else {
        return UnknownAnalysis::Unreliable(diff_tables(&raw, &toml::Table::new()));
    };

    let mut reserialized: toml::Table = toml::to_string(&config)
        .expect("config type is serializable")
        .parse()
        .expect("serialized config is valid TOML");
    seed_schema_skeleton::<C>(&mut reserialized);
    UnknownAnalysis::Parsed(diff_tables(&raw, &reserialized))
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
            (None, toml::Value::Table(raw_t)) => {
                // Whole subtree is schema-unknown. Mark the key at this level
                // and recurse so the preserve set is populated if a later
                // mutation causes `desired` to introduce this table.
                tree.keys.insert(key.clone());
                let nested = diff_tables(raw_t, &toml::Table::new());
                if !nested.is_empty() {
                    tree.nested.insert(key.clone(), nested);
                }
            }
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
/// (see the private `OtherStatus` helper). If that other-config analysis is
/// unreliable, the walk falls back to treating nested keys as unknown, so only
/// the hard-coded
/// [`nested_key_belongs_in`](crate::config::nested_key_belongs_in) redirects
/// still fire.
///
/// Returns an empty vec if either analysis is unreliable — the load path
/// surfaces parse/type errors elsewhere.
pub fn collect_unknown_warnings<C: WorktrunkConfig>(raw_contents: &str) -> Vec<UnknownWarning> {
    let raw_tree = match compute_unknown_tree::<C>(raw_contents) {
        UnknownAnalysis::Parsed(t) => t,
        UnknownAnalysis::Unreliable(_) => return Vec::new(),
    };
    let migrated = crate::config::migrate_content(raw_contents);
    let migrated_tree = match compute_unknown_tree::<C>(&migrated) {
        UnknownAnalysis::Parsed(t) => t,
        UnknownAnalysis::Unreliable(_) => return Vec::new(),
    };
    // The same content viewed as the *other* config type: a nested key absent
    // from this tree is valid there. Unreliable → no generalized redirect.
    let other_analysis = compute_unknown_tree::<C::Other>(&migrated);
    let other_root = match other_analysis.warn_tree() {
        Some(t) => OtherStatus::Known(Some(t)),
        None => OtherStatus::UnknownSection,
    };

    let mut out = Vec::new();
    for key in &raw_tree.keys {
        use crate::config::UnknownKeyKind;
        let warning = match crate::config::classify_unknown_key::<C>(key) {
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

fn walk_nested<C: WorktrunkConfig>(
    tree: &UnknownTree,
    prefix: &str,
    other: OtherStatus,
    out: &mut Vec<UnknownWarning>,
) {
    for key in &tree.keys {
        let path = format!("{prefix}.{key}");
        // Hard-coded redirects (commit.generation leaves) take precedence and
        // fire even when the other-config analysis is unreliable; otherwise
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
        if tree.keys.contains(key) {
            continue;
        }
        let path = format!("{prefix}.{key}");
        walk_nested::<C>(sub, &path, other.descend(key), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, UserConfig};

    fn parsed<C: WorktrunkConfig>(contents: &str) -> UnknownTree {
        match compute_unknown_tree::<C>(contents) {
            UnknownAnalysis::Parsed(t) => t,
            UnknownAnalysis::Unreliable(_) => panic!("expected Parsed"),
        }
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
    fn syntax_error_yields_unreliable() {
        let analysis = compute_unknown_tree::<UserConfig>("not valid {{{");
        assert!(matches!(analysis, UnknownAnalysis::Unreliable(_)));
        assert!(analysis.warn_tree().is_none());
    }

    #[test]
    fn type_mismatch_yields_unreliable_but_preserves_all() {
        // A hand-edit like `commit = "scalar"` can't round-trip through
        // UserConfig. The warn tree must be empty (parse error is surfaced
        // elsewhere) but the preserve tree must mark every on-disk key so
        // save_to doesn't drop data.
        let analysis = compute_unknown_tree::<UserConfig>(
            r#"
commit = "scalar"
skip-shell-integration-prompt = true
"#,
        );
        assert!(matches!(analysis, UnknownAnalysis::Unreliable(_)));
        assert!(analysis.warn_tree().is_none());
        let preserve = analysis.preserve_tree();
        assert!(preserve.keys.contains("commit"));
        assert!(preserve.keys.contains("skip-shell-integration-prompt"));
    }
}
