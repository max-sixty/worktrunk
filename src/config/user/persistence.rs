//! Config persistence - loading and saving to disk.
//!
//! Handles TOML serialization with formatting (multiline arrays, implicit tables)
//! and preserves comments when updating existing files.

use config::ConfigError;
use serde::Serialize;

use super::UserConfig;
use super::path::get_config_path;
use super::sections::CommitGenerationConfig;

impl UserConfig {
    /// Save the current configuration to the default config file location
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_impl(None)
    }

    /// Internal save implementation that handles both default and custom paths
    pub(super) fn save_impl(
        &self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        match config_path {
            Some(path) => self.save_to(path),
            None => {
                let path = get_config_path().ok_or_else(|| {
                    ConfigError::Message(
                        "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME environment variable".to_string(),
                    )
                })?;
                self.save_to(&path)
            }
        }
    }

    /// Update the [commit.generation] section in the document.
    fn update_commit_generation_section(&self, doc: &mut toml_edit::DocumentMut) {
        // Helper to update a string field only if changed, preserving comments
        fn update_string_field(
            table: &mut toml_edit::Table,
            key: &str,
            new_value: Option<&String>,
        ) {
            match new_value {
                Some(v) => {
                    // Only update if value changed (preserves comments if unchanged)
                    let current = table.get(key).and_then(|i| i.as_str());
                    if current != Some(v.as_str()) {
                        table[key] = toml_edit::value(v.as_str());
                    }
                }
                None => {
                    table.remove(key);
                }
            }
        }

        if let Some(ref commit_cfg) = self.configs.commit
            && let Some(ref gen_cfg) = commit_cfg.generation
        {
            // Ensure [commit] table exists
            if !doc.contains_key("commit") {
                doc["commit"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            if let Some(commit_table) = doc["commit"].as_table_mut() {
                // Ensure [commit.generation] table exists
                if !commit_table.contains_key("generation") {
                    commit_table["generation"] = toml_edit::Item::Table(toml_edit::Table::new());
                }
                if let Some(gen_table) = commit_table["generation"].as_table_mut() {
                    update_string_field(gen_table, "command", gen_cfg.command.as_ref());
                    update_string_field(gen_table, "template", gen_cfg.template.as_ref());
                    update_string_field(gen_table, "template-file", gen_cfg.template_file.as_ref());
                    update_string_field(
                        gen_table,
                        "squash-template",
                        gen_cfg.squash_template.as_ref(),
                    );
                    update_string_field(
                        gen_table,
                        "squash-template-file",
                        gen_cfg.squash_template_file.as_ref(),
                    );
                }
            }
        }
    }

    /// Update the [projects] section in the document.
    fn update_projects_section(&self, doc: &mut toml_edit::DocumentMut) {
        // Ensure projects table exists
        if !doc.contains_key("projects") {
            doc["projects"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        if let Some(projects) = doc["projects"].as_table_mut() {
            // Remove stale projects
            let stale: Vec<_> = projects
                .iter()
                .filter(|(k, _)| !self.projects.contains_key(*k))
                .map(|(k, _)| k.to_string())
                .collect();
            for key in stale {
                projects.remove(&key);
            }

            // Add/update projects
            for (project_id, project_config) in &self.projects {
                if !projects.contains_key(project_id) {
                    projects[project_id] = toml_edit::Item::Table(toml_edit::Table::new());
                }

                // worktree-path (only if set)
                if let Some(ref path) = project_config.overrides.worktree_path {
                    projects[project_id]["worktree-path"] = toml_edit::value(path);
                } else if let Some(table) = projects[project_id].as_table_mut() {
                    table.remove("worktree-path");
                }

                // approved-commands
                let commands =
                    Self::format_multiline_array(project_config.approved_commands.iter());
                projects[project_id]["approved-commands"] = toml_edit::value(commands);

                // Per-project nested config sections
                Self::serialize_project_config_section(
                    projects,
                    project_id,
                    "commit-generation",
                    project_config.commit_generation.as_ref(),
                );
                Self::serialize_project_config_section(
                    projects,
                    project_id,
                    "list",
                    project_config.overrides.list.as_ref(),
                );
                Self::serialize_project_config_section(
                    projects,
                    project_id,
                    "commit",
                    project_config.overrides.commit.as_ref(),
                );
                Self::serialize_project_config_section(
                    projects,
                    project_id,
                    "merge",
                    project_config.overrides.merge.as_ref(),
                );
                Self::serialize_project_config_section(
                    projects,
                    project_id,
                    "select",
                    project_config.overrides.select.as_ref(),
                );
            }
        }
    }

    /// Format a string array as multiline TOML for readability
    ///
    /// TODO: toml_edit doesn't provide a built-in multiline array format option.
    /// Consider replacing with a dependency if one emerges that handles this automatically.
    fn format_multiline_array<'a>(items: impl Iterator<Item = &'a String>) -> toml_edit::Array {
        let mut array: toml_edit::Array = items.collect();
        for item in array.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        array.set_trailing("\n");
        array.set_trailing_comma(true);
        array
    }

    /// Serialize a per-project config section (commit-generation, list, commit, merge).
    ///
    /// If the config is Some, serializes it as a nested table. If None, removes the section.
    /// Used when updating an existing file.
    fn serialize_project_config_section<T: Serialize>(
        projects: &mut toml_edit::Table,
        project_id: &str,
        section_name: &str,
        config: Option<&T>,
    ) {
        if let Some(cfg) = config {
            // Serialize to TOML value, then convert to toml_edit Item
            if let Ok(toml_value) = toml::to_string(cfg)
                && let Ok(parsed) = toml_value.parse::<toml_edit::DocumentMut>()
            {
                let mut table = toml_edit::Table::new();
                for (k, v) in parsed.iter() {
                    table[k] = v.clone();
                }
                projects[project_id][section_name] = toml_edit::Item::Table(table);
            }
        } else if let Some(project_table) = projects[project_id].as_table_mut() {
            project_table.remove(section_name);
        }
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
            } else if let Some(t) = item.as_table_mut() {
                Self::expand_inline_tables(t);
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

    /// Save the current configuration to a specific file path
    ///
    /// Use this in tests to save to a temporary location instead of the user's config.
    /// Preserves comments and formatting in the existing file when possible.
    ///
    /// TODO: This design is fragile. When file exists, we surgically update specific
    /// sections to preserve comments. If a new programmatically-modifiable field is added
    /// but not handled here, changes won't persist. Consider using a diff-based approach:
    /// compare self vs existing config and only update what changed.
    pub fn save_to(&self, config_path: &std::path::Path) -> Result<(), ConfigError> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::Message(format!("Failed to create config directory: {}", e))
            })?;
        }

        let toml_string = if config_path.exists() {
            // Surgically update sections to preserve comments
            let existing_content = std::fs::read_to_string(config_path)
                .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

            let mut doc: toml_edit::DocumentMut = existing_content
                .parse()
                .map_err(|e| ConfigError::Message(format!("Failed to parse config file: {}", e)))?;

            // Update all programmatically-modifiable sections
            // NOTE: If you add a new setter that modifies config, add the update here too!
            if self.skip_shell_integration_prompt {
                doc["skip-shell-integration-prompt"] = toml_edit::value(true);
            } else {
                doc.remove("skip-shell-integration-prompt");
            }

            if self.skip_commit_generation_prompt {
                doc["skip-commit-generation-prompt"] = toml_edit::value(true);
            } else {
                doc.remove("skip-commit-generation-prompt");
            }

            self.update_commit_generation_section(&mut doc);
            self.update_projects_section(&mut doc);
            Self::make_commit_table_implicit_if_only_subtables(&mut doc);

            doc.to_string()
        } else {
            // No existing file: serialize struct directly, then post-process formatting
            let mut doc = toml_edit::ser::to_document(&self)
                .map_err(|e| ConfigError::Message(format!("Serialization error: {e}")))?;

            // Convert inline tables to standard tables for readability
            Self::expand_inline_tables(doc.as_table_mut());
            Self::make_commit_table_implicit_if_only_subtables(&mut doc);

            // Post-process: format approved-commands as multiline arrays for readability
            if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
                projects.set_implicit(true); // Don't emit [projects] header
                for (_, project) in projects.iter_mut() {
                    if let Some(arr) = project
                        .get_mut("approved-commands")
                        .and_then(|a| a.as_array_mut())
                    {
                        for item in arr.iter_mut() {
                            item.decor_mut().set_prefix("\n    ");
                        }
                        arr.set_trailing("\n");
                        arr.set_trailing_comma(true);
                    }
                }
            }

            doc.to_string()
        };

        std::fs::write(config_path, toml_string)
            .map_err(|e| ConfigError::Message(format!("Failed to write config file: {}", e)))?;

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
        if let Some(ref path) = self.configs.worktree_path
            && path.trim().is_empty()
        {
            return Err(ConfigError::Message("worktree-path cannot be empty".into()));
        }

        // Validate per-project configs
        for (project, project_config) in &self.projects {
            // Validate worktree path
            if let Some(ref path) = project_config.overrides.worktree_path
                && path.trim().is_empty()
            {
                return Err(ConfigError::Message(format!(
                    "projects.{project}.worktree-path cannot be empty"
                )));
            }

            // Validate commit generation config (check both old and new locations)
            // Old: [projects."...".commit-generation] (deprecated)
            if let Some(ref cg) = project_config.commit_generation {
                Self::validate_commit_generation(cg, &format!("projects.{project}"))?;
            }
            // New: [projects."...".commit.generation]
            if let Some(ref commit) = project_config.overrides.commit
                && let Some(ref cg) = commit.generation
            {
                Self::validate_commit_generation(cg, &format!("projects.{project}"))?;
            }
        }

        // Validate commit generation config (check both old and new locations)
        let commit_gen = self.commit_generation(None);
        if commit_gen.template.is_some() && commit_gen.template_file.is_some() {
            return Err(ConfigError::Message(
                "commit.generation.template and commit.generation.template-file are mutually exclusive".into(),
            ));
        }

        if commit_gen.squash_template.is_some() && commit_gen.squash_template_file.is_some() {
            return Err(ConfigError::Message(
                "commit.generation.squash-template and commit.generation.squash-template-file are mutually exclusive".into(),
            ));
        }

        Ok(())
    }

    fn validate_commit_generation(
        cg: &CommitGenerationConfig,
        prefix: &str,
    ) -> Result<(), ConfigError> {
        if cg.template.is_some() && cg.template_file.is_some() {
            return Err(ConfigError::Message(format!(
                "{prefix}.commit-generation.template and template-file are mutually exclusive"
            )));
        }
        if cg.squash_template.is_some() && cg.squash_template_file.is_some() {
            return Err(ConfigError::Message(format!(
                "{prefix}.commit-generation.squash-template and squash-template-file are mutually exclusive"
            )));
        }
        Ok(())
    }
}
