//! Experimental git-config source for project configuration (#3454).
//!
//! # Purpose
//!
//! Lets a repo carry private, uncommitted project configuration in git config
//! under the `worktrunk.config.*` namespace. `.git/config` is never
//! transmitted by clone or fetch, so the source is typically local-only — but
//! not by construction: `include`/`includeIf` can pull in files that
//! originate remotely (a cloned dotfiles repo, for instance), which is
//! exactly why commands from this source keep the full approval gate. The
//! keys are shared across every linked worktree because the local scope
//! lives in the common git dir.
//!
//! # Key decisions
//!
//! - **Merged effective read.** Keys come from the bulk `git config --list -z`
//!   map ([`crate::git::Repository::worktrunk_config_git_pairs`]), so git
//!   resolves scope precedence (system → global → local) and conditional
//!   includes before worktrunk ever sees a key. Worktrunk adds no precedence
//!   machinery and never distinguishes scopes.
//! - **All-or-nothing selection.** When any `worktrunk.config.*` key exists,
//!   this source *is* the project config; `.config/wt.toml` (and the
//!   object-store fallback) is not read. There is no key-level merging
//!   between sources. A parse failure here fails the load loudly — falling
//!   back to the file would silently change which config runs.
//! - **Mechanical key mapping.** Strip `worktrunk.config.`; the remainder is
//!   the exact TOML key path as it would appear in `.config/wt.toml`
//!   (`worktrunk.config.post-start` → top-level `post-start`,
//!   `worktrunk.config.list.url` → `[list] url`). No renamed keys, no
//!   git-specific schema. Git lowercases the section and the final key
//!   component and preserves the middle verbatim, so keys must be written in
//!   lowercase — exactly how the schema spells them.
//! - **String leaves only.** Git config values are strings; they map to TOML
//!   strings, which every schema field that motivates this source accepts
//!   (hooks, aliases, `list.url`, `forge.platform`, `commit.generation.
//!   template-append`). Fields requiring other TOML types (currently only
//!   `step.copy-ignored.exclude`, an array) are not expressible; attempting
//!   one surfaces the deserialize error. Repeated keys follow git's own
//!   rule: the last value wins.
//! - **Same approval gate as the file.** Commands from this source pass
//!   through the ordinary project-command approval flow. Git config can
//!   carry remotely-authored content via `include`/`includeIf` (e.g. a
//!   cloned dotfiles repo), so source alone is not a trust signal.
//! - **No migration layer.** Deprecated spellings that deserialize via serde
//!   aliases (`pre-create`/`post-create`) or live fields (`[ci]`) still work
//!   here, but the file-migration rewrites and their deprecation warnings do
//!   not run — `wt config update` has nothing to rewrite in git config, and
//!   migration-only forms work in the file but not in this namespace. Docs
//!   recommend canonical spellings.
//! - **No worktree scope.** The bulk config read runs from the common git
//!   dir, so `config.worktree` values (`extensions.worktreeConfig`) are
//!   never consumed. The diagnostic command, run inside a linked worktree,
//!   can therefore list matching keys this source ignores.
//!
//! # Invariants
//!
//! - [`super::ProjectConfig::load`] is the only constructor of a
//!   `GitConfig`-sourced config, so `config.source` faithfully records
//!   provenance everywhere the cached config flows.
//! - The supersession warning fires exactly when selection actually ignores a
//!   resolvable file — no keys → no warning; no file → no warning.
//! - A `WORKTRUNK_PROJECT_CONFIG_PATH` override (any value, including empty)
//!   disables this source entirely, enforced in the sole accessor
//!   (`Repository::worktrunk_config_git_pairs`) so every consumer inherits
//!   the deferral by construction.

use std::sync::OnceLock;

use color_print::cformat;

use crate::styling::{eprintln, hint_message, warning_message};

use super::{ConfigError, ConfigFileKind, ProjectConfig, ProjectConfigSource};

/// Namespace prefix in git config. Everything after it is a project-config
/// TOML key path.
pub const GIT_CONFIG_PREFIX: &str = "worktrunk.config.";

/// Display label used wherever the git-config source is named as a config
/// origin (`wt config show`, `wt hook show`, parse errors).
pub const GIT_CONFIG_SOURCE_LABEL: &str = "git config (worktrunk.config.*)";

/// The diagnostic command that lists every active key with its scope and
/// origin file. Referenced verbatim from the supersession hint and the docs
/// so all surfaces teach the same incantation.
pub const GIT_CONFIG_LIST_COMMAND: &str =
    r"git config --show-scope --show-origin --get-regexp '^worktrunk\.config\.'";

