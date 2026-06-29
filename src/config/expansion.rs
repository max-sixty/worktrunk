//! Template expansion utilities for worktrunk
//!
//! Uses minijinja for template rendering. A single generic function takes a
//! [`ShellEscapeMode`] selecting how interpolated values are escaped:
//! - `Posix` — POSIX single-quoting, for command lines fed to `Cmd::shell`
//!   (`sh`/Git Bash): hooks, aliases.
//! - `PowerShell` — PowerShell single-quoting, for the `--execute` payload
//!   when the active directive shell is the PowerShell wrapper.
//! - `Literal` — values substituted verbatim, for filesystem paths.
//!
//! All templates support Jinja2 syntax including filters, conditionals, and loops.
//!
//! See `wt hook --help` for available filters and functions.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use color_print::cformat;
use minijinja::value::{Enumerator, Object, ObjectRepr};
use minijinja::{Environment, ErrorKind, UndefinedBehavior, Value};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::git::{Diagnostic, HookType, Repository};
use crate::path::to_posix_path;
use crate::shell_exec::{ShellEscapeMode, shell_escape_for};
use crate::styling::{
    eprintln, error_message, format_bash_with_gutter, format_with_gutter, hint_message,
    info_message, verbosity,
};

/// Active-context vars: point at the branch the operation acts on.
///
/// `upstream` is conditional on branch tracking configuration but is listed
/// here so templates may reference it in any context (guarded by
/// `{% if upstream %}`).
pub const ACTIVE_VARS: &[&str] = &[
    "branch",
    "worktree_path",
    "worktree_name",
    "commit",
    "short_commit",
    "upstream",
];

/// Repo/remote-metadata vars: describe the repository hosting the operation.
pub const REPO_VARS: &[&str] = &[
    "repo",
    "repo_path",
    "owner",
    "primary_worktree_path",
    "default_branch",
    "remote",
    "remote_url",
];

/// Exec-context vars always available outside hook infrastructure.
///
/// `cwd` is populated for every template expansion; `hook_type`/`hook_name`
/// are added by the hook runner itself (`HOOK_INFRASTRUCTURE_VARS`).
pub const EXEC_BASE_VARS: &[&str] = &["cwd"];

/// Template variables available in every context: the concatenation of
/// [`ACTIVE_VARS`], [`REPO_VARS`], and [`EXEC_BASE_VARS`].
///
/// Populated by `build_hook_context()` in `command_executor.rs`. Operation-
/// context vars (`base`, `target`, `pr_*`) and infrastructure vars
/// (`hook_type`, `hook_name`) are not in the base set — they're added per-
/// scope by `hook_extras` and the hook runner itself.
pub fn base_vars() -> Vec<&'static str> {
    let mut v = Vec::with_capacity(ACTIVE_VARS.len() + REPO_VARS.len() + EXEC_BASE_VARS.len());
    v.extend_from_slice(ACTIVE_VARS);
    v.extend_from_slice(REPO_VARS);
    v.extend_from_slice(EXEC_BASE_VARS);
    v
}

/// Reserved context key carrying a JSON-encoded `Vec<String>` of positional
/// CLI args forwarded to an alias. The key flows through
/// `HashMap<String, String>` — stable for stdin JSON — and
/// [`expand_template`] rehydrates it as a `ShellArgs` object so bare
/// `{{ args }}` renders as a space-joined, shell-escaped string while
/// indexing, iteration, and `length` behave like a sequence.
pub const ALIAS_ARGS_KEY: &str = "args";

/// Deprecated template variable aliases (still valid for backward compatibility).
///
/// These map to current variables and are available in every scope:
/// - `main_worktree` → `repo`
/// - `repo_root` → `repo_path`
/// - `worktree` → `worktree_path`
/// - `main_worktree_path` → `primary_worktree_path`
pub const DEPRECATED_TEMPLATE_VARS: &[&str] = &[
    "main_worktree",
    "repo_root",
    "worktree",
    "main_worktree_path",
];

/// Variables available in `wt list` custom-column templates (plus `vars.*`).
///
/// Deliberately narrower than [`base_vars`]: column values are computed per
/// row before the table skeleton renders, so only row-identity data that is
/// already in memory at that point is offered.
pub const LIST_COLUMN_VARS: &[&str] = &["branch", "worktree_path", "worktree_name"];

/// The context in which a template will be expanded.
///
/// Validation uses this to answer "which variables are available here?" —
/// the single source of truth for hook-type-specific vars, alias-only vars,
/// and the `--execute` context. Each hook type gets the base set plus its
/// own extras (e.g., `target` for merge/remove, `base` for create/switch).
#[derive(Debug, Clone, Copy)]
pub enum ValidationScope {
    /// A hook of the given type. Adds hook infrastructure vars (`hook_type`,
    /// `hook_name`) plus hook-specific vars (`base`, `target`, etc.).
    Hook(HookType),
    /// The `--execute` template or trailing args for `wt switch --create`.
    /// Adds `base` / `base_worktree_path` for the source worktree.
    SwitchExecute,
    /// An alias body. Adds `args` for positional CLI forwarding.
    Alias,
}

/// Hook-type-specific extras that sit on top of [`base_vars`].
///
/// These are the vars injected by callers via `extra_vars` when running a
/// hook. Keeping the mapping in one place means "which vars work in a
/// `post-merge` hook?" is answerable without chasing inline comments.
///
/// Each arm's order must be a prefix-ordered subset of the operation-context
/// block in the user-facing help table (`src/cli/mod.rs`, `## Template
/// variables`): `base, base_worktree_path, target, target_worktree_path,
/// pr_number, pr_url`.
fn hook_extras(hook_type: HookType) -> &'static [&'static str] {
    use HookType::*;
    match hook_type {
        // Switch: source branch (`base`) and destination (`target`).
        // `pr_number`/`pr_url` are populated for `post-switch` when creating
        // via `pr:N` / `mr:N`; pre-switch fires before the PR/MR API call,
        // so they're never set there but remain accepted for portability.
        PreSwitch | PostSwitch => &[
            "base",
            "base_worktree_path",
            "target",
            "target_worktree_path",
            "pr_number",
            "pr_url",
        ],
        // Create: source worktree (`base`) and newly-created destination
        // (`target`). On create, the destination branch equals the bare `branch`
        // var — `target` is accepted for template portability with switch hooks.
        // `pr_number`/`pr_url` are populated when creating via `pr:N` / `mr:N`
        // (GitLab MRs reuse the same `pr_*` names).
        PreCreate | PostCreate => &[
            "base",
            "base_worktree_path",
            "target",
            "target_worktree_path",
            "pr_number",
            "pr_url",
        ],
        // Commit: integration target for the pre-commit squash.
        PreCommit | PostCommit => &["target"],
        // Merge: where the feature is being merged into.
        PreMerge | PostMerge => &["target", "target_worktree_path"],
        // Remove: where the user ends up after removal.
        PreRemove | PostRemove => &["target", "target_worktree_path"],
    }
}

/// Vars added by the hook execution infrastructure itself (`expand_commands`
/// / `expand_command_template`), regardless of hook type.
const HOOK_INFRASTRUCTURE_VARS: &[&str] = &["hook_type", "hook_name"];

/// All template variables available in a given scope.
///
/// The returned list is [`base_vars`] + scope-specific extras + deprecated
/// aliases. Used by [`validate_template`] to build the placeholder context
/// and by error messages to list what the user could have typed.
pub fn vars_available_in(scope: ValidationScope) -> Vec<&'static str> {
    let mut vars: Vec<&'static str> = base_vars();
    match scope {
        ValidationScope::Hook(hook_type) => {
            vars.extend(HOOK_INFRASTRUCTURE_VARS);
            vars.extend(hook_extras(hook_type));
            vars.push(ALIAS_ARGS_KEY);
        }
        ValidationScope::SwitchExecute => {
            vars.extend(["base", "base_worktree_path"]);
        }
        ValidationScope::Alias => {
            vars.push(ALIAS_ARGS_KEY);
        }
    }
    vars.extend(DEPRECATED_TEMPLATE_VARS);
    vars
}

