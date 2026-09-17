//! Writing a config mutation into the file.
//!
//! A mutation writes the one value it changed, at its key path, into the file
//! parsed as a `toml_edit` document ([`ConfigEdit`]). The rest of the file keeps
//! its comments, formatting, key order, values written at their defaults, keys
//! wt doesn't know, and deprecated spellings the load path migrates in memory.
//! `cargo add`, uv, and poetry edit user-owned TOML the same way; no library
//! merges a serialized struct into a document while keeping all of that.
//!
//! The exception is an edit the load path wouldn't read as written. A load-time
//! migration that moves config onto the edit's path — a deprecated
//! `[commit-generation]` onto `[commit.generation]` — declines once the
//! canonical table exists, so the edit goes into the migrated file instead,
//! which writes those migrations too ([`ConfigFile::edited`]).

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use crate::config::ConfigError;
use crate::config::Deprecations;
use crate::config::deprecation::migrate_doc;
use crate::path::format_path_for_display;

use super::UserConfig;

/// One value a config mutation writes: the tables it sits in, its key, and the
/// value.
pub(super) struct ConfigEdit<'a> {
    pub(super) tables: Vec<&'a str>,
    pub(super) key: &'a str,
    pub(super) value: Value,
}

impl ConfigEdit<'_> {
    /// Set the value in `doc`, leaving everything else as it is.
    ///
    /// An existing value is replaced in place and keeps its decor — the spacing
    /// after `=` and a trailing comment. A missing table is created implicit, so
    /// it writes no header of its own (`[commit.generation]`, not `[commit]`
    /// above it), or inline inside an inline table, which can't hold a standard
    /// one. An existing table is edited in whatever form the file writes it:
    /// standard, inline, or dotted keys.
    pub(super) fn apply(&self, doc: &mut DocumentMut) -> Result<(), ConfigError> {
        let mut table: &mut dyn TableLike = doc.as_table_mut();
        let mut inline = false;
        for &name in &self.tables {
            let item = table.entry(name).or_insert_with(|| {
                if inline {
                    Item::Value(Value::InlineTable(Default::default()))
                } else {
                    let mut implicit = Table::new();
                    implicit.set_implicit(true);
                    Item::Table(implicit)
                }
            });
            inline = item.is_inline_table();
            table = item.as_table_like_mut().ok_or_else(|| {
                ConfigError(format!(
                    "Failed to write config file: `{name}` is not a table"
                ))
            })?;
        }

        let mut value = self.value.clone();
        match table.get_mut(self.key) {
            Some(Item::Value(existing)) => {
                *value.decor_mut() = existing.decor().clone();
                *existing = value;
            }
            _ => {
                table.insert(self.key, Item::Value(value));
            }
        }
        Ok(())
    }
}

/// The user config file as a mutation reads it: the document to edit and the
/// config it loads as.
pub(super) struct ConfigFile {
    doc: DocumentMut,
    pub(super) config: UserConfig,
}

impl ConfigFile {
    /// Read and parse the file; a missing file is an empty document holding the
    /// default config.
    pub(super) fn read(path: &std::path::Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self {
                doc: DocumentMut::new(),
                config: UserConfig::default(),
            });
        }

        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError(format!(
                "Failed to read config file {}: {}",
                format_path_for_display(path),
                e
            ))
        })?;
        let parse_error = |e: String| {
            ConfigError(format!(
                "Failed to parse config file {}: {}",
                format_path_for_display(path),
                e
            ))
        };
        let doc: DocumentMut = content
            .parse()
            .map_err(|e: toml_edit::TomlError| parse_error(e.to_string()))?;
        let config = load(&doc).map_err(|e| parse_error(e.to_string()))?;
        Ok(Self { doc, config })
    }

    /// The file's content with `edit` applied, such that it loads as `expected`.
    ///
    /// The edit goes into the file as written when that loads as `expected`.
    /// Otherwise a load-time migration touches the edit's path — a deprecated
    /// `[commit-generation]` migrates to `[commit.generation]` only while that
    /// table is absent — and the edit goes into the migrated file, which loads
    /// as `expected` by construction: the migrations are idempotent and the
    /// edit lands after them. That file carries *every* load-path migration,
    /// not just the one on the edit's path, so it can also move an unrelated
    /// deprecated section and drop the keys its destination has no field for.
    /// A mutation is otherwise not what materializes migrations —
    /// `wt config update` is — so [`Edited::Migrated`] says so, and its caller
    /// tells the user.
    pub(super) fn edited(
        &self,
        edit: &ConfigEdit,
        expected: &UserConfig,
    ) -> Result<Edited, ConfigError> {
        let mut doc = self.doc.clone();
        edit.apply(&mut doc)?;
        if load(&doc).is_ok_and(|config| &config == expected) {
            return Ok(Edited::AsWritten(doc.to_string()));
        }

        let mut doc = self.doc.clone();
        let changes = migrate_doc(&mut doc);
        edit.apply(&mut doc)?;
        Ok(Edited::Migrated {
            content: doc.to_string(),
            changes,
        })
    }
}

/// A config file with an edit applied, and whether writing it took the load-path
/// migrations with it.
pub(super) enum Edited {
    AsWritten(String),
    /// The migrations came too, `changes` reporting each one — the sections
    /// they moved and the keys they removed.
    Migrated {
        content: String,
        changes: Deprecations,
    },
}

impl Edited {
    pub(super) fn content(&self) -> &str {
        match self {
            Self::AsWritten(content) | Self::Migrated { content, .. } => content,
        }
    }
}

/// The config a document loads as, after the load-time migrations.
fn load(doc: &DocumentMut) -> Result<UserConfig, toml::de::Error> {
    let mut migrated = doc.clone();
    migrate_doc(&mut migrated);
    toml::from_str(&migrated.to_string())
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