/// Render `worktrunk.config.*` pairs (prefix already stripped) as a TOML
/// document string.
///
/// Fails when a key path is malformed (empty segment) or when two keys
/// collide (one names a value where another needs a table). Serializing the
/// built table cannot fail in practice — it holds only string leaves and
/// nested tables — so that arm is a plain error passthrough.
pub fn render_git_source_toml(pairs: &[(String, String)]) -> Result<String, ConfigError> {
    let table = pairs_to_table(pairs)?;
    toml::to_string(&table).map_err(|e| ConfigError(format!("{GIT_CONFIG_SOURCE_LABEL}: {e}")))
}

/// Parse `worktrunk.config.*` pairs into a [`ProjectConfig`] tagged with
/// [`ProjectConfigSource::GitConfig`].
///
/// Emits unknown-field warnings through the same channel as file-based
/// config (per-process deduped). A schema violation is a hard error — the
/// caller must not fall back to `.config/wt.toml`.
pub fn project_config_from_git(pairs: &[(String, String)]) -> Result<ProjectConfig, ConfigError> {
    let rendered = render_git_source_toml(pairs)?;

    super::deprecation::warn_unknown_fields::<ProjectConfig>(
        &rendered,
        std::path::Path::new(GIT_CONFIG_SOURCE_LABEL),
        ConfigFileKind::Project,
    );

    let mut config: ProjectConfig = toml::from_str(&rendered).map_err(|e| {
        ConfigError(format!(
            "{} from {GIT_CONFIG_SOURCE_LABEL} failed to parse:\n{e}",
            ConfigFileKind::Project.label(),
        ))
    })?;
    config.source = ProjectConfigSource::GitConfig;
    Ok(config)
}

/// Build the nested TOML table from flat dotted key paths.
fn pairs_to_table(pairs: &[(String, String)]) -> Result<toml::Table, ConfigError> {
    let mut root = toml::Table::new();
    for (key, value) in pairs {
        insert_dotted(&mut root, key, value)?;
    }
    Ok(root)
}

/// Insert one `key = value` pair, creating intermediate tables along the
/// dotted path. Collisions between a value and a table at the same path are
/// errors, not silent overwrites.
fn insert_dotted(root: &mut toml::Table, key: &str, value: &str) -> Result<(), ConfigError> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(ConfigError(format!(
            "Invalid git config key {GIT_CONFIG_PREFIX}{key}: empty key segment"
        )));
    }
    let (leaf, path) = segments.split_last().expect("split('.') yields ≥1 segment");

    let mut table = root;
    let mut walked = String::new();
    for segment in path {
        walked.push_str(segment);
        table = match table
            .entry(segment.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        {
            toml::Value::Table(t) => t,
            _ => {
                return Err(ConfigError(format!(
                    "Conflicting git config keys: {GIT_CONFIG_PREFIX}{walked} is a value, but {GIT_CONFIG_PREFIX}{key} needs it to be a table"
                )));
            }
        };
        walked.push('.');
    }

    match table.entry(leaf.to_string()) {
        toml::map::Entry::Vacant(slot) => {
            slot.insert(toml::Value::String(value.to_string()));
            Ok(())
        }
        toml::map::Entry::Occupied(_) => Err(ConfigError(format!(
            "Conflicting git config keys: {GIT_CONFIG_PREFIX}{key} is set both as a value and as a table"
        ))),
    }
}

/// Warn (once per process) that the git-config source is superseding a
/// project config file that would otherwise load.
///
/// Called from [`super::ProjectConfig::load`] on the git-source selection
/// branch — the single point where supersession actually happens — so the
/// warning fires iff a resolvable file is being ignored. The file check
/// mirrors the load path's own resolution: an on-disk `.config/wt.toml`
/// (or override path), else the committed object-store fallback.
pub(crate) fn warn_superseded_project_file(repo: &crate::git::Repository) {
    if super::deprecation::warnings_suppressed() {
        return;
    }

    // Peek the latch before resolving the label (which can spawn `git show`
    // in the bare/parked layout), but SET it only on emit: setting up front
    // would consume it on the no-file path, and a later
    // `wt config create --project` in the same invocation would then have
    // its born-superseded warning silently suppressed.
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.get().is_some() {
        return;
    }

    let Some(superseded) = superseded_project_file_label(repo) else {
        return;
    };

    // Race-tolerant: the peek above already suppresses the common re-entry;
    // in the rare case two threads pass it before either sets the latch,
    // both emit once, which is preferable to an untestable race-loser guard.
    let _ = WARNED.set(());

    eprintln!(
        "{}",
        warning_message(cformat!(
            "Using <bold>worktrunk.config.*</> keys from git config as the project config; ignoring {superseded}"
        ))
    );
    eprintln!(
        "{}",
        hint_message(cformat!(
            "To list the keys and their origins, run <underline>{GIT_CONFIG_LIST_COMMAND}</>"
        ))
    );
}