/// Shared formatter for [`format_hook_variables`] and [`format_alias_variables`].
///
/// Renders `vars` as an aligned `name = value` block — no heading, no indent,
/// caller wraps. Values come from `ctx`; vars absent from `ctx` render as
/// `(unset)`, surfacing operation-specific gaps (e.g., `target_worktree_path`
/// during `wt switch -`, `upstream` when the branch doesn't track a remote).
///
/// When `referenced` is `Some`, vars absent from `ctx` *and* not in the set
/// render as dim `(unused)` — `build_hook_context` only skips computation for
/// expensive vars, so this fires precisely when the gate saved real work.
/// Cheap vars are populated unconditionally and always show their value, even
/// when the body doesn't reference them. `(unset)` is reserved for the
/// distinct case of a referenced var the operation couldn't supply.
///
/// `(unset)` relies on an invariant in `build_hook_context`: optional vars
/// are omitted from the map rather than inserted as empty strings. If a
/// future caller starts inserting `""`, revisit the empty-vs-absent
/// distinction here.
fn format_variables_table(
    vars: &[&'static str],
    ctx: &HashMap<String, String>,
    referenced: Option<&BTreeSet<String>>,
) -> String {
    let max_name = vars.iter().map(|v| v.len()).max().unwrap_or(0);
    vars.iter()
        .map(|var| match ctx.get(*var) {
            Some(value) => format!("{var:<max_name$} = {value}"),
            None if referenced.is_some_and(|r| !r.contains(*var)) => {
                cformat!("<dim>{var:<max_name$} = (unused)</>")
            }
            None => format!("{var:<max_name$} = (unset)"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format the resolved template variables for a hook invocation.
///
/// Ordered per the `## Template variables` help table in `src/cli/mod.rs`:
/// active, operation, repo, exec, infrastructure.
///
/// Deprecated aliases and `vars.*` (user state) are intentionally omitted.
pub fn format_hook_variables(hook_type: HookType, ctx: &HashMap<String, String>) -> String {
    let vars: Vec<&'static str> = ACTIVE_VARS
        .iter()
        .chain(hook_extras(hook_type))
        .chain(REPO_VARS)
        .chain(EXEC_BASE_VARS)
        .chain(HOOK_INFRASTRUCTURE_VARS)
        .copied()
        .collect();
    format_variables_table(&vars, ctx, None)
}

/// Format the resolved template variables for an alias invocation.
///
/// Ordering mirrors [`format_hook_variables`]; alias scope has no operation
/// or infrastructure vars, and `args` lives in the exec group per the help
/// table in `src/cli/mod.rs` (alongside `cwd`).
///
/// `args` is stored as a JSON-encoded `Vec<String>` per the [`ALIAS_ARGS_KEY`]
/// contract; the table displays it space-joined and shell-escaped so it
/// matches what `{{ args }}` substitutes in templates.
///
/// `referenced` (the set of vars the body actually substitutes) controls
/// the dim `(unused)` marker for vars the operation skipped computing —
/// the reader sees what's reachable without paying for values the body
/// won't substitute.
pub fn format_alias_variables(
    ctx: &HashMap<String, String>,
    referenced: Option<&BTreeSet<String>>,
) -> String {
    let vars: Vec<&'static str> = ACTIVE_VARS
        .iter()
        .copied()
        .chain(REPO_VARS.iter().copied())
        .chain(EXEC_BASE_VARS.iter().copied())
        .chain(std::iter::once(ALIAS_ARGS_KEY))
        .collect();
    let mut display_ctx = ctx.clone();
    if let Some(json) = ctx.get(ALIAS_ARGS_KEY) {
        let args: Vec<String> = serde_json::from_str(json)
            .expect("ALIAS_ARGS_KEY is always serialized from a Vec<String>");
        display_ctx.insert(ALIAS_ARGS_KEY.into(), shell_join(&args));
    }
    format_variables_table(&vars, &display_ctx, referenced)
}

/// Extend `referenced` with the implicit context-map keys an alias dispatch
/// needs:
///
/// - [`ALIAS_ARGS_KEY`] (`args`) — always present in alias scope (`run_alias`
///   inserts it after `build_hook_context`), so include it so the verbose
///   table renders the row.
/// - `branch` when the body references `vars` — `expand_template` reads
///   `branch` out of the context map to look up `{{ vars.X }}` from git
///   config at execution time. Bare `{{ vars }}`, `{{ vars.X }}`, and
///   `{{ vars["X"] }}` all surface `vars` in `undeclared_variables`.
///
/// New implicit dependencies (template-level reads from the context map that
/// `undeclared_variables` doesn't see) belong here, not at each call site.
pub fn alias_context_filter(mut referenced: BTreeSet<String>) -> BTreeSet<String> {
    let needs_branch = referenced.contains("vars");
    referenced.insert(ALIAS_ARGS_KEY.to_string());
    if needs_branch {
        referenced.insert("branch".to_string());
    }
    referenced
}

/// Positional CLI args forwarded from `wt <alias> a b c` into the alias's
/// template context. Bare `{{ args }}` renders as a space-joined,
/// shell-escaped string ready to append to a command line; `{{ args[0] }}`
/// and `{% for a in args %}…{% endfor %}` and `{{ args | length }}` all
/// behave as expected because the object reports as an
/// [`ObjectRepr::Seq`].
///
/// Shell escaping happens at render time via POSIX [`shell_escape_for`]
/// rather than through the template environment's formatter — the formatter
/// would otherwise quote the already-escaped joined string as a whole. The
/// formatter installed by `expand_template` detects `ShellArgs` and writes
/// it through unmodified.
///
/// `args` exists only in alias scope, whose bodies always run through
/// `Cmd::shell` (POSIX) — so this rendering is unconditionally POSIX,
/// independent of the active directive shell.
#[derive(Debug)]
struct ShellArgs(Vec<String>);

impl ShellArgs {
    fn new(args: Vec<String>) -> Self {
        Self(args)
    }
}

impl Object for ShellArgs {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let idx = key.as_usize()?;
        self.0.get(idx).cloned().map(Value::from)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.0.len())
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&shell_join(&self.0))
    }
}

/// Space-join POSIX-shell-escaped args — the canonical rendering of
/// `{{ args }}` used by both `ShellArgs::render` (template expansion) and the
/// alias `-v` variable table. Always POSIX: see [`ShellArgs`].
fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_escape_for(ShellEscapeMode::Posix, a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Hash a string to a port in range 10000-19999.
fn string_to_port(s: &str) -> u16 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    10000 + (h.finish() % 10000) as u16
}

const CODENAME_MAX_WORDS: usize = 8;

/// Wordlists used by [`codename`].
///
/// Sourced from `petname::Petnames::medium()` (1198 adjectives, 1052 nouns —
/// ~1.26M `codename(2)` combinations, ~1.5B for `codename(3)`). The pin in
/// `Cargo.toml` is `=3.0.0`; bumping it may change, add, or remove words and
/// would silently shift every existing user's `worktree-path` output. Treat
/// any upgrade as a breaking change — see `test_codename_outputs_are_stable`.
///
/// `petname::Petnames::medium()` is effectively free — its `Cow::Borrowed`
/// fields point at static slices embedded by the `petnames!` macro — so
/// re-constructing per call is cheaper than reaching for a `OnceLock`.
fn codename_words() -> petname::Petnames<'static> {
    petname::Petnames::medium()
}

/// Sanitize a branch name for use in filesystem paths.
///
/// Replaces path separators (`/` and `\`) with dashes to prevent directory traversal
/// and ensure the branch name is a single path component.
///
/// # Examples
/// ```
/// use worktrunk::config::sanitize_branch_name;
///
/// assert_eq!(sanitize_branch_name("feature/foo"), "feature-foo");
/// assert_eq!(sanitize_branch_name("user\\task"), "user-task");
/// assert_eq!(sanitize_branch_name("simple-branch"), "simple-branch");
/// ```
pub fn sanitize_branch_name(branch: &str) -> String {
    branch.replace(['/', '\\'], "-")
}

/// Sanitize a string for use as a database identifier.
///
/// Transforms input into an identifier compatible with most SQL databases
/// (PostgreSQL, MySQL, SQL Server). The transformation is more aggressive than
/// `sanitize_branch_name` to ensure compatibility with database identifier rules.
///
/// # Transformation Rules (applied in order)
/// 1. Convert to lowercase (ensures portability across case-sensitive systems)
/// 2. Replace non-alphanumeric characters with `_` (only `[a-z0-9_]` are safe)
/// 3. Collapse consecutive underscores into single underscore
/// 4. Add `_` prefix if identifier starts with a digit (SQL prohibits leading digits)
/// 5. Append 3-character hash suffix for uniqueness (avoids reserved words and collisions)
/// 6. Truncate to 48 characters total (well within PostgreSQL's 63-char identifier
///    limit, leaving room for prefixes/suffixes when composing paths or identifiers)
///
/// The hash suffix ensures that:
/// - SQL reserved words are avoided (e.g., `user` → `user_abc`, not a reserved word)
/// - Different inputs don't collide (e.g., `a-b` and `a_b` get different suffixes)
///
/// # Limitations
/// - Empty input produces empty output (not a valid identifier in most DBs)
///
/// # Examples
/// ```
/// use worktrunk::config::sanitize_db;
///
/// // Hash suffix ensures uniqueness
/// assert!(sanitize_db("feature/auth").starts_with("feature_auth_"));
/// assert!(sanitize_db("123-bug-fix").starts_with("_123_bug_fix_"));
/// assert!(sanitize_db("UPPERCASE.Branch").starts_with("uppercase_branch_"));
///
/// // Different inputs get different suffixes even if base transforms are identical
/// assert_ne!(sanitize_db("a-b"), sanitize_db("a_b"));
/// ```
pub fn sanitize_db(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Single pass: lowercase, replace non-alphanumeric with underscore, collapse consecutive
    let mut result = String::with_capacity(s.len() + 4); // +4 for _xxx suffix
    let mut prev_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            result.push('_');
            prev_underscore = true;
        }
    }

    // Prefix with underscore if starts with digit
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    // Truncate base to leave room for hash suffix (4 chars: _ + 3 hash chars).
    // Total cap is 48 chars (well within PostgreSQL's 63-char identifier limit),
    // so max base is 44.
    if result.len() > 44 {
        result.truncate(44);
    }

    // Append 3-character hash suffix for collision avoidance and reserved word safety
    // Hash is computed from original input, ensuring unique suffixes for colliding transforms
    if !result.ends_with('_') {
        result.push('_');
    }
    result.push_str(&short_hash(s));

    result
}

/// Generate a 3-character hash suffix from a string.
///
/// Uses base36 (0-9, a-z) for a compact representation with 46,656 unique values.
/// Used by `sanitize_db` and `sanitize_for_filename` to avoid collisions.
pub fn short_hash(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    let hash = h.finish();

    // Convert to base36 and take 3 characters
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let c0 = CHARS[(hash % 36) as usize];
    let c1 = CHARS[((hash / 36) % 36) as usize];
    let c2 = CHARS[((hash / 1296) % 36) as usize];
    String::from_utf8(vec![c0, c1, c2]).unwrap()
}

