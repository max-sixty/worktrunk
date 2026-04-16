//! Config persistence - loading and saving to disk.
//!
//! Handles TOML serialization with formatting (multiline arrays, implicit tables)
//! and preserves comments when updating existing files via diff-based merge.
//!
//! The existing-file save path works by diffing the serialized in-memory state
//! against the parsed file and merging only changed keys. This automatically
//! handles any new fields without manual wiring — if a struct field is
//! serializable, save_to persists it.

use crate::config::{ConfigError, UnknownTree, compute_unknown_tree};

use super::UserConfig;
use super::sections::CommitGenerationConfig;

impl UserConfig {
    /// Recursively convert inline tables to standard tables for readability.
    ///
    /// When using `toml_edit::ser::to_document()`, nested structs are serialized as inline tables
    /// (e.g., `commit = { generation = { command = "..." } }`). This converts them to standard
    /// multi-line tables for better human readability.
    fn expand_inline_tables(table: &mut toml_edit::Table) {
        let keys: Vec<_> = table.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            let item = table.get_mut(&key).unwrap();
            if let Some(inline) = item.as_inline_table() {
                let mut new_table = inline.clone().into_table();
                Self::expand_inline_tables(&mut new_table);
                *item = toml_edit::Item::Table(new_table);
            }
        }
    }

    /// If `[commit]` only contains subtables (like `[commit.generation]`), mark it implicit
    /// so TOML doesn't emit an empty `[commit]` header.
    fn make_commit_table_implicit_if_only_subtables(doc: &mut toml_edit::DocumentMut) {
        if let Some(commit) = doc.get_mut("commit").and_then(|c| c.as_table_mut()) {
            let has_only_subtables = commit.iter().all(|(_, v)| v.is_table());
            if has_only_subtables {
                commit.set_implicit(true);
            }
        }
    }

    /// Recursively merge desired state into existing document.
    ///
    /// - Keys in desired but not existing: inserted
    /// - Keys in existing but not desired: removed (unless in `preserve`)
    /// - Both standard tables: recurse (preserves existing formatting and comments)
    /// - Existing inline table, desired standard table: compare contents, preserve
    ///   inline format when semantically equal
    /// - Both exist, values differ: update existing to desired
    /// - Both exist, values equal: leave existing unchanged (preserves comments)
    fn merge_tables(
        existing: &mut toml_edit::Table,
        desired: &toml_edit::Table,
        preserve: &UnknownTree,
    ) {
        let stale_keys: Vec<_> = existing
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| !desired.contains_key(k) && !preserve.keys.contains(k))
            .collect();
        for key in &stale_keys {
            existing.remove(key);
        }

        let empty_tree = UnknownTree::default();
        for (key, desired_item) in desired.iter() {
            match existing.get_mut(key) {
                // Both standard tables: recurse
                Some(existing_item) if existing_item.is_table() && desired_item.is_table() => {
                    let nested_preserve = preserve.nested.get(key).unwrap_or(&empty_tree);
                    Self::merge_tables(
                        existing_item.as_table_mut().unwrap(),
                        desired_item.as_table().unwrap(),
                        nested_preserve,
                    );
                }
                // Existing inline table, desired standard table: compare contents
                // to preserve the user's inline formatting when nothing changed
                Some(existing_item)
                    if existing_item.is_inline_table() && desired_item.is_table() =>
                {
                    let as_table = existing_item
                        .as_inline_table()
                        .unwrap()
                        .clone()
                        .into_table();
                    if !Self::tables_equal(&as_table, desired_item.as_table().unwrap()) {
                        *existing_item = desired_item.clone();
                    }
                }
                Some(existing_item) => {
                    if !Self::items_equal(existing_item, desired_item) {
                        *existing_item = desired_item.clone();
                    }
                }
                None => {
                    existing[key] = desired_item.clone();
                }
            }
        }
    }

    /// Compare two Items for value equality, ignoring formatting and comments.
    fn items_equal(a: &toml_edit::Item, b: &toml_edit::Item) -> bool {
        match (a, b) {
            (toml_edit::Item::Value(va), toml_edit::Item::Value(vb)) => Self::values_equal(va, vb),
            (toml_edit::Item::Table(ta), toml_edit::Item::Table(tb)) => Self::tables_equal(ta, tb),
            _ => false,
        }
    }

    fn values_equal(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
        use toml_edit::Value;
        match (a, b) {
            (Value::String(a), Value::String(b)) => a.value() == b.value(),
            (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
            (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(a, b)| Self::values_equal(a, b))
            }
            _ => false,
        }
    }

    fn tables_equal(a: &toml_edit::Table, b: &toml_edit::Table) -> bool {
        a.len() == b.len()
            && a.iter()
                .all(|(k, v)| b.get(k).is_some_and(|bv| Self::items_equal(v, bv)))
    }

    /// Save the current configuration to a specific file path.
    ///
    /// Preserves comments and formatting in the existing file by diffing the
    /// serialized in-memory state against the parsed file and merging only
    /// changed keys. Schema-unknown keys at any nesting level (typos, fields
    /// from newer wt versions) are preserved so older wt versions don't
    /// silently strip forward-compatible config data.
    pub fn save_to(&self, config_path: &std::path::Path) -> Result<(), ConfigError> {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError(format!("Failed to create config directory: {}", e)))?;
        }

        let toml_string = if config_path.exists() {
            let existing_content = std::fs::read_to_string(config_path)
                .map_err(|e| ConfigError(format!("Failed to read config file: {}", e)))?;

            let mut existing_doc: toml_edit::DocumentMut = existing_content
                .parse()
                .map_err(|e| ConfigError(format!("Failed to parse config file: {}", e)))?;

            let mut desired_doc = toml_edit::ser::to_document(&self)
                .map_err(|e| ConfigError(format!("Serialization error: {e}")))?;
            Self::expand_inline_tables(desired_doc.as_table_mut());

            // Preserve unknown keys at every nesting level (typos, future
            // fields, deprecated keys not yet migrated) so they aren't
            // silently deleted on save. On type-mismatch we still preserve
            // every on-disk key — it's safer to round-trip the whole file
            // than to drop fields we can't interpret.
            let analysis = compute_unknown_tree::<UserConfig>(&existing_content);
            let preserve = analysis.preserve_tree();

            Self::merge_tables(
                existing_doc.as_table_mut(),
                desired_doc.as_table(),
                preserve,
            );
            Self::make_commit_table_implicit_if_only_subtables(&mut existing_doc);

            existing_doc.to_string()
        } else {
            let mut doc = toml_edit::ser::to_document(&self)
                .map_err(|e| ConfigError(format!("Serialization error: {e}")))?;

            Self::expand_inline_tables(doc.as_table_mut());
            Self::make_commit_table_implicit_if_only_subtables(&mut doc);

            if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
                projects.set_implicit(true);
            }

            doc.to_string()
        };

        std::fs::write(config_path, toml_string)
            .map_err(|e| ConfigError(format!("Failed to write config file: {}", e)))?;

        Ok(())
    }
}

