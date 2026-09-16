//! Config persistence - loading and saving to disk.
//!
//! Handles TOML serialization with formatting (multiline arrays, implicit tables)
//! and preserves comments when updating existing files via diff-based merge.
//!
//! The existing-file save path diffs the serialized in-memory state against the
//! file's own config, serialized the same way, and applies only that difference
//! to the parsed file (see `merge_tables`). This automatically handles any new
//! fields without manual wiring — if a struct field is serializable, save_to
//! persists it.

use std::borrow::Cow;

use crate::config::ConfigError;

use super::UserConfig;

impl UserConfig {
    /// Serialize to a document whose nested structs are standard tables.
    ///
    /// `projects` is implicit, so an empty map writes no bare `[projects]`
    /// header and a non-empty one writes only its `[projects."…"]` entries.
    fn to_expanded_document(&self) -> Result<toml_edit::DocumentMut, ConfigError> {
        let mut doc = toml_edit::ser::to_document(self)
            .map_err(|e| ConfigError(format!("Serialization error: {e}")))?;
        Self::expand_inline_tables(doc.as_table_mut());
        if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
            projects.set_implicit(true);
        }
        Ok(doc)
    }

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
    /// `base` is the file's own config, serialized the same way as `desired`,
    /// which is that config after the change being saved (`None` where it has
    /// no table at this path). A key only `base` has is one the change reset.
    /// A key neither has serializes to nothing — a value written out at its
    /// default, an empty section — or is one serde doesn't know (a typo, a
    /// field from a newer version); the change didn't touch it, so it stays.
    ///
    /// - Keys in desired but not existing: inserted
    /// - Keys in existing and base but not desired: removed
    /// - Both standard tables: recurse (preserves existing formatting and comments)
    /// - Existing inline table, desired standard table: merge through the same
    ///   recursive path, so the nested `base` applies; the inline format is kept
    ///   when that merge changed nothing
    /// - A pipeline: compared in the spelling the file uses (see
    ///   `in_existing_spelling`), as is its `base`; `[[…]]` blocks merge block by
    ///   block
    /// - Both exist, values differ: update existing to desired
    /// - Both exist, values equal: leave existing unchanged (preserves comments)
    fn merge_tables(
        existing: &mut toml_edit::Table,
        desired: &toml_edit::Table,
        base: Option<&toml_edit::Table>,
    ) {
        let stale_keys: Vec<_> = existing
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| !desired.contains_key(k) && base.is_some_and(|b| b.contains_key(k)))
            .collect();
        for key in &stale_keys {
            existing.remove(key);
        }

        for (key, desired_item) in desired.iter() {
            let desired_item = &*Self::in_existing_spelling(existing.get(key), desired_item);
            let base_item = base
                .and_then(|b| b.get(key))
                .map(|item| Self::in_existing_spelling(existing.get(key), item));
            let nested_base = base_item.as_deref().and_then(toml_edit::Item::as_table);

            // Existing inline table, desired standard table: merge into a table
            // view so the same nested preservation applies as in the
            // standard-table branch, then write back only if that changed
            // something — an untouched inline table keeps its formatting.
            if desired_item.is_table()
                && let Some(as_table) = existing
                    .get(key)
                    .and_then(|item| item.as_inline_table())
                    .map(|inline| inline.clone().into_table())
            {
                let mut merged = as_table.clone();
                Self::merge_tables(&mut merged, desired_item.as_table().unwrap(), nested_base);
                if !Self::tables_equal(&as_table, &merged) {
                    crate::config::replace_inline_with_table(existing, key, merged);
                }
                continue;
            }

            match existing.get_mut(key) {
                // Both standard tables: recurse
                Some(existing_item) if existing_item.is_table() && desired_item.is_table() => {
                    Self::merge_tables(
                        existing_item.as_table_mut().unwrap(),
                        desired_item.as_table().unwrap(),
                        nested_base,
                    );
                }
                // Both `[[…]]` blocks, as many of each: merge block by block, so
                // each block keeps its comments and formatting
                Some(toml_edit::Item::ArrayOfTables(blocks))
                    if desired_item
                        .as_array_of_tables()
                        .is_some_and(|desired_blocks| desired_blocks.len() == blocks.len()) =>
                {
                    let desired_blocks = desired_item.as_array_of_tables().unwrap();
                    let base_blocks = base_item
                        .as_deref()
                        .and_then(toml_edit::Item::as_array_of_tables);
                    for (i, (block, desired_block)) in
                        blocks.iter_mut().zip(desired_blocks.iter()).enumerate()
                    {
                        Self::merge_tables(
                            block,
                            desired_block,
                            base_blocks.and_then(|b| b.get(i)),
                        );
                    }
                }
                Some(existing_item) => {
                    if !Self::items_equal(existing_item, desired_item) {
                        Self::replace_keeping_decor(existing_item, desired_item);
                    }
                }
                None => {
                    existing[key] = desired_item.clone();
                }
            }
        }
    }

    /// Re-spell a desired hook pipeline the way the file already writes it.
    ///
    /// A pipeline serializes in one spelling whatever the file used: one step as
    /// its lone table, more as an inline array of tables. The file may write
    /// either as `[[post-start]]` blocks — the documented form — or as an inline
    /// array. Compared as serialized, an untouched pipeline reads as changed on
    /// every save, and the rewrite moves it to the serialized spelling and drops
    /// the comments on its blocks. So desired steps become blocks where the file
    /// has blocks, and a lone step becomes a one-element array where the file has
    /// an inline array; anything else stays as serialized. Only a pipeline meets
    /// its serialized form in these shapes, so the match keys on shape alone.
    fn in_existing_spelling<'a>(
        existing: Option<&toml_edit::Item>,
        desired: &'a toml_edit::Item,
    ) -> Cow<'a, toml_edit::Item> {
        use toml_edit::{Array, ArrayOfTables, Item, Value};
        match (existing, desired) {
            (Some(Item::ArrayOfTables(_)), Item::Table(step)) => {
                let mut blocks = ArrayOfTables::new();
                blocks.push(step.clone());
                Cow::Owned(Item::ArrayOfTables(blocks))
            }
            (Some(Item::ArrayOfTables(_)), Item::Value(Value::Array(steps)))
                if steps.iter().all(Value::is_inline_table) =>
            {
                let mut blocks = ArrayOfTables::new();
                for step in steps.iter().filter_map(Value::as_inline_table) {
                    blocks.push(step.clone().into_table());
                }
                Cow::Owned(Item::ArrayOfTables(blocks))
            }
            (Some(Item::Value(Value::Array(_))), Item::Table(step)) => {
                let mut steps = Array::new();
                steps.push(step.clone().into_inline_table());
                Cow::Owned(Item::Value(Value::Array(steps)))
            }
            _ => Cow::Borrowed(desired),
        }
    }

    /// Overwrite an item, keeping the value's own decor — the spacing after `=`
    /// and the trailing comment after the value.
    ///
    /// Comments and blank lines *above* the line sit on the key's leaf decor,
    /// which a value replacement never touches. The trailing comment sits on the
    /// value, so replacing the item wholesale drops it: whenever a save changes
    /// a value, and — since template-variable migration became `Structural` —
    /// on a line the command never touched, because every load rewrites retired
    /// names and the next unrelated mutation (declining the commit-generation
    /// offer, say) finds that line changed. An inline table turning into a
    /// standard one takes the other path, `replace_inline_with_table`, which
    /// moves decor onto the header.
    fn replace_keeping_decor(existing_item: &mut toml_edit::Item, desired_item: &toml_edit::Item) {
        let decor = existing_item.as_value().map(|v| v.decor().clone());
        *existing_item = desired_item.clone();
        if let Some(decor) = decor
            && let Some(value) = existing_item.as_value_mut()
        {
            *value.decor_mut() = decor;
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

    /// Compare two Values for equality, ignoring formatting.
    ///
    /// A variant with no arm of its own answers "not equal" for a value that
    /// never changed, and in the inline-table branch of `merge_tables` "not
    /// equal" is what rewrites the user's inline section as a standard table.
    /// So the mismatched-variant arm spells out every variant instead of using
    /// `_`: adding one to `toml_edit::Value` is then a compile error here
    /// rather than a section that silently stops keeping its formatting.
    fn values_equal(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
        use toml_edit::Value;
        match (a, b) {
            (Value::String(a), Value::String(b)) => a.value() == b.value(),
            (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
            (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
            (Value::Float(a), Value::Float(b)) => a.value() == b.value(),
            (Value::Datetime(a), Value::Datetime(b)) => a.value() == b.value(),
            (Value::InlineTable(a), Value::InlineTable(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|bv| Self::values_equal(v, bv)))
            }
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(a, b)| Self::values_equal(a, b))
            }
            // Two different variants. Exhaustive on purpose — see above.
            (
                Value::String(_)
                | Value::Integer(_)
                | Value::Float(_)
                | Value::Boolean(_)
                | Value::Datetime(_)
                | Value::Array(_)
                | Value::InlineTable(_),
                _,
            ) => false,
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
    /// changed keys. A key is removed only when the change being saved reset
    /// it (see `merge_tables`), so a value written out at its default keeps
    /// its line, and schema-unknown keys at any nesting level (typos, fields
    /// from newer wt versions) survive — older wt versions don't silently
    /// strip forward-compatible config data. An existing file that doesn't
    /// parse as a config fails the save.
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
            // Both configs below name these hooks only canonically.
            crate::config::deprecation::canonicalize_hook_keys(&mut existing_doc);

            let desired_doc = self.to_expanded_document()?;

            // Parsed the way `with_locked_mutation` reloads it, so this
            // differs from `desired` only by the change being saved.
            let migrated = crate::config::deprecation::migrate_content(&existing_content);
            let file_config: UserConfig = toml::from_str(&migrated)
                .map_err(|e| ConfigError(format!("Failed to parse config file: {e}")))?;
            let base_doc = file_config.to_expanded_document()?;

            Self::merge_tables(
                existing_doc.as_table_mut(),
                desired_doc.as_table(),
                Some(base_doc.as_table()),
            );
            Self::make_commit_table_implicit_if_only_subtables(&mut existing_doc);

            existing_doc.to_string()
        } else {
            let mut doc = self.to_expanded_document()?;
            Self::make_commit_table_implicit_if_only_subtables(&mut doc);
            doc.to_string()
        };

        crate::config::ensure_config_parses(&toml_string)?;
        crate::utils::write_atomically(config_path, &toml_string)
            .map_err(|e| ConfigError(format!("Failed to write config file: {}", e)))?;

        Ok(())
    }
}

// =========================================================================
// Validation
// =========================================================================

impl UserConfig {
    /// Validate configuration values.
    pub fn validate(&self) -> Result<(), ConfigError> {
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
        }

        Ok(())
    }
}