fn codename_index(input: &str, position: usize, salt: usize, pool: &str, len: usize) -> usize {
    // Cast to a fixed-width type so the hash is identical across 32-bit and
    // 64-bit builds. `usize::to_le_bytes` is architecture-dependent and would
    // change the on-disk codename for the same branch when users move between
    // architectures.
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher.update([0]);
    hasher.update((position as u64).to_le_bytes());
    hasher.update((salt as u64).to_le_bytes());
    hasher.update(pool.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    // Take the modulo in u64 before narrowing — `as usize` on a 32-bit
    // build truncates the upper 32 bits and would pick a different word
    // than 64-bit for the same branch, defeating the whole point of
    // hashing through a fixed-width type above.
    (u64::from_le_bytes(bytes) % len as u64) as usize
}

fn codename(input: &str, words: usize) -> String {
    let lists = codename_words();
    let adjectives: &[&str] = lists.adjectives.as_ref();
    let nouns: &[&str] = lists.nouns.as_ref();

    let mut parts = Vec::with_capacity(words);
    let adjective_count = words.saturating_sub(1);

    for position in 0..adjective_count {
        let mut salt = 0;
        loop {
            let index = codename_index(input, position, salt, "adjective", adjectives.len());
            let word = adjectives[index];
            if !parts.contains(&word) || salt >= adjectives.len() {
                parts.push(word);
                break;
            }
            salt += 1;
        }
    }

    let index = codename_index(input, adjective_count, 0, "noun", nouns.len());
    parts.push(nouns[index]);
    parts.join("-")
}

fn invalid_filter_arg(message: impl Into<String>) -> minijinja::Error {
    minijinja::Error::new(ErrorKind::InvalidOperation, message.into())
}

fn codename_filter(value: Value, words: Option<usize>) -> Result<String, minijinja::Error> {
    let words = words.unwrap_or(2);
    if words == 0 || words > CODENAME_MAX_WORDS {
        return Err(invalid_filter_arg(format!(
            "codename word count must be between 1 and {CODENAME_MAX_WORDS}"
        )));
    }
    Ok(codename(value.as_str().unwrap_or_default(), words))
}

/// Redact credentials from URLs for safe logging.
///
/// URLs with embedded credentials (e.g., `https://token@github.com/...`) have
/// the credential portion replaced with `[REDACTED]`.
///
/// # Examples
/// ```
/// use worktrunk::config::redact_credentials;
///
/// // URLs with credentials are redacted
/// assert_eq!(
///     redact_credentials("https://ghp_token123@github.com/owner/repo"),
///     "https://[REDACTED]@github.com/owner/repo"
/// );
///
/// // URLs without credentials are unchanged
/// assert_eq!(
///     redact_credentials("https://github.com/owner/repo"),
///     "https://github.com/owner/repo"
/// );
///
/// // Non-URL values pass through unchanged
/// assert_eq!(redact_credentials("main"), "main");
/// ```
pub fn redact_credentials(s: &str) -> String {
    // Pattern: scheme://credentials@host where credentials don't contain @
    // This matches URLs like https://token@github.com or https://user:pass@host.com
    thread_local! {
        static CREDENTIAL_URL: Regex = Regex::new(r"^([a-z][a-z0-9+.-]*://)([^@/]+)@").unwrap();
    }
    CREDENTIAL_URL.with(|re| re.replace(s, "${1}[REDACTED]@").into_owned())
}

/// Error from template expansion with rich context for diagnostics.
///
/// Produced by [`expand_template`] when a template fails to parse or render.
/// Contains structured data for styled display in `main.rs` (via downcast)
/// and a `message` field for callers that embed errors in other output.
#[derive(Debug)]
pub struct TemplateExpandError {
    /// Plain-text error summary for callers that embed errors in styled messages.
    pub message: String,
    /// The failing template line (if identifiable).
    pub source_line: Option<String>,
    /// Variable names available in this template context.
    pub available_vars: Vec<String>,
}

impl std::fmt::Display for TemplateExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Diagnostic for TemplateExpandError {
    fn render(&self) -> String {
        let mut parts = vec![error_message(&self.message).to_string()];
        if let Some(ref line) = self.source_line {
            parts.push(format_with_gutter(line, None));
        }
        if !self.available_vars.is_empty() {
            let underlined_vars: Vec<String> = self
                .available_vars
                .iter()
                .map(|v| cformat!("<underline>{}</>", v))
                .collect();
            parts.push(
                hint_message(cformat!(
                    "Available variables: {}",
                    underlined_vars.join(", ")
                ))
                .to_string(),
            );
        }
        parts.join("\n")
    }
}

impl std::error::Error for TemplateExpandError {}

/// Build a [`TemplateExpandError`] from a minijinja error, the original template
/// source, the template name (for error messages), and the available variable names.
///
/// Message format: `Failed to expand {name}: {kind}[: {detail}] [@ line {n}]`
///
/// ```text
/// Failed to expand {name}: {kind}[: {detail}] [@ line {n}]
/// │                 │        │       │              │
/// │                 │        │       │              └─ e.line() from minijinja
/// │                 │        │       └─ e.detail() from minijinja (None for UndefinedError)
/// │                 │        └─ e.kind() from minijinja ("undefined value", "syntax error")
/// │                 └─ `name` param passed by caller
/// └─ hardcoded prefix
/// ```
fn build_template_error(
    e: &minijinja::Error,
    template: &str,
    name: &str,
    available_vars: Vec<String>,
) -> TemplateExpandError {
    let lines: Vec<&str> = template.lines().collect();
    let line_num = e.line();
    let source_line =
        line_num.and_then(|n| lines.get(n.saturating_sub(1)).copied().map(String::from));

    // Build message: "Failed to expand {name}: {kind}[: {detail}] [@ line {n}]"
    // e.g. "Failed to expand --execute command: undefined value @ line 1"
    let detail = match e.detail() {
        Some(detail) => format!("{}: {detail}", e.kind()),
        None => e.kind().to_string(),
    };
    let is_undefined = e.kind() == ErrorKind::UndefinedError;

    // minijinja always provides a line number for syntax and render errors
    let message = match line_num {
        Some(n) => format!("Failed to expand {name}: {detail} @ line {n}"),
        None => format!("Failed to expand {name}: {detail}"),
    };

    TemplateExpandError {
        message,
        source_line,
        // Only show available vars for undefined errors (actionable hint)
        available_vars: if is_undefined {
            available_vars
        } else {
            Vec::new()
        },
    }
}

fn sorted_available_vars(vars: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = vars.iter().map(|k| k.to_string()).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn build_undefined_vars_error(
    name: &str,
    undefined_vars: &[String],
    available_vars: Vec<String>,
) -> TemplateExpandError {
    let names = undefined_vars
        .iter()
        .map(|var| format!("`{var}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if undefined_vars.len() == 1 {
        "value"
    } else {
        "values"
    };

    TemplateExpandError {
        message: format!("Failed to expand {name}: undefined {noun}: {names}"),
        source_line: None,
        available_vars,
    }
}

/// Set up a minijinja environment with worktrunk's custom filters and functions.
///
/// Shared by [`expand_template`], [`validate_template`], and the `wt list`
/// custom-column renderer (which holds one environment across all rows) so
/// every template user gets the same filters, functions, and
/// undefined-behavior settings.
pub fn template_environment(repo: &Repository) -> Environment<'static> {
    let mut env = Environment::new();
    // SemiStrict: errors on undefined variable use (printing, iteration) but allows
    // truthiness checks ({% if var %}). This catches typos while supporting optional vars.
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);

    // Register custom filters
    env.add_filter("sanitize", |value: Value| -> String {
        sanitize_branch_name(value.as_str().unwrap_or_default())
    });
    env.add_filter("sanitize_db", |value: Value| -> String {
        sanitize_db(value.as_str().unwrap_or_default())
    });
    env.add_filter("sanitize_hash", |value: Value| -> String {
        crate::path::sanitize_for_filename(value.as_str().unwrap_or_default())
    });
    env.add_filter("hash", |value: Value| -> String {
        short_hash(value.as_str().unwrap_or_default())
    });
    env.add_filter("hash_port", |value: String| string_to_port(&value));
    env.add_filter("dirname", |value: Value| -> String {
        std::path::Path::new(value.as_str().unwrap_or_default())
            .parent()
            .map(|p| to_posix_path(&p.to_string_lossy()))
            .unwrap_or_default()
    });
    env.add_filter("basename", |value: Value| -> String {
        std::path::Path::new(value.as_str().unwrap_or_default())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    env.add_filter("codename", codename_filter);

    // Register worktree_path_of_branch function for looking up branch worktree paths.
    // Returns raw paths — shell escaping is applied by the formatter at output time.
    let repo_clone = repo.clone();
    env.add_function("worktree_path_of_branch", move |branch: String| -> String {
        repo_clone
            .worktree_for_branch(&branch)
            .ok()
            .flatten()
            .map(|p| to_posix_path(&p.to_string_lossy()))
            .unwrap_or_default()
    });

    env
}

/// Top-level variables referenced by a single template.
///
/// Uses minijinja's AST analysis rather than string matching, avoiding false
/// positives from literal text like `template_vars.txt`. Templates that fail
/// to parse contribute nothing — a syntax error surfaces later at expansion
/// time with a richer message.
fn referenced_vars(template: &str) -> std::collections::HashSet<String> {
    minijinja::Environment::new()
        .template_from_str(template)
        .map(|tmpl| tmpl.undeclared_variables(false))
        .unwrap_or_default()
}

/// Check if a template references a specific top-level variable.
pub fn template_references_var(template: &str, var: &str) -> bool {
    referenced_vars(template).contains(var)
}

/// Union of top-level variables referenced across every command in `cfg`.
///
/// Drives alias-arg routing in `AliasOptions::parse`: a `--KEY=VALUE` token
/// binds to `{{ KEY }}` only when KEY appears in this set; otherwise it
/// forwards as a positional. A var referenced in any step of a pipeline is
/// a binding candidate for the whole invocation. A syntax error in any
/// template fails here so the user sees it before flags are routed — a
/// silent skip could mask a typo and change how subsequent CLI args bind.
/// `name` labels the failing alias or hook in the error, which matches
/// [`expand_template`]'s parse-failure shape so syntax errors render
/// identically wherever they surface.
pub fn referenced_vars_for_config(
    cfg: &super::CommandConfig,
    name: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let env = minijinja::Environment::new();
    let mut out = BTreeSet::new();
    for cmd in cfg.commands() {
        let tmpl = env
            .template_from_str(&cmd.template)
            .map_err(|e| build_template_error(&e, &cmd.template, name, Vec::new()))?;
        out.extend(tmpl.undeclared_variables(false));
    }
    Ok(out)
}

/// Parse-only syntax check for a template.
///
/// Hook and alias preparation runs this on every template so syntax errors
/// (e.g. `{{ vars..foo }}`) abort before the first pipeline step runs;
/// rendering — and with it semantic errors like undefined variables — is
/// deferred to execution time. The error matches [`expand_template`]'s
/// parse-failure shape so syntax errors render identically wherever they
/// surface.
pub fn validate_template_syntax(template: &str, name: &str) -> Result<(), TemplateExpandError> {
    minijinja::Environment::new()
        .template_from_named_str(name, template)
        .map(|_| ())
        .map_err(|e| build_template_error(&e, template, name, Vec::new()))
}

/// Validate that a template can be expanded without errors in the given scope.
///
/// Performs a trial expansion with placeholder values for exactly the variables
/// available in `scope` (see [`vars_available_in`]). Catches syntax errors and
/// undefined variable references *before* irreversible operations like worktree
/// creation — including context-mismatch typos like `{{ args }}` in a hook or
/// `{{ target }}` in a `pre-start` hook.
///
/// This is deliberately more permissive than real expansion: conditional vars
/// like `upstream` are provided even when they may be absent at runtime. A
/// template like `{{ upstream }}` passes validation but could fail later if
/// tracking isn't configured — the alternative (predicting which optional
/// variables will be available) would be fragile and context-dependent.
///
/// No verbose logging is performed — this is a pre-flight check, not the real expansion.
pub fn validate_template(
    template: &str,
    scope: ValidationScope,
    repo: &Repository,
    name: &str,
) -> Result<(), TemplateExpandError> {
    let available = vars_available_in(scope);
    let mut context: HashMap<String, minijinja::Value> = available
        .iter()
        .filter(|&&k| k != ALIAS_ARGS_KEY)
        .map(|&k| (k.to_string(), minijinja::Value::from("PLACEHOLDER")))
        .collect();
    // Inject vars as empty map so {{ vars.key | default(...) }} doesn't error
    context.insert(
        "vars".to_string(),
        minijinja::Value::from_serialize(std::collections::BTreeMap::<String, String>::new()),
    );
    // In alias and hook scopes, inject `args` as an empty sequence so
    // `{{ args }}`, `{{ args[0] | default(...) }}`, `{{ args | length }}`,
    // and `{% for a in args %}…{% endfor %}` all validate.
    if matches!(scope, ValidationScope::Alias | ValidationScope::Hook(_)) {
        context.insert(
            ALIAS_ARGS_KEY.to_string(),
            Value::from_object(ShellArgs::new(Vec::new())),
        );
    }

    let env = template_environment(repo);

    let tmpl = env
        .template_from_named_str(name, template)
        .map_err(|e| build_template_error(&e, template, name, Vec::new()))?;

    let mut allowed: BTreeSet<String> = available.iter().map(|k| k.to_string()).collect();
    allowed.insert("vars".to_string());
    let mut undefined: Vec<String> = tmpl
        .undeclared_variables(false)
        .into_iter()
        .filter(|var| !allowed.contains(var))
        .collect();
    undefined.sort();
    if !undefined.is_empty() {
        return Err(build_undefined_vars_error(
            name,
            &undefined,
            sorted_available_vars(&available),
        ));
    }

    tmpl.render(minijinja::Value::from_object(context))
        .map_err(|e| build_template_error(&e, template, name, sorted_available_vars(&available)))?;

    Ok(())
}

/// Expand a template with variable substitution.
///
/// # Arguments
/// * `template` - Template string using Jinja2 syntax (e.g., `{{ branch }}`)
/// * `vars` - Variables to substitute
/// * `escape_mode` - How to escape interpolated values:
///   - [`ShellEscapeMode::Posix`] / [`ShellEscapeMode::PowerShell`] — escape
///     for safe splicing into a command line of that shell. Callers that feed
///     the result to `Cmd::shell` (hooks, aliases) always pass `Posix`; only
///     the `--execute` payload, parsed by the active directive shell, may pass
///     `PowerShell`.
///   - [`ShellEscapeMode::Literal`] — substitute values verbatim (filesystem
///     paths).
/// * `repo` - Repository for looking up worktree paths
///
/// # Filters
/// - `sanitize` — Replace `/` and `\` with `-` for filesystem-safe paths
/// - `sanitize_db` — Transform to database-safe identifier (`[a-z0-9_]`, max 48 chars)
/// - `sanitize_hash` — Filesystem-safe name with hash suffix so distinct inputs never collide
/// - `hash` — 3-character base36 hash digest of the input
/// - `hash_port` — Hash to deterministic port number (10000-19999)
/// - `dirname` — Strip the last path component (e.g., `/a/b/c` → `/a/b`)
/// - `basename` — Keep only the last path component (e.g., `/a/b/c` → `c`)
/// - `codename(n)` — deterministic friendly words, e.g. `malleable-opah`
///
/// # Functions
/// - `worktree_path_of_branch(branch)` — Look up the filesystem path of a branch's worktree
///   Returns empty string if branch has no worktree.
///
/// The `name` parameter appears in error messages to help identify which template failed.
pub fn expand_template(
    template: &str,
    vars: &HashMap<&str, &str>,
    escape_mode: ShellEscapeMode,
    repo: &Repository,
    name: &str,
) -> Result<String, TemplateExpandError> {
    // Build context map with raw values (shell escaping is applied at output time via formatter).
    // The `args` key is reserved: run_alias encodes positional CLI args as a JSON list string,
    // and we rehydrate it here as a `ShellArgs` object so `{{ args }}` behaves sequence-like.
    let mut context = HashMap::new();
    for (key, value) in vars {
        if *key == ALIAS_ARGS_KEY {
            let parsed: Vec<String> = serde_json::from_str(value).unwrap_or_default();
            context.insert(key.to_string(), Value::from_object(ShellArgs::new(parsed)));
        } else {
            context.insert(
                key.to_string(),
                minijinja::Value::from((*value).to_string()),
            );
        }
    }

    let mut env = template_environment(repo);
    if escape_mode != ShellEscapeMode::Literal {
        // Preserve trailing newlines in templates (important for multiline shell commands)
        env.set_keep_trailing_newline(true);

        // Shell-escape values at output time, not before template rendering.
        // This ensures filters (sanitize, sanitize_db, etc.) operate on raw values
        // and the escaping is applied to the final output, preventing corruption
        // when filters modify already-escaped strings.
        env.set_formatter(move |out, _state, value| {
            if value.is_none() {
                return Ok(());
            }
            // ShellArgs renders each element pre-escaped and space-joined
            // (see [`ShellArgs::render`]); passing through its Display
            // output avoids re-escaping the whole joined string as one
            // opaque token. Iteration and indexing yield plain string
            // values that still flow through the generic escape branch.
            if value.downcast_object_ref::<ShellArgs>().is_some() {
                write!(out, "{value}")?;
                return Ok(());
            }
            let escaped = shell_escape_for(escape_mode, &value.to_string());
            write!(out, "{escaped}")?;
            Ok(())
        });
    }

    // Cache verbosity level for consistent behavior within this call
    let verbose = verbosity();

    // -vv: Full debug logging with vars
    // Redact credentials from values to prevent leaking tokens in logs
    if verbose >= 2 {
        tracing::debug!(name = %name, template = ?template, "[template:{name}] template={template:?}");
        // Sort keys for deterministic output in tests
        let mut sorted_vars: Vec<_> = vars.iter().collect();
        sorted_vars.sort_by_key(|(k, _)| *k);
        tracing::debug!(
            name = %name,
            "[template:{name}] vars={{{}}}",
            sorted_vars
                .iter()
                .map(|(k, v)| format!("{k}={:?}", redact_credentials(v)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Parse errors are always SyntaxError, never UndefinedError — no need for available_vars
    let tmpl = env
        .template_from_named_str(name, template)
        .map_err(|e| build_template_error(&e, template, name, Vec::new()))?;

    // Inject vars data as a nested object: {{ vars.env }}, {{ vars["env"] }},
    // {{ vars.config.port }}. When branch is present, always inject (even if
    // empty map) so {{ vars.key | default(...) }} works in SemiStrict mode.
    // Only look up vars data if the parsed template references the top-level
    // `vars` object (avoids a git process spawn per expansion while supporting
    // every MiniJinja access form without false positives from literal text).
    if tmpl.undeclared_variables(false).contains("vars")
        && let Some(branch) = vars.get("branch")
    {
        context.insert(
            "vars".to_string(),
            vars_map_to_value(&repo.vars_entries(branch)),
        );
    }

    let result = tmpl
        .render(minijinja::Value::from_object(context))
        .map_err(|e| {
            let mut keys: Vec<String> = vars.keys().map(|k| k.to_string()).collect();
            keys.sort();
            build_template_error(&e, template, name, keys)
        })?;

    // -vv: Full debug logging with result
    // Redact credentials from result to prevent leaking tokens in logs
    if verbose >= 2 {
        tracing::debug!(name = %name, "[template:{name}] result={:?}", redact_credentials(&result));
    }

    // -v: Nice styled output showing template expansion. The source template
    // and the rendered result each get a labeled info header above a bash gutter
    // block, so the eye separates input from output. Callers that emit several
    // expansions in a row (or a single standalone one, like `wt step eval`) add
    // a trailing blank to separate them; in a command pipeline the executor
    // already provides that separation.
    // Single atomic write to avoid interleaving in multi-threaded execution
    if verbose == 1 {
        let source_header = info_message(cformat!("<bold>{name}</> source"));
        let source_gutter = format_bash_with_gutter(template);
        let result_header = info_message(cformat!("<bold>{name}</> result"));
        let result_gutter = format_bash_with_gutter(&result);
        eprintln!("{source_header}\n{source_gutter}\n{result_header}\n{result_gutter}");
    }
    Ok(result)
}

/// Convert raw vars entries into a minijinja object value.
///
/// JSON objects/arrays are parsed so nested access (`{{ vars.config.port }}`)
/// works; plain strings and numbers stay as-is.
pub fn vars_map_to_value(entries: &std::collections::BTreeMap<String, String>) -> Value {
    let vars_map: std::collections::BTreeMap<&str, Value> = entries
        .iter()
        .map(|(k, v)| {
            let value = serde_json::from_str::<serde_json::Value>(v)
                .ok()
                .filter(|j| j.is_object() || j.is_array())
                .map(|j| Value::from_serialize(&j))
                .unwrap_or_else(|| Value::from(v.clone()));
            (k.as_str(), value)
        })
        .collect();
    Value::from_serialize(&vars_map)
}

/// A stand-in for the `vars` / `git.branch` maps that answers every key
/// with a placeholder string.
///
/// Used only by [`validate_list_column_template`]'s trial render: list
/// columns lean on `{{ vars.key }}` / `{{ git.branch.key }}` for keys that
/// only some branches set, so validating against an empty map (as
/// [`validate_template`] does) would wrongly reject the dominant pattern.
/// Answering every key keeps the trial render exercising filters without
/// constraining which keys exist.
#[derive(Debug)]
struct AnyKeyVars;

impl Object for AnyKeyVars {
    fn get_value(self: &Arc<Self>, _key: &Value) -> Option<Value> {
        Some(Value::from("PLACEHOLDER"))
    }
}

/// Validate a `wt list` custom-column template.
///
/// Checks syntax, restricts top-level variables to `LIST_COLUMN_VARS` ∪
/// `{vars, git}`, and trial-renders with placeholder values so filter errors
/// (e.g. a misspelled filter name) surface at config resolution rather than as
/// silently empty cells. `vars.*` and `git.branch.*` access is unconstrained —
/// any key validates, since which keys exist is per-branch runtime state.
pub fn validate_list_column_template(
    template: &str,
    repo: &Repository,
    name: &str,
) -> Result<(), TemplateExpandError> {
    let env = template_environment(repo);
    let tmpl = env
        .template_from_named_str(name, template)
        .map_err(|e| build_template_error(&e, template, name, Vec::new()))?;

    let mut undefined: Vec<String> = tmpl
        .undeclared_variables(false)
        .into_iter()
        .filter(|var| var != "vars" && var != "git" && !LIST_COLUMN_VARS.contains(&var.as_str()))
        .collect();
    undefined.sort();
    if !undefined.is_empty() {
        let mut available = LIST_COLUMN_VARS.to_vec();
        available.push("vars.<key>");
        available.push("git.branch.<key>");
        return Err(build_undefined_vars_error(
            name,
            &undefined,
            sorted_available_vars(&available),
        ));
    }

    let mut context: HashMap<String, Value> = LIST_COLUMN_VARS
        .iter()
        .map(|&k| (k.to_string(), Value::from("PLACEHOLDER")))
        .collect();
    context.insert("vars".to_string(), Value::from_object(AnyKeyVars));
    // git.branch.<key> resolves to a placeholder; nesting AnyKeyVars under
    // `branch` lets the trial render exercise `{{ git.branch.* }}` filters.
    let git = HashMap::from([("branch".to_string(), Value::from_object(AnyKeyVars))]);
    context.insert("git".to_string(), Value::from_object(git));
    match tmpl.render(Value::from_object(context)) {
        Ok(_) => Ok(()),
        // At runtime an undefined value renders as an empty cell by design
        // (e.g. nested access like `{{ vars.config.port }}` against the flat
        // placeholder), so it is never a config error here. Everything else —
        // unknown filters, type errors — is a real config problem.
        Err(e) if e.kind() == ErrorKind::UndefinedError => Ok(()),
        Err(e) => Err(build_template_error(
            &e,
            template,
            name,
            sorted_available_vars(LIST_COLUMN_VARS),
        )),
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;
    use crate::shell_exec::Cmd;
    use crate::testing::TestRepo;

    fn test_repo() -> TestRepo {
        TestRepo::new()
    }

    #[test]
    fn test_sanitize_branch_name() {
        let cases = [
            ("feature/foo", "feature-foo"),
            (r"user\task", "user-task"),
            ("feature/user/task", "feature-user-task"),
            (r"feature/user\task", "feature-user-task"),
            ("simple-branch", "simple-branch"),
            ("", ""),
            ("///", "---"),
            ("/feature", "-feature"),
            ("feature/", "feature-"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_branch_name(input), expected, "input: {input}");
        }
    }

    #[test]
    fn test_sanitize_db() {
        // Test that base transformations are correct (ignore hash suffix)
        let cases = [
            // Examples from spec
            ("feature/auth-oauth2", "feature_auth_oauth2_"),
            ("123-bug-fix", "_123_bug_fix_"),
            ("UPPERCASE.Branch", "uppercase_branch_"),
            // Lowercase conversion
            ("MyBranch", "mybranch_"),
            ("ALLCAPS", "allcaps_"),
            // Non-alphanumeric replacement
            ("feature/foo", "feature_foo_"),
            ("feature-bar", "feature_bar_"),
            ("feature.baz", "feature_baz_"),
            ("feature@qux", "feature_qux_"),
            // Consecutive underscore collapse
            ("a--b", "a_b_"),
            ("a///b", "a_b_"),
            ("a...b", "a_b_"),
            ("a-/-b", "a_b_"),
            // Leading digit prefix
            ("1branch", "_1branch_"),
            ("123", "_123_"),
            ("0test", "_0test_"),
            // No prefix needed
            ("branch1", "branch1_"),
            ("_already", "_already_"),
            // Edge cases (non-empty)
            ("a", "a_"),
            // Mixed cases
            ("Feature/Auth-OAuth2", "feature_auth_oauth2_"),
            ("user/TASK/123", "user_task_123_"),
            // Non-ASCII characters become underscores
            ("naïve-impl", "na_ve_impl_"),
            ("über-feature", "_ber_feature_"),
        ];
        for (input, expected_prefix) in cases {
            let result = sanitize_db(input);
            assert!(
                result.starts_with(expected_prefix),
                "input: {input}, expected prefix: {expected_prefix}, got: {result}"
            );
            // Result should be prefix + 3-char hash
            assert_eq!(
                result.len(),
                expected_prefix.len() + 3,
                "input: {input}, result: {result}"
            );
        }

        // Empty input stays empty (no hash suffix)
        assert_eq!(sanitize_db(""), "");

        // Special cases that collapse to just underscore + hash
        for input in ["_", "-", "---", "日本語"] {
            let result = sanitize_db(input);
            assert!(result.starts_with('_'), "input: {input}, got: {result}");
            assert_eq!(result.len(), 4, "input: {input}, got: {result}"); // _xxx
        }
    }

    #[test]
    fn test_sanitize_db_collision_avoidance() {
        // Different inputs that would collide without hash suffix now differ
        assert_ne!(sanitize_db("a-b"), sanitize_db("a_b"));
        assert_ne!(sanitize_db("feature/auth"), sanitize_db("feature-auth"));
        assert_ne!(sanitize_db("UPPERCASE"), sanitize_db("uppercase"));

        // Same input always produces same output (deterministic)
        assert_eq!(sanitize_db("test"), sanitize_db("test"));
        assert_eq!(sanitize_db("feature/foo"), sanitize_db("feature/foo"));
    }

    #[test]
    fn test_sanitize_db_reserved_words() {
        // Reserved words get hash suffix, making them safe
        let user = sanitize_db("user");
        assert!(user.starts_with("user_"), "got: {user}");
        assert_ne!(user, "user"); // Not a bare reserved word

        let select = sanitize_db("select");
        assert!(select.starts_with("select_"), "got: {select}");
        assert_ne!(select, "select");
    }

    #[test]
    fn test_sanitize_db_truncation() {
        // Total output is always max 48 characters
        // Base is truncated to 44 chars, then _xxx suffix (4 chars) is added

        // Very long input: base truncated to 44, + 4 = 48
        let long_input = "a".repeat(100);
        let result = sanitize_db(&long_input);
        assert_eq!(result.len(), 48, "result: {result}");
        assert!(result.starts_with(&"a".repeat(43)), "result: {result}");
        assert!(!result.ends_with('_'), "should end with hash chars");

        // Short input: base + _ + hash
        let short = "test";
        let result = sanitize_db(short);
        assert!(result.starts_with("test_"), "result: {result}");
        assert_eq!(result.len(), 8, "result: {result}"); // test_ + 3 hash chars

        // Truncation happens after prefix is added for digit-starting inputs
        let digit_start = format!("1{}", "x".repeat(100));
        let result = sanitize_db(&digit_start);
        assert_eq!(result.len(), 48, "result: {result}");
        assert!(result.starts_with("_1"), "result: {result}");
    }

    #[test]
    fn test_expand_template_basic() {
        let test = test_repo();

        // Single variable
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        assert_eq!(
            expand_template(
                "Hello {{ name }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "Hello world"
        );

        // Multiple variables
        vars.insert("repo", "myrepo");
        assert_eq!(
            expand_template(
                "{{ repo }}/{{ name }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "myrepo/world"
        );

        // Empty/static cases
        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            expand_template("", &empty, ShellEscapeMode::Literal, &test.repo, "test").unwrap(),
            ""
        );
        assert_eq!(
            expand_template(
                "static text",
                &empty,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "static text"
        );
        // Undefined variables now error in SemiStrict mode
        let err = expand_template(
            "no {{ variables }} here",
            &empty,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn test_expand_template_shell_escape() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("path", "my path");
        let expanded = expand_template(
            "cd {{ path }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(expanded.contains("'my path'") || expanded.contains(r"my\ path"));

        // Command injection prevention
        vars.insert("arg", "test;rm -rf");
        let expanded = expand_template(
            "echo {{ arg }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(!expanded.contains(";rm") || expanded.contains("'"));

        // No escape for literal mode
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_errors() {
        let test = test_repo();
        let vars = HashMap::new();
        let err = expand_template(
            "{{ unclosed",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(err.message.contains("syntax error"), "got: {}", err.message);
        assert!(
            expand_template(
                "{{ 1 + }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .is_err()
        );

        // Diagnostic::render produces source line but no available vars hint for syntax errors
        assert_snapshot!(crate::git::Diagnostic::render(&err), @"
        [31m✗[39m [31mFailed to expand test: syntax error: unexpected end of input, expected end of variable block @ line 1[39m
        [107m [0m {{ unclosed
        ");
    }

    #[test]
    fn test_expand_template_undefined_var_details() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "main");
        vars.insert("remote", "origin");

        let err = expand_template(
            "echo {{ target }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "should mention undefined value: {}",
            err.message
        );
        assert!(err.available_vars.contains(&"branch".to_string()));
        assert!(err.available_vars.contains(&"remote".to_string()));
        assert_eq!(err.source_line.as_deref(), Some("echo {{ target }}"));

        // Diagnostic::render produces source line and available vars hint
        assert_snapshot!(crate::git::Diagnostic::render(&err), @"
        [31m✗[39m [31mFailed to expand test: undefined value @ line 1[39m
        [107m [0m echo {{ target }}
        [2m↳[22m [2mAvailable variables: [4mbranch[24m, [4mremote[24m[22m
        ");
    }

    #[test]
    fn test_expand_template_jinja_features() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("debug", "true");
        assert_eq!(
            expand_template(
                "{% if debug %}DEBUG{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "DEBUG"
        );

        vars.insert("debug", "");
        assert_eq!(
            expand_template(
                "{% if debug %}DEBUG{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            ""
        );

        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            expand_template(
                "{{ missing | default('fallback') }}",
                &empty,
                ShellEscapeMode::Literal,
                &test.repo,
                "test",
            )
            .unwrap(),
            "fallback"
        );

        vars.insert("name", "hello");
        assert_eq!(
            expand_template(
                "{{ name | upper }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "HELLO"
        );
    }

    #[test]
    fn test_expand_template_strip_prefix() {
        let test = test_repo();
        let mut vars = HashMap::new();

        // Built-in replace filter strips prefix (replaces all occurrences)
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "foo"
        );

        // Replace + sanitize for worktree paths
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') | sanitize }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "foo"
        );

        // Branch without prefix passes through unchanged
        vars.insert("branch", "main");
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "main"
        );

        // Slicing for prefix-only removal (avoids replacing mid-string)
        vars.insert("branch", "feature/nested/feature/deep");
        assert_eq!(
            expand_template(
                "{{ branch[8:] }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "nested/feature/deep"
        );

        // Conditional slicing for safe prefix removal
        assert_eq!(
            expand_template(
                "{% if branch[:8] == 'feature/' %}{{ branch[8:] }}{% else %}{{ branch }}{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "nested/feature/deep"
        );

        // Conditional passes through non-matching branches
        vars.insert("branch", "bugfix/bar");
        assert_eq!(
            expand_template(
                "{% if branch[:8] == 'feature/' %}{{ branch[8:] }}{% else %}{{ branch }}{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "bugfix/bar"
        );
    }

    #[test]
    fn test_expand_template_sanitize_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch | sanitize }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "feature-foo"
        );

        // Backslashes are also sanitized
        vars.insert("branch", r"feature\bar");
        assert_eq!(
            expand_template(
                "{{ branch | sanitize }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "feature-bar"
        );

        // Multiple slashes
        vars.insert("branch", "user/feature/task");
        assert_eq!(
            expand_template(
                "{{ branch | sanitize }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "user-feature-task"
        );

        // Raw branch is unchanged
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "feature/foo"
        );

        // Shell escaping + sanitize: filters operate on raw values, escaping applied at output.
        // Previously, shell escaping was applied BEFORE filters, corrupting the result
        // when values contained shell-special characters (quotes, backslashes).
        vars.insert("branch", "user's/feature");
        let result = expand_template(
            "{{ branch | sanitize }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        // sanitize replaces / with -, producing "user's-feature"
        // shell_escape wraps it: 'user'\''s-feature' (valid shell for user's-feature)
        assert_eq!(result, r"'user'\''s-feature'", "sanitize + shell escape");

        // Without the fix, pre-escaping would produce corrupted output because
        // sanitize would replace the / and \ in the already-escaped value.

        // Shell escaping without filter: raw value with special chars
        let result = expand_template(
            "{{ branch }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        // shell_escape wraps: 'user'\''s/feature' (valid shell for user's/feature)
        assert_eq!(
            result, r"'user'\''s/feature'",
            "shell escape without filter"
        );

        // Shell-escape formatter handles none values (renders as empty string)
        let result = expand_template(
            "prefix-{{ none }}-suffix",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "prefix--suffix", "none renders as empty");
    }

    #[test]
    fn test_expand_template_sanitize_db_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();

        // Basic transformation (with hash suffix)
        vars.insert("branch", "feature/auth-oauth2");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("feature_auth_oauth2_"), "got: {result}");

        // Leading digit gets underscore prefix
        vars.insert("branch", "123-bug-fix");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("_123_bug_fix_"), "got: {result}");

        // Uppercase conversion
        vars.insert("branch", "UPPERCASE.Branch");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("uppercase_branch_"), "got: {result}");

        // Raw branch is unchanged
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_trailing_newline() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("cmd", "echo hello");
        assert!(
            expand_template(
                "{{ cmd }}\n",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap()
            .ends_with('\n')
        );
    }

    #[test]
    fn test_string_to_port_deterministic_and_in_range() {
        for input in ["main", "feature-foo", "", "a", "long-branch-name-123"] {
            let p1 = string_to_port(input);
            let p2 = string_to_port(input);
            assert_eq!(p1, p2, "same input should produce same port");
            assert!((10000..20000).contains(&p1), "port {} out of range", p1);
        }
    }

    #[test]
    fn test_hash_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/very-long-branch-name");

        // Filter produces a 3-char base36 digest
        let result = expand_template(
            "{{ branch | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result.len(), 3);
        assert!(
            result
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
            "got: {result}"
        );

        // Deterministic: same input produces same hash across calls
        let r1 = expand_template(
            "{{ branch | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let r2 = expand_template(
            "{{ branch | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(r1, r2);

        // Composable: hash reflects the upstream filter's output
        vars.insert("branch", "feature/auth");
        let raw = expand_template(
            "{{ branch | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let sanitized = expand_template(
            "{{ branch | sanitize | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        // `sanitize` rewrites `/` to `-`, so the hashed input differs and the digest does too.
        assert_ne!(raw, sanitized);

        // User-composed truncation + hash recipe (from extending docs)
        let truncated = expand_template(
            "{{ (branch | sanitize)[:8] }}_{{ branch | sanitize | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(truncated.starts_with("feature-"), "got: {truncated}");
        assert_eq!(truncated.len(), 8 + 1 + 3);

        // Empty input: still produces a 3-char digest (empty-string hash is stable)
        vars.insert("branch", "");
        let empty = expand_template(
            "{{ branch | hash }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(empty.len(), 3);
    }

    #[test]
    fn test_codename_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/very-long-branch-name");

        let default = expand_template(
            "{{ branch | codename }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let explicit_default = expand_template(
            "{{ branch | codename(2) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(default, explicit_default);
        assert_eq!(default.split('-').count(), 2, "got: {default}");

        let one_word = expand_template(
            "{{ branch | codename(1) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(!one_word.contains('-'), "got: {one_word}");
        let lists = codename_words();
        let nouns: &[&str] = lists.nouns.as_ref();
        assert!(nouns.contains(&one_word.as_str()));

        let three_words = expand_template(
            "{{ branch | codename(3) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(three_words.split('-').count(), 3, "got: {three_words}");

        let repeat = expand_template(
            "{{ branch | codename }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(default, repeat);

        vars.insert("branch", "feature/different-name");
        let other = expand_template(
            "{{ branch | codename }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(other.split('-').count(), 2, "got: {other}");

        vars.insert("branch", "feature/73");
        let ticket_73 = expand_template(
            "{{ branch | codename(3) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        vars.insert("branch", "feature/149");
        let ticket_149 = expand_template(
            "{{ branch | codename(3) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_ne!(ticket_73, ticket_149);
    }

    #[test]
    fn test_codename_filter_rejects_invalid_counts() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature");

        let zero = expand_template(
            "{{ branch | codename(0) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap_err()
        .to_string();
        assert!(
            zero.contains("codename word count must be between 1 and 8"),
            "got: {zero}"
        );

        let too_many = expand_template(
            "{{ branch | codename(9) }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap_err()
        .to_string();
        assert!(
            too_many.contains("codename word count must be between 1 and 8"),
            "got: {too_many}"
        );
    }

    #[test]
    fn test_codename_word_lists_are_path_safe() {
        fn assert_word_list(name: &str, words: &[&str]) {
            assert!(words.len() >= 160, "{name} should have enough entries");
            let mut seen = std::collections::HashSet::new();
            for &word in words {
                assert!(seen.insert(word), "duplicate {name} word: {word}");
                assert!(!word.is_empty(), "empty {name} word");
                assert!(
                    word.chars().all(|c| c.is_ascii_lowercase()),
                    "{name} word is not lowercase ASCII: {word}"
                );
                assert!(
                    !word.contains('-') && !word.contains(' '),
                    "{name} word is not a single path component fragment: {word}"
                );
            }
        }

        let lists = codename_words();
        let adjectives: &[&str] = lists.adjectives.as_ref();
        let nouns: &[&str] = lists.nouns.as_ref();
        assert_word_list("adjective", adjectives);
        assert_word_list("noun", nouns);
        // codename(2) cardinality — petname::medium() gives ~1.26M combinations,
        // so a single adjective+noun pair is enough on its own for typical
        // worktree counts.
        assert!(adjectives.len() * nouns.len() >= 1_000_000);
    }

    /// Pins specific `codename(input, n)` outputs so the on-disk contract
    /// is impossible to break by accident. Once a user adopts this filter in
    /// their `worktree-path` template, the petname wordlists (and the
    /// hash-input layout in `codename_index`) become an on-disk identity for
    /// every worktree they own. Anything that shifts the wordlists (a
    /// `petname` version bump that adds, removes, or reorders a word) or
    /// changes how the hash is computed produces a different name for the
    /// same branch on the next `wt switch`, orphaning the existing worktree.
    ///
    /// If this test fails:
    ///
    /// 1. Stop. Do not update the expected values to silence it.
    /// 2. Confirm whether you actually intended to change the wordlists
    ///    (e.g. via a `petname` version bump) or the hash layout. If it was
    ///    unintentional (a refactor, a Cargo update), revert.
    /// 3. If the change is intentional, accept that you are breaking every
    ///    existing user's worktree paths. Coordinate the rollout, document
    ///    it as a breaking change, and only then update the expected values.
    #[test]
    fn test_codename_outputs_are_stable() {
        let cases: &[(&str, usize, &str)] = &[
            ("main", 1, "gorilla"),
            ("feature/auth", 2, "malleable-opah"),
            ("feature/73", 2, "prodigious-shoveler"),
            ("feature/149", 2, "tuneful-vendace"),
            ("release/1.0", 3, "intent-equipped-treefrog"),
            (
                "hotfix/some-very-long-thing",
                4,
                "noteworthy-musical-durable-silkworm",
            ),
        ];

        for (input, n, expected) in cases {
            let actual = codename(input, *n);
            assert_eq!(
                &actual, expected,
                "\n\
                 codename({input:?}, {n}) returned {actual:?}, expected {expected:?}.\n\
                 \n\
                 Changing the petname version, or the codename algorithm,\n\
                 BREAKS every existing user's worktree paths derived from\n\
                 `{{{{ branch | codename(...) }}}}`. See the comment above\n\
                 this test before updating these expectations.\n"
            );
        }
    }

    #[test]
    fn test_validate_template_accepts_codename_filter() {
        let test = test_repo();
        assert!(
            validate_template(
                "{{ branch | codename(2) }}",
                ValidationScope::SwitchExecute,
                &test.repo,
                "test"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_hash_port_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature-foo");
        vars.insert("repo", "myrepo");

        // Filter produces a number in range
        let result = expand_template(
            "{{ branch | hash_port }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let port: u16 = result.parse().expect("should be a number");
        assert!((10000..20000).contains(&port));

        // Concatenation produces different (but deterministic) result
        let r1 = expand_template(
            "{{ (repo ~ '-' ~ branch) | hash_port }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let r1_port: u16 = r1.parse().expect("should be a number");
        let r2 = expand_template(
            "{{ (repo ~ '-' ~ branch) | hash_port }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        let r2_port: u16 = r2.parse().expect("should be a number");

        assert!((10000..20000).contains(&r1_port));
        assert!((10000..20000).contains(&r2_port));

        assert_eq!(r1, r2);
    }

    #[test]
    fn test_dirname_and_basename_filters() {
        let test = test_repo();
        let mut vars = HashMap::new();

        // Bare repo wrapped in a hidden dir: `dirname | basename` recovers the wrapper name
        // (the case from #1279 — `{{ repo }}` resolves to `.git`, but the user wants `myrepo`)
        vars.insert("repo_path", "/projects/myrepo/.git");
        let result = expand_template(
            "{{ repo_path | dirname | basename }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "myrepo");

        // Composing into a worktree-path template
        vars.insert("branch", "feature-auth");
        let result = expand_template(
            "{{ repo_path }}/../{{ repo_path | dirname | basename }}.{{ branch | sanitize }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "/projects/myrepo/.git/../myrepo.feature-auth");

        // `dirname` strips the last component
        vars.insert("repo_path", "/a/b/c");
        let dirname = expand_template(
            "{{ repo_path | dirname }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(dirname, "/a/b");

        // `basename` keeps only the last component
        let basename = expand_template(
            "{{ repo_path | basename }}",
            &vars,
            ShellEscapeMode::Literal,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(basename, "c");

        // No separator: dirname is empty, basename is the whole input
        vars.insert("repo_path", "myrepo");
        assert_eq!(
            expand_template(
                "{{ repo_path | dirname }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            ""
        );
        assert_eq!(
            expand_template(
                "{{ repo_path | basename }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "myrepo"
        );
    }

    #[test]
    fn test_redact_credentials_https_token() {
        // GitHub-style personal access token
        assert_eq!(
            redact_credentials("https://ghp_token123@github.com/owner/repo"),
            "https://[REDACTED]@github.com/owner/repo"
        );
        // GitLab-style token
        assert_eq!(
            redact_credentials("https://glpat-xxxxxxxxxxxx@gitlab.com/owner/repo.git"),
            "https://[REDACTED]@gitlab.com/owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_https_user_pass() {
        // Username:password format
        assert_eq!(
            redact_credentials("https://user:password123@github.com/owner/repo"),
            "https://[REDACTED]@github.com/owner/repo"
        );
    }

    #[test]
    fn test_redact_credentials_no_credentials() {
        // Normal HTTPS URL without credentials - unchanged
        assert_eq!(
            redact_credentials("https://github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
        // SSH URL - unchanged (no credentials in URL format)
        assert_eq!(
            redact_credentials("git@github.com:owner/repo.git"),
            "git@github.com:owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_non_url() {
        // Non-URL values pass through unchanged
        assert_eq!(redact_credentials("main"), "main");
        assert_eq!(redact_credentials("feature/auth"), "feature/auth");
        assert_eq!(redact_credentials("/path/to/worktree"), "/path/to/worktree");
        assert_eq!(redact_credentials(""), "");
    }

    #[test]
    fn test_redact_credentials_git_protocol() {
        // git:// protocol with credentials
        assert_eq!(
            redact_credentials("git://token@github.com/owner/repo.git"),
            "git://[REDACTED]@github.com/owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_preserves_path() {
        // Full URL with path and query should preserve everything after host
        assert_eq!(
            redact_credentials("https://token@github.com/owner/repo.git?ref=main"),
            "https://[REDACTED]@github.com/owner/repo.git?ref=main"
        );
    }

    #[test]
    fn test_expand_template_vars_data() {
        let test = test_repo();

        // Set vars data via git config
        Cmd::new("git")
            .args(["config", "worktrunk.state.main.vars.env", "staging"])
            .current_dir(test.path())
            .run()
            .unwrap();
        Cmd::new("git")
            .args(["config", "worktrunk.state.main.vars.port", "3000"])
            .current_dir(test.path())
            .run()
            .unwrap();

        let mut vars = HashMap::new();
        vars.insert("branch", "main");

        // Access vars via dot notation
        assert_eq!(
            expand_template(
                "{{ vars.env }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "staging"
        );
        assert_eq!(
            expand_template(
                r#"{{ vars["env"] }}"#,
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "staging"
        );
        assert_eq!(
            expand_template(
                "{% if vars %}vars loaded{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "vars loaded"
        );
        assert_eq!(
            expand_template(
                "{{ vars.port }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "3000"
        );

        // Default filter for missing vars keys
        assert_eq!(
            expand_template(
                "{{ vars.missing | default('fallback') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "fallback"
        );

        // Conditional on vars
        assert_eq!(
            expand_template(
                "{% if vars.env %}env={{ vars.env }}{% endif %}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "env=staging"
        );
    }

    #[test]
    fn test_expand_template_vars_json_dot_access() {
        let test = test_repo();

        // Store a JSON object as a vars value
        Cmd::new("git")
            .args([
                "config",
                "worktrunk.state.main.vars.config",
                r#"{"port": 3000, "debug": true}"#,
            ])
            .current_dir(test.path())
            .run()
            .unwrap();

        // Store a JSON array
        Cmd::new("git")
            .args([
                "config",
                "worktrunk.state.main.vars.tags",
                r#"["alpha", "beta"]"#,
            ])
            .current_dir(test.path())
            .run()
            .unwrap();

        // Store a plain string (not JSON)
        Cmd::new("git")
            .args(["config", "worktrunk.state.main.vars.env", "staging"])
            .current_dir(test.path())
            .run()
            .unwrap();

        let mut vars = HashMap::new();
        vars.insert("branch", "main");

        // Dot access into JSON object
        assert_eq!(
            expand_template(
                "{{ vars.config.port }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "3000"
        );
        assert_eq!(
            expand_template(
                "{{ vars.config.debug }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "true"
        );

        // Array index access
        assert_eq!(
            expand_template(
                "{{ vars.tags[0] }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "alpha"
        );

        // Plain string still works
        assert_eq!(
            expand_template(
                "{{ vars.env }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "staging"
        );

        // Default filter on missing nested key
        assert_eq!(
            expand_template(
                "{{ vars.config.missing | default('fallback') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "fallback"
        );
    }

    #[test]
    fn test_expand_template_vars_json_shell_escape() {
        let test = test_repo();

        Cmd::new("git")
            .args([
                "config",
                "worktrunk.state.main.vars.config",
                r#"{"name": "my project", "cmd": "echo hello"}"#,
            ])
            .current_dir(test.path())
            .run()
            .unwrap();

        let mut vars = HashMap::new();
        vars.insert("branch", "main");

        // Shell escaping should work on JSON-parsed nested values
        let result = expand_template(
            "{{ vars.config.name }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "'my project'");

        let result = expand_template(
            "{{ vars.config.cmd }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(result, "'echo hello'");
    }

    #[test]
    fn test_expand_template_vars_empty_when_no_branch() {
        let test = test_repo();
        let vars = HashMap::new(); // No branch var

        // vars should be undefined (no branch to look up)
        assert_eq!(
            expand_template(
                "{{ vars | default('none') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "none"
        );
    }

    #[test]
    fn test_expand_template_vars_empty_when_no_data() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "main");

        // vars injected as empty map when no entries exist — use default filter for missing keys
        assert_eq!(
            expand_template(
                "{{ vars.env | default('dev') }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "dev"
        );
    }

    #[test]
    fn test_expand_template_args_sequence() {
        let test = test_repo();
        let args_json = serde_json::to_string(&["foo", "bar baz", "qux"]).unwrap();
        let mut vars = HashMap::new();
        vars.insert("args", args_json.as_str());

        // Bare {{ args }} with shell escaping: space-joined, per-element escaped,
        // NOT wrapped in outer quotes as a single token.
        assert_eq!(
            expand_template(
                "wt switch {{ args }}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            "wt switch foo 'bar baz' qux"
        );

        // Indexing returns a plain string — flows through the shell-escape formatter.
        assert_eq!(
            expand_template(
                "{{ args[0] }}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            "foo"
        );
        assert_eq!(
            expand_template(
                "{{ args[1] }}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            "'bar baz'"
        );

        // Length works like any sequence.
        assert_eq!(
            expand_template(
                "{{ args | length }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "3"
        );

        // Iteration yields per-element string values; each escaped by the formatter.
        assert_eq!(
            expand_template(
                "{% for a in args %}[{{ a }}]{% endfor %}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            "[foo]['bar baz'][qux]"
        );
    }

    #[test]
    fn test_expand_template_args_empty() {
        let test = test_repo();
        let args_json = serde_json::to_string(&Vec::<String>::new()).unwrap();
        let mut vars = HashMap::new();
        vars.insert("args", args_json.as_str());

        // Empty args renders empty. No stray whitespace, no error.
        assert_eq!(
            expand_template(
                "wt switch{{ args }}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            "wt switch"
        );

        // Length still defined for empty.
        assert_eq!(
            expand_template(
                "{{ args | length }}",
                &vars,
                ShellEscapeMode::Literal,
                &test.repo,
                "test"
            )
            .unwrap(),
            "0"
        );

        // Iteration yields nothing.
        assert_eq!(
            expand_template(
                "{% for a in args %}X{% endfor %}",
                &vars,
                ShellEscapeMode::Posix,
                &test.repo,
                "test"
            )
            .unwrap(),
            ""
        );
    }

    #[test]
    fn test_expand_template_args_shell_metachar_safety() {
        // The point of ShellArgs is that bare {{ args }} is safe to splice into
        // a command line even when args contain shell metacharacters — each
        // element is individually POSIX single-quoted by `shell_join`, and the
        // outer formatter doesn't re-quote the joined result.
        let test = test_repo();
        let args_json = serde_json::to_string(&["; rm -rf /", "$(whoami)", "a'b"]).unwrap();
        let mut vars = HashMap::new();
        vars.insert("args", args_json.as_str());

        let rendered = expand_template(
            "echo {{ args }}",
            &vars,
            ShellEscapeMode::Posix,
            &test.repo,
            "test",
        )
        .unwrap();
        assert_eq!(rendered, r#"echo '; rm -rf /' '$(whoami)' 'a'\''b'"#);
    }

    #[test]
    fn test_validate_template_valid() {
        let test = test_repo();
        let hook = ValidationScope::Hook(HookType::PostCreate);

        // Static text
        assert!(validate_template("echo hello", hook, &test.repo, "test").is_ok());

        // Base variables are available in every scope
        assert!(validate_template("{{ branch }}", hook, &test.repo, "test").is_ok());
        assert!(validate_template("{{ repo }}/{{ branch }}", hook, &test.repo, "test").is_ok());

        // Filters
        assert!(validate_template("{{ branch | sanitize }}", hook, &test.repo, "test").is_ok());
        assert!(validate_template("{{ branch | sanitize_db }}", hook, &test.repo, "test").is_ok());
        assert!(
            validate_template("{{ branch | sanitize_hash }}", hook, &test.repo, "test").is_ok()
        );
        assert!(validate_template("{{ branch | hash }}", hook, &test.repo, "test").is_ok());
        assert!(validate_template("{{ branch | hash_port }}", hook, &test.repo, "test").is_ok());

        // Conditionals with optional vars
        assert!(
            validate_template(
                "{% if upstream %}{{ upstream }}{% endif %}",
                hook,
                &test.repo,
                "test"
            )
            .is_ok()
        );

        // Deprecated vars still valid in every scope
        assert!(validate_template("{{ main_worktree }}", hook, &test.repo, "test").is_ok());

        // `args` validates in both Hook and Alias scopes.
        assert!(validate_template("echo {{ args }}", hook, &test.repo, "test").is_ok());
        let alias = ValidationScope::Alias;
        assert!(validate_template("wt switch {{ args }}", alias, &test.repo, "test").is_ok());
        assert!(validate_template("{{ args | length }}", alias, &test.repo, "test").is_ok());
        assert!(
            validate_template(
                "{% for a in args %}{{ a }}{% endfor %}",
                alias,
                &test.repo,
                "test"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_template_scope_rejects_out_of_scope_vars() {
        let test = test_repo();

        // `base` is unavailable in pre-merge — catch the typo at validation time.
        let err = validate_template(
            "{{ base }}",
            ValidationScope::Hook(HookType::PreMerge),
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );

        // `base` is available in pre-start.
        assert!(
            validate_template(
                "{{ base }}",
                ValidationScope::Hook(HookType::PreCreate),
                &test.repo,
                "test"
            )
            .is_ok()
        );

        // `target` is available in pre-merge.
        assert!(
            validate_template(
                "{{ target }}",
                ValidationScope::Hook(HookType::PreMerge),
                &test.repo,
                "test"
            )
            .is_ok()
        );

        // Typos in conditional predicates must not be hidden by SemiStrict truthiness.
        let err = validate_template(
            "{% if targte %}echo {{ target }}{% endif %}",
            ValidationScope::Hook(HookType::PreMerge),
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );
        assert!(err.message.contains("targte"), "got: {}", err.message);

        // `pr_number`/`pr_url` are available in pre-start (populated when
        // creating via `pr:N` / `mr:N`).
        for var in ["pr_number", "pr_url"] {
            assert!(
                validate_template(
                    &format!("{{{{ {var} }}}}"),
                    ValidationScope::Hook(HookType::PreCreate),
                    &test.repo,
                    "test"
                )
                .is_ok(),
                "{var} should validate in pre-start scope"
            );
        }

        // `pr_number` is not available in pre-merge (different hook type).
        let err = validate_template(
            "{{ pr_number }}",
            ValidationScope::Hook(HookType::PreMerge),
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );

        // `args` is available in hook scope (forwarded via smart routing).
        assert!(
            validate_template(
                "{{ args }}",
                ValidationScope::Hook(HookType::PreCreate),
                &test.repo,
                "test",
            )
            .is_ok()
        );

        // `args` is not available in SwitchExecute.
        let err = validate_template(
            "{{ args }}",
            ValidationScope::SwitchExecute,
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn test_validate_template_syntax_error() {
        let test = test_repo();

        let err = validate_template("{{ unclosed", ValidationScope::Alias, &test.repo, "test")
            .unwrap_err();
        assert!(err.message.contains("syntax error"), "got: {}", err.message);
    }

    #[test]
    fn test_referenced_vars_for_config_syntax_error_propagates() {
        let cfg = super::super::CommandConfig::single("echo {{ unclosed");
        let err = referenced_vars_for_config(&cfg, "deploy").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Failed to expand deploy"), "got: {msg}");
        assert!(msg.contains("syntax error"), "got: {msg}");
    }

    #[test]
    fn test_validate_template_undefined_var() {
        let test = test_repo();

        let err = validate_template(
            "{{ nonexistent_var }}",
            ValidationScope::Hook(HookType::PostCreate),
            &test.repo,
            "test",
        )
        .unwrap_err();
        assert!(
            err.message.contains("undefined value"),
            "got: {}",
            err.message
        );
        // Should list available vars in hint
        assert!(!err.available_vars.is_empty(), "should list available vars");
        assert!(err.available_vars.contains(&"branch".to_string()));
    }

    #[test]
    fn test_format_hook_variables_groups_and_unset() {
        let mut ctx: HashMap<String, String> = HashMap::new();
        ctx.insert("branch".into(), "feature".into());
        ctx.insert("worktree_path".into(), "/tmp/feature".into());
        ctx.insert("worktree_name".into(), "feature".into());
        ctx.insert("base".into(), "main".into());
        ctx.insert("base_worktree_path".into(), "/tmp/main".into());
        ctx.insert("target".into(), "-".into());
        // target_worktree_path deliberately absent — mimics `wt switch -`
        ctx.insert("repo".into(), "demo".into());
        ctx.insert("repo_path".into(), "/tmp/demo".into());
        ctx.insert("cwd".into(), "/tmp/feature".into());
        ctx.insert("hook_type".into(), "pre-switch".into());
        ctx.insert("hook_name".into(), "show-variables".into());

        assert_snapshot!(format_hook_variables(HookType::PreSwitch, &ctx), @r"
        branch                = feature
        worktree_path         = /tmp/feature
        worktree_name         = feature
        commit                = (unset)
        short_commit          = (unset)
        upstream              = (unset)
        base                  = main
        base_worktree_path    = /tmp/main
        target                = -
        target_worktree_path  = (unset)
        pr_number             = (unset)
        pr_url                = (unset)
        repo                  = demo
        repo_path             = /tmp/demo
        owner                 = (unset)
        primary_worktree_path = (unset)
        default_branch        = (unset)
        remote                = (unset)
        remote_url            = (unset)
        cwd                   = /tmp/feature
        hook_type             = pre-switch
        hook_name             = show-variables
        ");
    }

    #[test]
    fn test_format_hook_variables_filters_operation() {
        // pre-commit only has `target` in operation scope — no base*, pr_*, etc.
        let mut ctx: HashMap<String, String> = HashMap::new();
        ctx.insert("target".into(), "main".into());
        let out = format_hook_variables(HookType::PreCommit, &ctx);
        assert!(out.contains("target                = main"), "got: {out}");
        assert!(
            !out.contains("base "),
            "pre-commit has no `base`; got: {out}"
        );
        assert!(
            !out.contains("pr_number"),
            "pre-commit has no `pr_number`; got: {out}"
        );
    }

    #[test]
    fn test_format_alias_variables_includes_args_no_hook_keys() {
        let mut ctx: HashMap<String, String> = HashMap::new();
        ctx.insert("branch".into(), "feature".into());
        ctx.insert("worktree_path".into(), "/tmp/feature".into());
        ctx.insert("worktree_name".into(), "feature".into());
        ctx.insert("repo".into(), "demo".into());
        ctx.insert("repo_path".into(), "/tmp/demo".into());
        ctx.insert("cwd".into(), "/tmp/feature".into());
        // args is JSON-encoded per `ALIAS_ARGS_KEY` contract; the table
        // decodes and shell-renders it to match `{{ args }}` substitution.
        ctx.insert(ALIAS_ARGS_KEY.into(), r#"["a","b c"]"#.into());

        let out = format_alias_variables(&ctx, None);
        assert!(
            out.contains("args                  = a 'b c'"),
            "got: {out}"
        );
        // No hook-only keys appear in alias scope.
        assert!(!out.contains("hook_type"), "got: {out}");
        assert!(!out.contains("target"), "got: {out}");
        assert!(!out.contains("base "), "got: {out}");
    }

    #[test]
    fn test_format_alias_variables_args_empty() {
        let mut ctx: HashMap<String, String> = HashMap::new();
        ctx.insert(ALIAS_ARGS_KEY.into(), "[]".into());
        let out = format_alias_variables(&ctx, None);
        // Empty args render as an empty string after the `=` — distinct from
        // `(unset)`, which means the key was absent entirely. `args` sits last
        // in alias ordering, so the output ends with it.
        assert!(out.ends_with("args                  = "), "got: {out:?}");
    }

    #[test]
    fn test_vars_map_to_value_parses_json() {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("ticket".to_string(), "JIRA-1".to_string());
        entries.insert("config".to_string(), r#"{"port": 8080}"#.to_string());
        let value = vars_map_to_value(&entries);

        // Plain strings stay as-is; JSON objects support nested access
        assert_eq!(
            value.get_attr("ticket").unwrap(),
            Value::from("JIRA-1".to_string())
        );
        assert_eq!(
            value.get_attr("config").unwrap().get_attr("port").unwrap(),
            Value::from(8080)
        );
    }

    #[test]
    fn test_validate_list_column_template() {
        let test = test_repo();

        // Row-identity vars, unconstrained vars.* keys, and filters validate
        validate_list_column_template("{{ branch }} {{ worktree_name }}", &test.repo, "t").unwrap();
        validate_list_column_template("{{ vars.any_key_at_all }}", &test.repo, "t").unwrap();
        validate_list_column_template("{{ branch | sanitize }}", &test.repo, "t").unwrap();
        // Nested vars access (JSON values at runtime) must not be rejected
        // even though the trial render's placeholder vars are flat strings
        validate_list_column_template("{{ vars.config.port }}", &test.repo, "t").unwrap();
        // git.branch.* mirrors vars.*: any key validates, filters apply
        validate_list_column_template("{{ git.branch.jira }}", &test.repo, "t").unwrap();
        validate_list_column_template(
            "{{ git.branch.description | lines | first }}",
            &test.repo,
            "t",
        )
        .unwrap();

        // Unknown top-level variables list the available set, both namespaces included
        let err = validate_list_column_template("{{ branhc }}", &test.repo, "t").unwrap_err();
        assert!(err.message.contains("branhc"), "got: {}", err.message);
        assert_eq!(
            err.available_vars,
            vec![
                "branch",
                "git.branch.<key>",
                "vars.<key>",
                "worktree_name",
                "worktree_path"
            ]
        );

        // Syntax errors fail
        validate_list_column_template("{{ branch", &test.repo, "t").unwrap_err();

        // Misspelled filters surface via the trial render rather than as
        // silently empty cells at runtime
        let err =
            validate_list_column_template("{{ branch | nosuch }}", &test.repo, "t").unwrap_err();
        assert!(err.message.contains("nosuch"), "got: {}", err.message);
    }
}