// =========================================================================
// Validation
// =========================================================================

impl UserConfig {
    /// Validate configuration values.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        // Validate worktree path (only if explicitly set - default is always valid)
        if let Some(ref path) = self.worktree_path
            && path.trim().is_empty()
        {
            return Err(ConfigError("worktree-path cannot be empty".into()));
        }

        // Validate per-project configs
        for (project, project_config) in &self.projects {
            // Validate worktree path
            if let Some(ref path) = project_config.worktree_path
                && path.trim().is_empty()
            {
                return Err(ConfigError(format!(
                    "projects.{project}.worktree-path cannot be empty"
                )));
            }

            if let Some(ref cg) = project_config.commit.generation {
                Self::validate_commit_generation(cg, &format!("projects.{project}"))?;
            }
        }

        if let Some(ref cg) = self.commit.generation {
            if cg.template.is_some() && cg.template_file.is_some() {
                return Err(ConfigError(
                    "commit.generation.template and commit.generation.template-file are mutually exclusive".into(),
                ));
            }

            if cg.squash_template.is_some() && cg.squash_template_file.is_some() {
                return Err(ConfigError(
                    "commit.generation.squash-template and commit.generation.squash-template-file are mutually exclusive".into(),
                ));
            }
        }

        Ok(())
    }

    fn validate_commit_generation(
        cg: &CommitGenerationConfig,
        prefix: &str,
    ) -> Result<(), ConfigError> {
        if cg.template.is_some() && cg.template_file.is_some() {
            return Err(ConfigError(format!(
                "{prefix}.commit-generation.template and template-file are mutually exclusive"
            )));
        }
        if cg.squash_template.is_some() && cg.squash_template_file.is_some() {
            return Err(ConfigError(format!(
                "{prefix}.commit-generation.squash-template and squash-template-file are mutually exclusive"
            )));
        }
        Ok(())
    }
}