/// Display label for the project config file the git-config source is
/// superseding, if one would otherwise load: the on-disk `.config/wt.toml`
/// (or override path), else the committed object-store copy's revision spec.
/// `None` when no file source resolves — then nothing is superseded.
///
/// Shared by the load-time warning and `wt config show`, so the two surfaces
/// cannot disagree about whether supersession is happening.
pub fn superseded_project_file_label(repo: &crate::git::Repository) -> Option<String> {
    match repo.project_config_path() {
        Ok(Some(path)) if path.exists() => Some(crate::path::format_path_for_display(&path)),
        _ => repo
            .default_branch_project_config_content()
            .map(|(_, spec)| spec.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_top_level_hook_maps_to_flattened_key() {
        let config = project_config_from_git(&pairs(&[("post-start", "pnpm install")])).unwrap();
        assert_eq!(config.source, ProjectConfigSource::GitConfig);
        // `post-start` deserializes into the `post_create` field (serde
        // rename — the field kept its pre-rename name).
        let cfg = config.hooks.post_create.as_ref().expect("post-start set");
        let commands: Vec<_> = cfg.commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].template, "pnpm install");
    }

    #[test]
    fn test_nested_keys_map_to_tables() {
        let config = project_config_from_git(&pairs(&[
            ("list.url", "http://localhost:{{ branch | hash_port }}"),
            ("forge.platform", "github"),
            (
                "commit.generation.template-append",
                "use conventional commits",
            ),
        ]))
        .unwrap();
        assert_eq!(
            config.list.url.as_deref(),
            Some("http://localhost:{{ branch | hash_port }}")
        );
        assert_eq!(config.forge.platform.as_deref(), Some("github"));
        assert_eq!(
            config.commit_template_append(),
            Some("use conventional commits")
        );
    }

    #[test]
    fn test_alias_maps_to_aliases_table() {
        let config = project_config_from_git(&pairs(&[("aliases.deploy", "make deploy")])).unwrap();
        let alias = config.aliases.get("deploy").expect("alias present");
        let commands: Vec<_> = alias.commands().collect();
        assert_eq!(commands[0].template, "make deploy");
    }

    #[test]
    fn test_value_table_conflict_is_an_error() {
        let err = project_config_from_git(&pairs(&[
            ("list", "oops"),
            ("list.url", "http://localhost:3000"),
        ]))
        .unwrap_err();
        assert!(err.0.contains("worktrunk.config.list"), "{}", err.0);
    }

    #[test]
    fn test_table_value_conflict_is_an_error() {
        let err = project_config_from_git(&pairs(&[
            ("list.url", "http://localhost:3000"),
            ("list", "oops"),
        ]))
        .unwrap_err();
        assert!(err.0.contains("worktrunk.config.list"), "{}", err.0);
    }

    #[test]
    fn test_empty_segment_is_an_error() {
        let err = project_config_from_git(&pairs(&[("list..url", "x")])).unwrap_err();
        assert!(err.0.contains("empty key segment"), "{}", err.0);
    }

    #[test]
    fn test_non_string_field_fails_loudly() {
        // step.copy-ignored.exclude is an array; a string leaf cannot satisfy
        // it, and the error must surface rather than fall back to the file.
        let err = project_config_from_git(&pairs(&[("step.copy-ignored.exclude", "target")]))
            .unwrap_err();
        assert!(err.0.contains(GIT_CONFIG_SOURCE_LABEL), "{}", err.0);
    }

    #[test]
    fn test_file_source_is_the_default() {
        let config: ProjectConfig = toml::from_str("post-start = \"x\"").unwrap();
        assert_eq!(config.source, ProjectConfigSource::File);
    }

    #[test]
    fn test_superseded_warning_latch_short_circuits_repeat_calls() {
        // A first successful emit sets the process latch; later calls return
        // at the peek without re-resolving the label. Output is not asserted
        // (a parallel test may legitimately have latched warning
        // suppression); this exercises the latch path itself.
        let test = crate::testing::TestRepo::with_initial_commit();
        std::fs::create_dir_all(test.root_path().join(".config")).unwrap();
        std::fs::write(
            test.root_path().join(".config/wt.toml"),
            "pre-merge = \"cargo test\"\n",
        )
        .unwrap();
        test.run_git(&["config", "worktrunk.config.post-start", "echo hi"]);
        let repo = crate::git::Repository::at(test.root_path()).unwrap();
        warn_superseded_project_file(&repo);
        warn_superseded_project_file(&repo);
    }
}
