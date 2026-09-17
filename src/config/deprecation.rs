//! Deprecation detection and migration.
//!
//! Scans config files for deprecated patterns and surfaces them to the user:
//! - Deprecated template variables (repo_root → repo_path, etc.)
//! - Deprecated config sections (\[commit-generation\] → \[commit.generation\])
//! - Deprecated fields (args merged into command)
//! - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
//!
//! Each deprecated pattern is one row in [`DEPRECATION_RULES`]: a single
//! idempotent function that rewrites the pattern into canonical form and
//! reports what it changed. Detection runs the same functions against a
//! scratch copy of the document, so a warning fires exactly when
//! `wt config update` would change the file — detection and migration share
//! one predicate and cannot drift. The table order is both the
//! warning-emission order and the migration order.
//!
//! Detection is purely in-memory — nothing writes to the filesystem from a
//! config load path. `check_and_migrate` returns the structurally migrated
//! content (for serde) and a `DeprecationInfo` describing what needs fixing.
//! Users materialize migrations explicitly via `wt config update` (which
//! overwrites the config file and copies approved-commands to `approvals.toml`)
//! or export them via `wt config update --output <path>` (`-` for stdout).
//!
//! Per-path warning dedup still applies within a process so `wt list` doesn't
//! spam the same deprecation message from multiple config layers.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use anyhow::Context;
use color_print::cformat;
use minijinja::machinery::{ast, parse as parse_template};
use shell_escape::unix::escape;

use crate::config::WorktrunkConfig;
use crate::shell_exec::Cmd;
use crate::styling::{
    eprint, eprintln, format_with_gutter, hint_message, info_message, suggest_command_in_dir,
    warning_message,
};

/// Which config file a deprecation pass is examining.
///
/// Replaces the string labels that used to travel with each check: the kind
/// derives the display label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFileKind {
    /// `~/.config/worktrunk/config.toml` — the file `wt config update` rewrites.
    User,
    /// The system-wide config layer (same schema as user config; never
    /// rewritten by `wt config update`).
    System,
    /// The repo's `.config/wt.toml`.
    Project,
}

impl ConfigFileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "User config",
            Self::System => "System config",
            Self::Project => "Project config",
        }
    }
}

/// Tracks which config paths have already shown deprecation warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_DEPRECATED_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Set once the "Run wt config show..." hint has been emitted this process,
/// so multiple deprecated configs (user + project) share a single hint line.
static DEPRECATION_HINT_EMITTED: OnceLock<()> = OnceLock::new();

/// Latch that silences config deprecation/unknown-field warnings for the rest
/// of the process. Set by shell completion, picker, statusline, and help paths
/// — surfaces where stderr output would appear above the user's prompt or TUI.
static SUPPRESS_WARNINGS: OnceLock<()> = OnceLock::new();

pub fn suppress_warnings() {
    let _ = SUPPRESS_WARNINGS.set(());
}

/// Whether [`suppress_warnings`] latched this process. Consulted by warning
/// emitters outside this module (e.g. the `wt list` invalid-schema warning) so
/// suppressed surfaces like the statusline stay clean.
pub fn warnings_suppressed() -> bool {
    SUPPRESS_WARNINGS.get().is_some()
}

/// Tracks which config paths have already shown unknown field warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_UNKNOWN_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Retired template variables, mapped to their replacement. No renderer
/// supplies the old name any more, so the rewrite is
/// [`DeprecationRule::Structural`]: it applies on every load, before serde
/// parses, and is what keeps an unmigrated template rendering what it always
/// did. The warning still fires and still points at `wt config update`, which
/// writes the rename into the user's file.
///
/// Every row is a mechanical identifier swap — the replacement resolves to the
/// same value the old name did — which is what makes rewriting the in-memory
/// config on every load safe. `commits` is squash-template-only, and each
/// `commit_details` element renders as its subject when printed bare, so a
/// migrated `{% for c in commit_details %}{{ c }}` reads identically to the old
/// `{% for c in commits %}{{ c }}` (see #2984 and `CommitDetailValue`).
///
/// The rewrite happens at load, not on request, because an old name that
/// reached a renderer would fail differently depending on where it sat. A
/// hook, alias, or `worktree-path` template would fail its `SemiStrict`
/// expansion with an undefined-variable error — loud, and fixable from the
/// message. A squash template would render nothing at all: `build_prompt`
/// renders under minijinja's default `UndefinedBehavior::Lenient`, which
/// iterates an undefined value as an empty sequence, so the prompt would lose
/// its commit list silently — the outcome #2984 opens by calling out.
///
/// TODO(retired-vars): revisit dropping these rows after 2026-12-16, three
/// months on from the load-time rewrite (#4080). Dropping a row stops the
/// rename, so the old name has to fail loudly on its own before it goes, and
/// the split above is what makes the rows non-uniform: the `SemiStrict`
/// surfaces already do, while `commits` can only go once `build_prompt`
/// rejects the name itself (#2984).
const RETIRED_VARS: &[(&str, &str)] = &[
    ("repo_root", "repo_path"),
    ("worktree", "worktree_path"),
    ("main_worktree", "repo"),
    ("main_worktree_path", "primary_worktree_path"),
    ("commits", "commit_details"),
];

/// Metadata for a deprecated top-level section key.
#[derive(Debug)]
pub struct DeprecatedSection {
    /// The deprecated key name (e.g., "commit-generation")
    pub key: &'static str,
    /// The canonical top-level key that replaces this, for determining which config type
    /// it belongs to via `WorktrunkConfig::is_valid_key()` (e.g., "commit")
    pub canonical_top_key: &'static str,
    /// Human-readable canonical form for display (e.g., "[commit.generation]")
    pub canonical_display: &'static str,
}

/// Top-level keys that are deprecated and handled by the deprecation system —
/// renamed sections (`[commit-generation]` → `[commit.generation]`).
///
/// When a deprecated key appears in the config type where its canonical replacement
/// is valid, `warn_unknown_fields` skips it (the deprecation system provides better
/// messaging). When it appears in the wrong config type, `warn_unknown_fields`
/// warns that it belongs in the other config with the canonical form.
pub const DEPRECATED_SECTION_KEYS: &[DeprecatedSection] = &[
    DeprecatedSection {
        key: "commit-generation",
        canonical_top_key: "commit",
        canonical_display: "[commit.generation]",
    },
    DeprecatedSection {
        key: "select",
        canonical_top_key: "switch",
        canonical_display: "[switch.picker]",
    },
    DeprecatedSection {
        key: "ci",
        canonical_top_key: "forge",
        canonical_display: "[forge]",
    },
];

/// Normalize a template string by replacing deprecated variables with their canonical names.
///
/// This allows approval matching to work regardless of whether the command was saved
/// with old or new variable names. For example, `{{ repo_root }}` and `{{ repo_path }}`
/// will both normalize to `{{ repo_path }}`.
///
/// Returns `Cow::Borrowed` if no replacements needed, avoiding allocation.
pub fn normalize_template_vars(template: &str) -> Cow<'_, str> {
    match migrate_retired_vars(template) {
        Some((migrated, _)) => Cow::Owned(migrated),
        None => Cow::Borrowed(template),
    }
}

/// Rewrite every retired template variable in `template`, returning the new
/// text alongside the `(old, new)` pairs it replaced, in [`RETIRED_VARS`]
/// order.
///
/// `None` leaves the template exactly as written: no retired name is read, it
/// doesn't parse, or a statement binds either half of a pair — see
/// [`TemplateVars`] for why a bound name drops the pair rather than renaming
/// half a scope. An identifier that isn't a variable read — an attribute
/// (`{{ foo.repo_root }}`), a keyword argument, an assignment target — isn't a
/// use, so it doesn't bring a pair in on its own.
///
/// Detection and migration are this one call: the rule warns about the pairs
/// the same invocation rewrites, so the two cannot drift.
fn migrate_retired_vars(template: &str) -> Option<(String, Vec<(&'static str, &'static str)>)> {
    // Quick check: if none of the retired vars appear, skip parsing
    if !RETIRED_VARS.iter().any(|(old, _)| template.contains(old)) {
        return None;
    }

    let vars = TemplateVars::of(template)?;
    let replacements = RETIRED_VARS
        .iter()
        .copied()
        .filter(|(old, new)| {
            vars.reads.iter().any(|(name, _)| name == old)
                && !vars.bound.contains(old)
                && !vars.bound.contains(new)
        })
        .collect::<Vec<_>>();
    if replacements.is_empty() {
        return None;
    }

    let mut edits = vars
        .reads
        .into_iter()
        .filter_map(|(name, at)| {
            let (_, new) = replacements.iter().find(|(old, _)| *old == name)?;
            Some((at, *new))
        })
        .collect::<Vec<_>>();
    // Reads are collected in visit order, which is not source order — a
    // conditional expression is visited test-first (`{{ a if b }}` yields `b`
    // before `a`), and a map's keys all precede its values.
    edits.sort_by_key(|(at, _)| at.start);

    let mut migrated = String::with_capacity(template.len());
    let mut cursor = 0;
    for (at, new) in edits {
        migrated.push_str(&template[cursor..at.start]);
        migrated.push_str(new);
        cursor = at.end;
    }
    migrated.push_str(&template[cursor..]);
    Some((migrated, replacements))
}

/// A template as MiniJinja's own parser reads it: every variable read, with
/// the byte range of the identifier behind it, and every name a statement
/// binds.
///
/// Both halves were once scanned by hand, which meant re-deriving MiniJinja's
/// delimiters, whitespace control, string quoting, `{% raw %}` handling and
/// assignment-target grammar. Each place the two readings disagreed was
/// visible in the file worktrunk writes (#4117): a `}}` inside a string ended
/// a tag early, so the reference after it never migrated, and an unrecognized
/// target list renamed a local's uses out from under its binding. Parsing
/// through [`minijinja::machinery`] removes the second reading instead of
/// correcting it — the reads are the `Expr::Var` nodes, which is by
/// construction the set the renderer resolves against the context, and the
/// bindings are the statements' own target expressions.
///
/// `machinery` carries no semver guarantee, so the coupling is to the AST's
/// shape alone: the matches below are exhaustive over `Stmt` and `Expr`, and
/// `Cargo.toml` takes MiniJinja with `default-features = false`, so the
/// `macros`, `multi_template` and `loop_controls` variants don't exist to
/// handle. A MiniJinja that adds or moves a node fails this build rather than
/// quietly mis-migrating a config, and the compile error names what to
/// handle.
struct TemplateVars<'a> {
    /// The name and byte range of every `Expr::Var`, in visit order.
    reads: Vec<(&'a str, Range<usize>)>,
    /// Every name a `set`, `for`, or `with` binds, at any depth.
    ///
    /// The rewrite has no notion of scope — it replaces reads wherever they
    /// sit — so a deprecated name bound anywhere is dropped from the
    /// replacement set entirely rather than renamed per scope:
    /// `{{ repo_root }}{% set repo_root = "local" %}{{ repo_root }}` must not
    /// have its last use renamed away from the binding it reads. Detection
    /// reads that same set, so such a template neither migrates nor warns.
    /// Losing the warning is the price of not quietly rendering something
    /// else.
    ///
    /// Since [`RETIRED_VARS`] became a [`DeprecationRule::Structural`] row
    /// that price is paid at render time rather than deferred: the retired
    /// name survives the load-path rewrite and reaches a renderer that has
    /// nothing to resolve it to, failing a `SemiStrict` expansion loudly or
    /// rendering empty in a squash template. Both beat renaming one scope's
    /// worth of uses out from under the template that bound the name.
    ///
    /// The *canonical* name is checked against this set too. Binding it
    /// captures the global use the rename produces: `{% for repo_path in items
    /// %}` around a `{{ repo_root }}` reads the global today and the loop
    /// variable once renamed. The same collision reaches every pair — `{% for
    /// commit_details in … %}{{ commits }}` is the squash-template shape of it.
    bound: HashSet<&'a str>,
}

impl<'a> TemplateVars<'a> {
    /// `None` when MiniJinja can't parse `template` — the templates its
    /// renderer rejects too, left untouched rather than guessed at.
    fn of(template: &'a str) -> Option<Self> {
        // The syntax and whitespace defaults, spelled `Default::default()`
        // because `SyntaxConfig` is a unit struct without MiniJinja's
        // `custom_syntax` feature. Neither has to match the environment that
        // renders the template — `expand_template_with` sets
        // `keep_trailing_newline(true)` for every `ShellEscapeMode` but
        // `Literal`, and a `WhitespaceConfig` only ever shapes literal text
        // (which byte the tokenizer stops at, where an `EmitRaw` node's
        // boundaries fall), never an `Expr::Var` span. Everything outside
        // those spans is copied from the original string, so the two readings
        // cannot move an edit apart.
        let ast =
            parse_template(template, "<config>", Default::default(), Default::default()).ok()?;
        let mut vars = TemplateVars {
            reads: Vec::new(),
            bound: HashSet::new(),
        };
        vars.stmt(&ast);
        Some(vars)
    }

    fn body(&mut self, body: &[ast::Stmt<'a>]) {
        for stmt in body {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &ast::Stmt<'a>) {
        match stmt {
            ast::Stmt::Template(node) => self.body(&node.children),
            ast::Stmt::EmitExpr(node) => self.expr(&node.expr),
            // Literal output — the text around the tags, and everything inside
            // a `{% raw %}` block.
            ast::Stmt::EmitRaw(_) => {}
            ast::Stmt::ForLoop(node) => {
                self.target(&node.target);
                self.expr(&node.iter);
                if let Some(filter) = &node.filter_expr {
                    self.expr(filter);
                }
                self.body(&node.body);
                self.body(&node.else_body);
            }
            ast::Stmt::IfCond(node) => {
                self.expr(&node.expr);
                self.body(&node.true_body);
                self.body(&node.false_body);
            }
            ast::Stmt::WithBlock(node) => {
                for (target, value) in &node.assignments {
                    self.target(target);
                    self.expr(value);
                }
                self.body(&node.body);
            }
            ast::Stmt::Set(node) => {
                self.target(&node.target);
                self.expr(&node.expr);
            }
            ast::Stmt::SetBlock(node) => {
                self.target(&node.target);
                if let Some(filter) = &node.filter {
                    self.expr(filter);
                }
                self.body(&node.body);
            }
            ast::Stmt::AutoEscape(node) => {
                self.expr(&node.enabled);
                self.body(&node.body);
            }
            ast::Stmt::FilterBlock(node) => {
                self.expr(&node.filter);
                self.body(&node.body);
            }
            ast::Stmt::Do(node) => self.call(&node.call),
        }
    }

    fn expr(&mut self, expr: &ast::Expr<'a>) {
        match expr {
            ast::Expr::Var(node) => {
                let span = node.span();
                self.reads.push((
                    node.id,
                    span.start_offset as usize..span.end_offset as usize,
                ));
            }
            ast::Expr::Const(_) => {}
            ast::Expr::Slice(node) => {
                self.expr(&node.expr);
                for bound in [&node.start, &node.stop, &node.step].into_iter().flatten() {
                    self.expr(bound);
                }
            }
            ast::Expr::UnaryOp(node) => self.expr(&node.expr),
            ast::Expr::BinOp(node) => {
                self.expr(&node.left);
                self.expr(&node.right);
            }
            ast::Expr::Compare(node) => {
                self.expr(&node.expr);
                for op in &node.ops {
                    self.expr(&op.expr);
                }
            }
            ast::Expr::IfExpr(node) => {
                self.expr(&node.test_expr);
                self.expr(&node.true_expr);
                if let Some(false_expr) = &node.false_expr {
                    self.expr(false_expr);
                }
            }
            // A filter or test names a function the environment supplies, not
            // a variable, so only its input and arguments are reads.
            ast::Expr::Filter(node) => {
                if let Some(expr) = &node.expr {
                    self.expr(expr);
                }
                self.args(&node.args);
            }
            ast::Expr::Test(node) => {
                self.expr(&node.expr);
                self.args(&node.args);
            }
            // `{{ foo.repo_root }}` reads `foo`; the attribute belongs to
            // whatever that resolves to, never to the deprecated global.
            ast::Expr::GetAttr(node) => self.expr(&node.expr),
            ast::Expr::GetItem(node) => {
                self.expr(&node.expr);
                self.expr(&node.subscript_expr);
            }
            ast::Expr::Call(node) => self.call(node),
            ast::Expr::List(node) => {
                for item in &node.items {
                    self.expr(item);
                }
            }
            ast::Expr::Map(node) => {
                for entry in node.keys.iter().chain(&node.values) {
                    self.expr(entry);
                }
            }
        }
    }

    fn call(&mut self, call: &ast::Call<'a>) {
        self.expr(&call.expr);
        self.args(&call.args);
    }

    /// A keyword argument's name belongs to the call it is passed to, so only
    /// the argument values are reads.
    fn args(&mut self, args: &[ast::CallArg<'a>]) {
        for arg in args {
            match arg {
                ast::CallArg::Pos(expr)
                | ast::CallArg::Kwarg(_, expr)
                | ast::CallArg::PosSplat(expr)
                | ast::CallArg::KwargSplat(expr) => self.expr(expr),
            }
        }
    }

    /// The names an assignment target binds.
    fn target(&mut self, target: &ast::Expr<'a>) {
        match target {
            ast::Expr::Var(node) => {
                self.bound.insert(node.id);
            }
            // A tuple target, nested arbitrarily: `{% for (a, (b, c)) in … %}`.
            ast::Expr::List(node) => {
                for item in &node.items {
                    self.target(item);
                }
            }
            // A dotted target mutates an attribute of whatever the path
            // resolves to, so `{% set repo_root.x = … %}` reads the global
            // rather than binding it.
            read => self.expr(read),
        }
    }
}

/// Replace every [`RETIRED_VARS`] template variable in every string value of
/// the document, mutating the `toml_edit` tree in place; returns one
/// [`DeprecationKind::TemplateVar`] per `(old, new)` pair replaced, in
/// [`RETIRED_VARS`] order.
///
/// Operating on the parsed tree (rather than a raw `str::replace` against the
/// file text) is correct when the TOML source uses escapes: the decoded value
/// would not appear verbatim in the file, so a raw replace silently skipped
/// the migration while detection still warned. `toml_edit` re-serializes the
/// changed string with proper escaping.
fn migrate_template_vars_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    type Replaced = HashSet<(&'static str, &'static str)>;

    fn walk_table(table: &mut toml_edit::Table, replaced: &mut Replaced) {
        for (_, item) in table.iter_mut() {
            walk_item(item, replaced);
        }
    }
    fn walk_item(item: &mut toml_edit::Item, replaced: &mut Replaced) {
        match item {
            toml_edit::Item::Value(v) => walk_value(v, replaced),
            toml_edit::Item::Table(t) => walk_table(t, replaced),
            toml_edit::Item::ArrayOfTables(arr) => {
                for t in arr.iter_mut() {
                    walk_table(t, replaced);
                }
            }
            _ => {}
        }
    }
    fn walk_value(value: &mut toml_edit::Value, replaced: &mut Replaced) {
        match value {
            toml_edit::Value::String(s) => {
                if let Some((migrated, pairs)) = migrate_retired_vars(s.value()) {
                    let decor = s.decor().clone();
                    let mut formatted = toml_edit::Formatted::new(migrated);
                    *formatted.decor_mut() = decor;
                    *value = toml_edit::Value::String(formatted);
                    replaced.extend(pairs);
                }
            }
            toml_edit::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    walk_value(v, replaced);
                }
            }
            toml_edit::Value::InlineTable(t) => {
                for (_, v) in t.iter_mut() {
                    walk_value(v, replaced);
                }
            }
            _ => {}
        }
    }

    let mut replaced = Replaced::new();
    walk_table(doc.as_table_mut(), &mut replaced);
    RETIRED_VARS
        .iter()
        .filter(|pair| replaced.contains(*pair))
        .map(|&(old, new)| DeprecationKind::TemplateVar { old, new })
        .collect()
}

/// Which scopes of one config file carried a deprecated section: the top
/// level, and/or named `[projects."<id>"]` entries.
///
/// A section rename warns once per scope so the line names a path the reader
/// actually has — `[projects."<id>".select]` rather than a bare `[select]`
/// they never wrote.
#[derive(Debug, Default, Clone)]
pub struct ScopedSections {
    /// The deprecated section appeared at the top level.
    pub has_top_level: bool,
    /// Project keys whose `[projects."<id>"]` entry carried the section.
    pub project_keys: Vec<String>,
}

impl ScopedSections {
    pub fn is_empty(&self) -> bool {
        !self.has_top_level && self.project_keys.is_empty()
    }

    /// Record that the section was migrated in `scope`.
    fn record(&mut self, scope: Option<&str>) {
        match scope {
            None => self.has_top_level = true,
            Some(key) => self.project_keys.push(key.to_string()),
        }
    }
}

/// One deprecated config pattern, carrying the payload its warning needs.
///
/// Each variant maps to a single `format_deprecation_warnings` arm. Silently
/// migrated patterns (the `-create` → `-start` hook rename) have NO variant —
/// they produce no warning by construction.
#[derive(Debug, Clone)]
pub enum DeprecationKind {
    /// Deprecated template variable `old` replaced by `new`.
    TemplateVar {
        old: &'static str,
        new: &'static str,
    },
    /// `[commit-generation]` sections → `[commit.generation]`, one warning
    /// line per scope that was migrated.
    CommitGeneration(ScopedSections),
    /// `approved-commands` under `[projects."..."]` (moved to approvals.toml).
    ApprovedCommands,
    /// `[select]` sections → `[switch.picker]`, one warning line per scope
    /// that was migrated.
    Select(ScopedSections),
    /// A key in a deprecated section that its replacement has no field for,
    /// so the migration removes it instead of carrying it over. `section` is
    /// the deprecated section's display form in its own scope (`[select]`,
    /// `[projects."<id>".commit-generation]`); `key` is the key.
    UnsupportedKey { section: String, key: String },
    /// `[ci]` section (moved to `[forge]`).
    CiSection,
    /// `no-ff` in `[merge]` (use `ff` instead).
    NoFf,
    /// `no-cd` in `[switch]` (use `cd` instead).
    NoCd,
    /// `task-timeout-ms` under `[list]` (removed — `[list] timeout-ms` bounds
    /// the collect phase).
    ListTaskTimeout,
}

/// All deprecation patterns detected in a config file, in the order their
/// warnings are emitted.
///
/// Pure data with no path/label context. Used by both config loading (brief
/// warnings) and `wt config show` (full details). Empty when nothing is
/// deprecated.
pub type Deprecations = Vec<DeprecationKind>;

/// A warning rule's single function: rewrite the deprecated pattern into
/// canonical form and return one [`DeprecationKind`] per warning for what
/// changed. A non-empty return means the document was modified — the same
/// value drives the rewrite, the warning, and the `wt config update` diff,
/// so a rule cannot rewrite without warning or warn about something the
/// rewrite won't fix. Idempotent — feeding a migrated document back in
/// returns empty.
type MigrateFn = fn(&mut toml_edit::DocumentMut) -> Deprecations;

/// A silent rule's migration: rewrites with no warning by construction — the
/// signature has no channel for a [`DeprecationKind`]. Returns whether the
/// document changed.
type SilentMigrateFn = fn(&mut toml_edit::DocumentMut) -> bool;

/// One deprecated config pattern: a single function that detects, rewrites,
/// and reports, plus when the rewrite applies.
enum DeprecationRule {
    /// Warns, and is rewritten on every config load before serde parses.
    Structural(MigrateFn),
    /// Warns, but the deprecated form still works at runtime
    /// (`approved-commands` is still a valid serde field), so the load path
    /// leaves it alone. Rewritten only via [`compute_migrated_content`]
    /// (`wt config show` / `wt config update`). A template variable nothing
    /// supplies any more belongs in [`RETIRED_VARS`], which is rewritten
    /// structurally instead — an unmigrated name would otherwise reach a
    /// renderer that has nothing to resolve it to.
    UpdateOnly(MigrateFn),
    /// Silently-migrated rename: rewritten on every load like `Structural`,
    /// but with no warning by construction.
    Silent(SilentMigrateFn),
}

/// Which pass rules run under.
///
/// `Load` is the structural rewrite before serde parses — `Structural` and
/// `Silent` rows only. `Update` is what `wt config update` materializes —
/// every row. Detection always runs the `Update` pass against a scratch copy,
/// so a rule's detection and migration share one predicate and cannot drift.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RulePass {
    Load,
    Update,
}

/// Every deprecation, one row each. The table order is the contract:
/// detection ([`detect_deprecations_from_doc`]) and migration
/// ([`migrate_content_doc`], [`compute_migrated_content`]) iterate top to
/// bottom, so a row's position is both its warning-emission position and its
/// migration position. Detection applies each rule to a scratch copy that
/// earlier rules have already migrated, so a rule reports a pattern in its
/// post-migration shape.
///
/// The `[ci]` → `[forge]` rule is order-independent: `[forge]` renders at an
/// explicit document position either way — the one it was parsed at, or
/// `[ci]`'s when the rule creates the table (see [`migrate_ci_doc`]) — so its
/// rendered placement doesn't depend on which tables other rules re-append.
///
/// A [`DeprecationRule::Structural`] rule must not depend on an `UpdateOnly`
/// rewrite preceding it: the load path skips `UpdateOnly` rules while
/// detection applies them, so such a dependency would make the load-path
/// rewrite diverge from what was warned. No rule here has such a dependency —
/// the template-variable row reads and writes a key space
/// (`{{ … }}` identifiers inside string values) that no other rule touches,
/// and `approved-commands` is read by no other rule.
///
/// A rule that moves a section's table wholesale into a new location must
/// remove the keys its destination has no field for, reporting each via
/// [`drop_unsupported_keys`]. Carrying such a key over puts it at a path the
/// user never typed, which the unknown-field check then names on every command
/// and which `wt config update` writes into the file rather than clearing.
/// `test_warning_fires_iff_update_changes` pins the resulting invariant: what
/// an update writes raises neither warning.
///
/// Adding a deprecation: a single idempotent migrate-and-report fn, a
/// [`DeprecationKind`] variant with its `format_deprecation_warnings` arm,
/// and a row here (plus a [`DeprecatedSection`] entry for a removed top-level
/// section). A silently-migrated rename is just a [`DeprecationRule::Silent`]
/// row.
const DEPRECATION_RULES: &[DeprecationRule] = &[
    // Retired template variables: {{ repo_root }} → {{ repo_path }},
    // {{ commits }} → {{ commit_details }}, etc., inside any string value.
    // Structural because no renderer supplies any of these names any more —
    // the load-path rewrite is what keeps an unmigrated template rendering
    // what it always did.
    DeprecationRule::Structural(migrate_template_vars_doc),
    // [commit-generation] → [commit.generation], top-level and per-project.
    DeprecationRule::Structural(migrate_commit_generation_doc),
    // approved-commands under [projects."..."] → approvals.toml. The rule only
    // removes; `wt config update` copies the entries to approvals.toml first
    // (see `copy_approved_commands_to_approvals_file`).
    DeprecationRule::UpdateOnly(remove_approved_commands_doc),
    // [select] → [switch.picker].
    DeprecationRule::Structural(migrate_select_doc),
    // pre-create/post-create → pre-start/post-start. Silent: the creation
    // hook rename is paused (see #2838) — both names load via serde aliases,
    // but in-memory migration to canonical keeps round-trip analysis
    // (`unknown_tree`) coherent for the table and array-of-tables forms,
    // where serde aliases on the field don't cover every shape.
    DeprecationRule::Silent(canonicalize_hook_keys),
    // [ci] → [forge]. Moves `platform` only; unrelated `[ci]` keys stay where
    // the user wrote them, so they keep warning at their own path rather than
    // being relocated (contrast the wholesale-move rules above, which drop
    // what the destination can't hold).
    DeprecationRule::Structural(migrate_ci_doc),
    // merge.no-ff → merge.ff (inverted).
    DeprecationRule::Structural(|doc| {
        migrate_negated_bool_doc(doc, "merge", "no-ff", "ff", DeprecationKind::NoFf)
    }),
    // switch.no-cd → switch.cd (inverted).
    DeprecationRule::Structural(|doc| {
        migrate_negated_bool_doc(doc, "switch", "no-cd", "cd", DeprecationKind::NoCd)
    }),
    // list.task-timeout-ms — removed; `[list] timeout-ms` bounds the collect
    // phase, and the drain has its own fallback bound.
    DeprecationRule::Structural(|doc| {
        if for_each_config_table_mut(doc, |scope, table| {
            remove_section_key_in(table, scope, "list", "task-timeout-ms")
        }) {
            vec![DeprecationKind::ListTaskTimeout]
        } else {
            Vec::new()
        }
    }),
];

/// Test helper: detect deprecations in config content without I/O.
#[cfg(test)]
fn detect_deprecations(content: &str) -> Deprecations {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    detect_deprecations_from_doc(&doc)
}

/// Detect deprecations from an already-parsed document.
///
/// Runs every rule against a scratch copy and reports what they changed, so
/// a warning fires exactly when `wt config update` would change the file.
/// Pushes kinds in [`DEPRECATION_RULES`] order — the warning-emission order —
/// so iterating the returned `Vec` reproduces the warning text byte-for-byte.
fn detect_deprecations_from_doc(doc: &toml_edit::DocumentMut) -> Deprecations {
    let mut scratch = doc.clone();
    let mut kinds = Vec::new();
    apply_rules(&mut scratch, RulePass::Update, &mut kinds);
    kinds
}

/// Run every rule in table order against `doc`, appending each rule's warning
/// kinds to `kinds` and returning whether the document changed.
///
/// The load pass excludes [`DeprecationRule::UpdateOnly`] rewrites (cosmetic
/// or still-valid serde fields, so serde doesn't need them applied).
/// Detection and [`compute_migrated_content`] run the update pass.
fn apply_rules(doc: &mut toml_edit::DocumentMut, pass: RulePass, kinds: &mut Deprecations) -> bool {
    let mut modified = false;
    for rule in DEPRECATION_RULES {
        match rule {
            DeprecationRule::Structural(migrate) => {
                let new_kinds = migrate(doc);
                modified |= !new_kinds.is_empty();
                kinds.extend(new_kinds);
            }
            DeprecationRule::UpdateOnly(migrate) => {
                if matches!(pass, RulePass::Update) {
                    let new_kinds = migrate(doc);
                    modified |= !new_kinds.is_empty();
                    kinds.extend(new_kinds);
                }
            }
            DeprecationRule::Silent(migrate) => modified |= migrate(doc),
        }
    }
    modified
}

/// Scope walk: apply `f` mutably to the top-level table (scope
/// `None`) and to each `[projects."key"]` table (scope `Some(key)`), returning
/// whether any scope reported a change.
///
/// Both the `projects` container and each entry inside it can be written as a
/// standard table or inline (`"host/org/repo" = { merge = { no-ff = true } }`),
/// and `toml_edit` surfaces those as different node types. Every rule routed
/// through this walk sees the same scopes either way: an inline entry is
/// migrated through a table view and written back inline, so a shape the user
/// chose can't decide whether their config gets migrated. The alternative —
/// skipping inline entries — leaves a deprecated key unmigrated at a path serde
/// has no alias for, which drops the setting from the typed config on load.
fn for_each_config_table_mut(
    doc: &mut toml_edit::DocumentMut,
    mut f: impl FnMut(Option<&str>, &mut toml_edit::Table) -> bool,
) -> bool {
    let mut modified = f(None, doc.as_table_mut());
    match doc.get_mut("projects") {
        Some(toml_edit::Item::Table(projects)) => {
            for (key, entry) in projects.iter_mut() {
                let scope = key.get();
                modified |= match entry {
                    toml_edit::Item::Table(table) => f(Some(scope), table),
                    toml_edit::Item::Value(value) => migrate_inline_scope(scope, value, &mut f),
                    _ => false,
                };
            }
        }
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(projects))) => {
            for (key, entry) in projects.iter_mut() {
                modified |= migrate_inline_scope(key.get(), entry, &mut f);
            }
        }
        _ => {}
    }
    modified
}

/// Apply `f` to an inline `[projects."key"]` entry through a table view,
/// writing the result back inline only when `f` reported a change so an
/// untouched entry keeps its original formatting.
///
/// That write-back is what `f` owes in return: a rule that mutates the scope
/// table while reporting no change keeps its edit on the standard-table path
/// and loses it here, so the shape would decide the outcome again — the bug
/// this walk exists to close.
fn migrate_inline_scope<F>(scope: &str, entry: &mut toml_edit::Value, f: &mut F) -> bool
where
    F: FnMut(Option<&str>, &mut toml_edit::Table) -> bool,
{
    let toml_edit::Value::InlineTable(inline) = entry else {
        return false;
    };
    let mut as_table = inline.clone().into_table();
    if !f(Some(scope), &mut as_table) {
        return false;
    }
    *inline = as_table.into_inline_table();
    true
}

/// Keys `[switch.picker]` accepts — the destination of the `[select]` rename.
/// One schema, because only user config has a picker section; were a project
/// one added, this would need the union too, for the reason
/// [`COMMIT_GENERATION_KEYS`] gives.
static SWITCH_PICKER_KEYS: LazyLock<Vec<String>> =
    LazyLock::new(crate::config::schema_property_names::<crate::config::SwitchPickerConfig>);

/// Keys `[commit.generation]` accepts, across both config files: the user
/// schema plus the project one (a subset — `template-append` only). The union,
/// rather than the schema of the file being migrated, is what keeps a
/// user-only key written into project config on its "belongs in user config"
/// redirect (see `USER_ONLY_COMMIT_GENERATION_PATHS`) instead of dropping it.
static COMMIT_GENERATION_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut keys = crate::config::schema_property_names::<crate::config::CommitGenerationConfig>();
    keys.extend(crate::config::schema_property_names::<
        crate::config::ProjectCommitGenerationConfig,
    >());
    keys
});

/// What a section-rename migration did to one scope.
struct SectionMigration {
    /// Whether the replacement section was written. False when the rule
    /// declined, and when every key was dropped (nothing left to move).
    moved: bool,
    /// One [`DeprecationKind::UnsupportedKey`] per key removed for want of a
    /// destination field.
    dropped: Deprecations,
}

impl SectionMigration {
    /// The rule declined: nothing moved, nothing removed.
    fn none() -> Self {
        Self {
            moved: false,
            dropped: Vec::new(),
        }
    }

    /// Every key was removed, so there was nothing left to move.
    fn dropped_only(dropped: Deprecations) -> Self {
        Self {
            moved: false,
            dropped,
        }
    }
}

/// Display form of a deprecated section within its scope: `[select]` at the
/// top level, `[projects."<id>".select]` under a project entry.
fn section_display(scope: Option<&str>, section: &str) -> String {
    match scope {
        None => format!("[{section}]"),
        Some(project) => format!("[projects.\"{project}\".{section}]"),
    }
}

/// Remove every key `supported` doesn't list, reporting one
/// [`DeprecationKind::UnsupportedKey`] each in document order.
///
/// A rename that moves its table wholesale would otherwise carry a key with no
/// destination field to a path the user never typed. The unknown-field check
/// then names that path (`switch.picker.height` for a `[select] height`) on
/// every command, and `wt config update` — the command the deprecation hint
/// points at — writes the key there rather than clearing it, so the warning
/// outlives every fix available to the user. Removing the key closes the loop:
/// applying the update silences every warning it raised.
///
/// `supported` is the union of the destination's schemas across config files,
/// so a key that is merely *misplaced* (valid in the other config) survives to
/// collect its redirect warning; only a key with no home anywhere goes. Those
/// schemas must not use `#[serde(alias = "…")]`, which
/// [`schema_property_names`](crate::config::schema_property_names) cannot see;
/// an alias would have to be added here the way `schema_top_level_keys` adds
/// the `pre-create`/`post-create` hook aliases.
fn drop_unsupported_keys(
    table: &mut toml_edit::Table,
    supported: &[String],
    section: &str,
) -> Deprecations {
    let unsupported: Vec<String> = table
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !supported.iter().any(|s| s == key))
        .collect();
    unsupported
        .into_iter()
        .map(|key| {
            table.remove(&key);
            DeprecationKind::UnsupportedKey {
                section: section.to_string(),
                key,
            }
        })
        .collect()
}

/// Run a wholesale section move over every scope and report what it did.
///
/// This is the contract a wholesale-move rule owes, in one place: record the
/// rename only for a scope that actually got a replacement section written,
/// count a drop-only outcome as a change so the document is rewritten, and
/// emit the rename kind before the per-key drops. A third such rule is a row
/// and a `migrate_*_table`, not another copy of this.
///
/// `kind` is the tuple-variant constructor for the rename ([`DeprecationKind`]
/// variants coerce to `fn` pointers); the scopes it carries each emit their own
/// warning line.
fn migrate_section_doc(
    doc: &mut toml_edit::DocumentMut,
    migrate: impl Fn(&mut toml_edit::Table, Option<&str>) -> SectionMigration,
    kind: fn(ScopedSections) -> DeprecationKind,
) -> Deprecations {
    let mut renamed = ScopedSections::default();
    let mut dropped = Deprecations::new();
    for_each_config_table_mut(doc, |scope, table| {
        let outcome = migrate(table, scope);
        if outcome.moved {
            renamed.record(scope);
        }
        let changed = outcome.moved || !outcome.dropped.is_empty();
        dropped.extend(outcome.dropped);
        changed
    });
    let mut kinds = Deprecations::new();
    if !renamed.is_empty() {
        kinds.push(kind(renamed));
    }
    kinds.extend(dropped);
    kinds
}

/// Migrate `[commit-generation]` → `[commit.generation]` in every scope.
fn migrate_commit_generation_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    migrate_section_doc(
        doc,
        migrate_commit_generation_in,
        DeprecationKind::CommitGeneration,
    )
}

/// Migrate `[select]` → `[switch.picker]` in every scope.
fn migrate_select_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    migrate_section_doc(doc, migrate_select_table, DeprecationKind::Select)
}

/// Whether a TOML item is a table or inline table (can be migrated as a section).
fn is_table_like(item: &toml_edit::Item) -> bool {
    matches!(
        item,
        toml_edit::Item::Table(_) | toml_edit::Item::Value(toml_edit::Value::InlineTable(_))
    )
}

/// Whether a TOML item is a table or inline table with at least one entry.
/// Section-rename rules leave empty deprecated sections alone: they contribute
/// no config, so rewriting them isn't worth a warning.
fn is_nonempty_table_like(item: &toml_edit::Item) -> bool {
    table_like_len(item).is_some_and(|len| len > 0)
}

/// Whether a slot can host a freshly-inserted subtable: absent, or already a
/// (possibly inline) table. A scalar/array occupant blocks insertion — the
/// migration would remove the deprecated source but have nowhere to put
/// `[parent.child]`, silently dropping the user's data.
fn can_host_subtable(item: Option<&toml_edit::Item>) -> bool {
    item.is_none_or(is_table_like)
}

fn has_table_like_child(item: Option<&toml_edit::Item>, key: &str) -> bool {
    match item {
        Some(toml_edit::Item::Table(t)) => t.get(key).is_some_and(is_table_like),
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => t
            .get(key)
            .is_some_and(|v| matches!(v, toml_edit::Value::InlineTable(_))),
        _ => false,
    }
}

/// Replace a key's inline-table value with a standard table, carrying the line's
/// comments onto the table header.
///
/// The key was parsed from `merge = { … }`, so its leaf decor holds whatever
/// preceded the line — comments, blank lines — plus the space before `=`. A
/// standard table renders that decor *inside* its brackets, so leaving it in
/// place writes `[# comment\nmerge ]`: a config file wt can no longer parse,
/// and the user's own comment is what breaks it. Move the prefix to the header
/// and drop the rest.
///
/// A trailing comment after the closing brace sits in the inline value's own
/// decor, which `InlineTable::into_table` discards, so it is read from
/// `existing` before the replacement and lands after the header's `]`. It is
/// carried only when it holds a comment; bare whitespace there would just trail
/// the header.
fn replace_inline_with_table(
    existing: &mut toml_edit::Table,
    key: &str,
    mut table: toml_edit::Table,
) {
    let prefix = existing
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .filter(|prefix| prefix.as_str() != Some(""))
        .cloned();
    let suffix = existing
        .get(key)
        .and_then(|item| item.as_inline_table())
        .and_then(|inline| inline.decor().suffix())
        .filter(|suffix| suffix.as_str().is_some_and(|s| s.contains('#')))
        .cloned();
    if let Some(prefix) = prefix {
        table.decor_mut().set_prefix(prefix);
    }
    if let Some(suffix) = suffix {
        table.decor_mut().set_suffix(suffix);
    }
    if let Some(mut key_mut) = existing.key_mut(key) {
        key_mut.leaf_decor_mut().clear();
    }
    existing[key] = toml_edit::Item::Table(table);
}

/// Ensure a table-like parent is writable as a standard table.
///
/// Inline tables can deserialize like tables, but TOML forbids extending them
/// with later subtables. Convert before inserting migrated nested sections so
/// existing inline parent fields survive alongside the new child table.
///
/// The conversion goes through [`replace_inline_with_table`] so the
/// key's leading comments and blank lines land above the header rather than
/// inside its brackets. These rules run on the load path, so a header the key's
/// decor broke is a config file that stops parsing on every command, not just
/// one `wt config update` writes back.
fn ensure_standard_table_parent<'a>(
    table: &'a mut toml_edit::Table,
    key: &str,
) -> Option<&'a mut toml_edit::Table> {
    if !table.contains_key(key) {
        let mut parent = toml_edit::Table::new();
        parent.set_implicit(true);
        table.insert(key, toml_edit::Item::Table(parent));
    }

    if let Some(inline) = table
        .get(key)
        .and_then(|item| item.as_inline_table())
        .cloned()
    {
        replace_inline_with_table(table, key, inline.into_table());
    }
    table.get_mut(key)?.as_table_mut()
}

/// Convert a table-like TOML item into a `Table`. Returns `None` for other shapes.
fn into_table(item: toml_edit::Item) -> Option<toml_edit::Table> {
    match item {
        toml_edit::Item::Table(t) => Some(t),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => Some(it.into_table()),
        _ => None,
    }
}

/// Migrate one scope's `[commit-generation]` → `[commit.generation]`.
///
/// Skips when a canonical `[commit.generation]` already exists (new format takes
/// precedence). Peeks before removing so a malformed value (e.g. a bare string)
/// is left in place rather than silently dropped when a sibling migration also
/// serializes the doc; an empty section is likewise left alone — it contributes
/// no config, so rewriting it isn't worth a warning. Requires `commit` be
/// absent or a table — a scalar `commit = "x"` blocks insertion of
/// `[commit.generation]`, and removing the source then would silently drop the
/// deprecated section.
///
/// Keys `[commit.generation]` has no field for are dropped and reported (see
/// [`drop_unsupported_keys`]) rather than moved into a section that would
/// reject them. A section left empty by those drops writes no
/// `[commit.generation]` at all — removing the source is the whole change.
fn migrate_commit_generation_in(
    table: &mut toml_edit::Table,
    scope: Option<&str>,
) -> SectionMigration {
    if has_table_like_child(table.get("commit"), "generation")
        || !table
            .get("commit-generation")
            .is_some_and(is_nonempty_table_like)
        || !can_host_subtable(table.get("commit"))
    {
        return SectionMigration::none();
    }
    let Some(old_section) = table.remove("commit-generation") else {
        return SectionMigration::none();
    };
    let mut generation = into_table(old_section).expect("checked is_nonempty_table_like above");

    // Merge args into command if present.
    merge_args_into_command(&mut generation);

    let dropped = drop_unsupported_keys(
        &mut generation,
        &COMMIT_GENERATION_KEYS,
        &section_display(scope, "commit-generation"),
    );
    if generation.is_empty() {
        return SectionMigration::dropped_only(dropped);
    }

    // Ensure [commit] exists (implicit, so only [commit.generation] renders a
    // header) and move the migrated section under it.
    if let Some(commit_table) = ensure_standard_table_parent(table, "commit") {
        commit_table.insert("generation", toml_edit::Item::Table(generation));
    }
    SectionMigration {
        moved: true,
        dropped,
    }
}

/// Remove non-empty `approved-commands` arrays from `\[projects."..."\]`
/// sections (moved to approvals.toml).
///
/// An empty `approved-commands = []` is not deprecated and survives untouched;
/// a non-array value is left for serde's type error rather than removed
/// without ever having been copied to approvals.toml. A project section
/// emptied by the removal is dropped, as is a `\[projects\]` table emptied by
/// dropping its projects — but a pre-existing empty `\[projects\]` stays (this
/// rule didn't change it).
fn remove_approved_commands_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    let mut modified = false;

    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        let remove_from: Vec<String> = projects
            .iter()
            .filter(|(_, project_value)| {
                project_value.as_table().is_some_and(|t| {
                    t.get("approved-commands")
                        .and_then(|a| a.as_array())
                        .is_some_and(|a| !a.is_empty())
                })
            })
            .map(|(key, _)| key.to_string())
            .collect();

        for key in &remove_from {
            let project_table = projects
                .get_mut(key)
                .and_then(|v| v.as_table_mut())
                .expect("selected as a table above");
            project_table.remove("approved-commands");
            modified = true;
            if project_table.is_empty() {
                projects.remove(key);
            }
        }
    }

    if modified
        && doc
            .get("projects")
            .and_then(|p| p.as_table())
            .is_some_and(|t| t.is_empty())
    {
        doc.remove("projects");
    }

    if modified {
        vec![DeprecationKind::ApprovedCommands]
    } else {
        Vec::new()
    }
}

/// Migrate a non-empty `select` key to `switch.picker` within a table.
/// Skips when `[switch.picker]` already exists.
///
/// Leaves a malformed `select` (e.g., a string) in place rather than removing
/// it — silently dropping it would lose user config when a sibling migration
/// also rewrites the document. An empty section is likewise left alone — it
/// contributes no config, so rewriting it isn't worth a warning.
///
/// Keys `[switch.picker]` has no field for are dropped and reported (see
/// [`drop_unsupported_keys`]) rather than moved into a section that would
/// reject them. A `[select]` left empty by those drops writes no
/// `[switch.picker]` at all — removing the source is the whole change.
fn migrate_select_table(table: &mut toml_edit::Table, scope: Option<&str>) -> SectionMigration {
    let has_new_section = has_table_like_child(table.get("switch"), "picker");

    if has_new_section {
        return SectionMigration::none();
    }

    if !table.get("select").is_some_and(is_nonempty_table_like) {
        return SectionMigration::none();
    }

    // A scalar `switch = "x"` blocks insertion of `[switch.picker]`; removing
    // `select` then would silently drop the user's picker config.
    if !can_host_subtable(table.get("switch")) {
        return SectionMigration::none();
    }

    let mut select_table =
        into_table(table.remove("select").unwrap()).expect("checked is_nonempty_table_like above");

    let dropped = drop_unsupported_keys(
        &mut select_table,
        &SWITCH_PICKER_KEYS,
        &section_display(scope, "select"),
    );
    if select_table.is_empty() {
        return SectionMigration::dropped_only(dropped);
    }

    if let Some(switch_table) = ensure_standard_table_parent(table, "switch") {
        switch_table.insert("picker", toml_edit::Item::Table(select_table));
    }

    SectionMigration {
        moved: true,
        dropped,
    }
}

fn table_like_len(item: &toml_edit::Item) -> Option<usize> {
    match item {
        toml_edit::Item::Table(t) => Some(t.len()),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => Some(t.len()),
        _ => None,
    }
}

/// Migrate `[ci]` section to `[forge]`.
///
/// Moves `platform` from `[ci]` to `[forge]` when `platform` is a non-empty
/// string, preserving the value. Anything else — no `[ci]`, no `platform`, an
/// empty or non-string `platform` — is left untouched: there's nothing the
/// rewrite could meaningfully move. Removes `[ci]` if `platform` was its only
/// field.
///
/// What suppresses the migration is an occupied *destination key*, not the
/// presence of `forge`: a `[forge]` that already carries `platform` means the
/// user already migrated, and a `forge` that isn't a table (`forge = "x"`) has
/// no key to insert into — overwriting it would silently drop their config,
/// where serde's type error points at it instead. A `[forge]` table without
/// `platform` is the common half-migrated shape (`hostname` set for a GHE or
/// self-hosted GitLab remote, `platform` still back in `[ci]`), and its slot is
/// free, so `platform` lands there. Suppressing on the table's mere existence
/// left that user un-migrated *and* unwarned, so the deprecated key kept
/// working silently — right up until `[ci]` is removed.
///
/// A fresh `[forge]` takes over `[ci]`'s document position — a new table has no
/// position and would render at the end of the file instead of in the user's
/// original spot. The `platform` entry moves wholesale (key and item), so
/// comments attached to the line survive. When `[ci]` is fully consumed, its
/// decor (comments and blank lines above the header) moves to `[forge]` too;
/// when other keys keep `[ci]` alive, the decor stays there and `[forge]`
/// renders directly after the remainder — it shares `[ci]`'s position, the
/// position sort is stable, and `[forge]` is inserted later in visit order.
///
/// Inserting into an *existing* `[forge]` has no position to take over and no
/// home for `[ci]`'s decor: `[forge]` is wherever the user wrote it, which can
/// be far from the comment above `[ci]`. So an emptied `[ci]` is removed only
/// when its own decor is blank — a commented one stays as an empty section,
/// which contributes no config, raises no warning, and keeps the user's prose
/// where they put it.
fn migrate_ci_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    // Read the destination before touching `[ci]`, so a suppressed migration
    // leaves the document byte-identical.
    let forge_slot = match doc.get("forge") {
        None => ForgeSlot::Absent,
        Some(forge) => match forge.as_table() {
            Some(table) if !table.contains_key("platform") => ForgeSlot::FreeSlot,
            // `platform` already there, or `forge` is a scalar, an array, or an
            // inline table we can't extend without reformatting what the user
            // wrote.
            _ => return Vec::new(),
        },
    };

    let Some(ci_table) = doc.get_mut("ci").and_then(|ci| ci.as_table_mut()) else {
        return Vec::new();
    };
    if ci_table
        .get("platform")
        .is_none_or(|p| p.as_str().is_none_or(str::is_empty))
    {
        return Vec::new();
    }

    // Move only the migrated entry; keep any other keys so we don't silently
    // drop config that wasn't part of the migration.
    let (key, item) = ci_table
        .remove_entry("platform")
        .expect("checked platform exists above");
    let ci_emptied = ci_table.is_empty();
    let ci_position = ci_table.position();
    let ci_decor = ci_table.decor().clone();

    match forge_slot {
        ForgeSlot::Absent => {
            let mut forge_table = toml_edit::Table::new();
            forge_table.insert_formatted(&key, item);
            forge_table.set_position(ci_position);
            if ci_emptied {
                *forge_table.decor_mut() = ci_decor;
                doc.remove("ci");
            }
            doc.insert("forge", toml_edit::Item::Table(forge_table));
        }
        ForgeSlot::FreeSlot => {
            let forge_table = doc
                .get_mut("forge")
                .and_then(|forge| forge.as_table_mut())
                .expect("checked forge is a table above");
            forge_table.insert_formatted(&key, item);
            // A `[forge]` that exists only because of a sub-table renders no
            // header of its own until something is written directly into it.
            forge_table.set_implicit(false);
            if ci_emptied && decor_prefix_is_blank(&ci_decor) {
                doc.remove("ci");
            }
        }
    }

    vec![DeprecationKind::CiSection]
}

/// Where the migrated `platform` key is headed — see [`migrate_ci_doc`].
enum ForgeSlot {
    /// No `forge` key at all: build the table and take over `[ci]`'s position.
    Absent,
    /// A `[forge]` table whose `platform` slot is free: insert in place.
    FreeSlot,
}

/// Whether a table's leading decor carries nothing worth keeping (no comment,
/// just whitespace). Decor that was never parsed from a document reads as
/// blank — it holds no user text either way.
fn decor_prefix_is_blank(decor: &toml_edit::Decor) -> bool {
    decor
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .is_none_or(|prefix| prefix.trim().is_empty())
}

/// Migrate a negated boolean field within a table (e.g., `no-ff = true` →
/// `ff = false`).
///
/// When the new key is already present it takes precedence: the deprecated
/// key is removed without inverting it into a value. A non-bool old value is
/// left in place for the unknown-field warning rather than silently dropped.
///
/// Returns true if a migration was performed.
fn migrate_negated_bool(table: &mut toml_edit::Table, old_key: &str, new_key: &str) -> bool {
    let Some(old_val) = table
        .get(old_key)
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_bool())
    else {
        return false;
    };
    table.remove(old_key);
    if !table.contains_key(new_key) {
        table.insert(new_key, toml_edit::value(!old_val));
    }
    true
}

/// The inline-table counterpart of [`migrate_negated_bool`], for a section
/// written inline as `merge = { no-ff = true }`. `toml_edit` surfaces a regular
/// `[merge]` section and an inline `merge = { … }` as different node types, so
/// both forms need their own walk.
fn migrate_negated_bool_inline(
    table: &mut toml_edit::InlineTable,
    old_key: &str,
    new_key: &str,
) -> bool {
    let Some(old_val) = table.get(old_key).and_then(|v| v.as_bool()) else {
        return false;
    };
    table.remove(old_key);
    if !table.contains_key(new_key) {
        table.insert(new_key, toml_edit::Value::from(!old_val));
    }
    true
}

/// Migrate a negated boolean field in a section and its project-level
/// counterparts, reporting `kind` when any scope changed. Handles a section
/// written as a standard table (`[merge]`) or inline (`merge = { … }`).
fn migrate_negated_bool_doc(
    doc: &mut toml_edit::DocumentMut,
    section: &str,
    old_key: &str,
    new_key: &str,
    kind: DeprecationKind,
) -> Deprecations {
    if for_each_config_table_mut(doc, |_, scope| match scope.get_mut(section) {
        Some(toml_edit::Item::Table(table)) => migrate_negated_bool(table, old_key, new_key),
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(table))) => {
            migrate_negated_bool_inline(table, old_key, new_key)
        }
        _ => false,
    }) {
        vec![kind]
    } else {
        Vec::new()
    }
}

/// Apply the load-path migrations — every [`DeprecationRule::Structural`] and
/// [`DeprecationRule::Silent`] rule — to a parsed document, in table order.
/// Returns true if any modifications were made.
///
/// [`DeprecationRule::UpdateOnly`] rules are excluded — template variable
/// renaming is cosmetic (would break `--var` overrides), and approved-commands
/// is still a valid serde field. They apply in [`compute_migrated_content`].
fn migrate_content_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    apply_rules(doc, RulePass::Load, &mut Vec::new())
}

/// Apply the load-path migrations to `doc`, reporting what they changed.
///
/// The config mutations use this where [`migrate_content_doc`]'s bool isn't
/// enough: writing an edit into a migrated file removes the keys the
/// destinations have no field for, which the mutation names to the user.
pub(crate) fn migrate_doc(doc: &mut toml_edit::DocumentMut) -> Deprecations {
    let mut deprecations = Vec::new();
    apply_rules(doc, RulePass::Load, &mut deprecations);
    deprecations
}

/// Rename the `pre-create`/`post-create` hook aliases to `pre-start`/`post-start`,
/// in every config scope (see [`for_each_config_table_mut`]).
fn canonicalize_hook_keys(doc: &mut toml_edit::DocumentMut) -> bool {
    for_each_config_table_mut(doc, |_, table| {
        let pre = rename_hook_key(table, "pre-create", "pre-start");
        let post = rename_hook_key(table, "post-create", "post-start");
        pre || post
    })
}

/// Rename `old_key` to `new_key` in `table`, keeping the comment lines above it.
///
/// Skips a table where `new_key` already exists — the user has already
/// consolidated there, and clobbering their canonical value would lose config.
/// The rewrite preserves the value shape (string, `[table]`, or
/// `[[array-of-tables]]`) since it moves the `Item` unchanged.
fn rename_hook_key(table: &mut toml_edit::Table, old_key: &str, new_key: &str) -> bool {
    if table.contains_key(new_key) {
        return false;
    }
    let Some((key, item)) = table.remove_entry(old_key) else {
        return false;
    };
    let renamed = toml_edit::Key::new(new_key).with_leaf_decor(key.leaf_decor().clone());
    table.insert_formatted(&renamed, item);
    true
}

/// Remove `key` from a top-level `section` in a table, dropping a
/// `[projects."<id>"]`-scoped section that the removal empties.
///
/// A default section serializes away, so an empty one is absent from the
/// round-trip `unknown_tree` compares against. `seed_schema_skeleton` puts
/// every valid *top-level* key back, which is why an emptied top-level
/// `[list]` reads as known and an emptied `[projects."<id>".list]` reads as
/// `unknown field projects.<id>.list`. So only the project scope needs the
/// section removed; a top-level one keeps its position and the comments above
/// its header. (An empty nested section the user wrote by hand warns the same
/// way — that false positive belongs to `unknown_tree`, and this rule only
/// avoids adding to it.)
///
/// A section can be written as a section table (`[list]`) or inline
/// (`list = { … }`); `toml_edit` surfaces these as different node types, so
/// each shape gets its own branch — matching the inline-aware `no-cd`/`no-ff`
/// rules.
fn remove_section_key_in(
    table: &mut toml_edit::Table,
    scope: Option<&str>,
    section: &str,
    key: &str,
) -> bool {
    let (removed, now_empty) = match table.get_mut(section) {
        Some(toml_edit::Item::Table(t)) => (t.remove(key).is_some(), t.is_empty()),
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(it))) => {
            (it.remove(key).is_some(), it.is_empty())
        }
        _ => return false,
    };
    if removed && now_empty && scope.is_some() {
        table.remove(section);
    }
    removed
}

fn migrate_content_from_doc(content: &str, mut doc: toml_edit::DocumentMut) -> String {
    if migrate_content_doc(&mut doc) {
        doc.to_string()
    } else {
        content.to_string()
    }
}

/// Apply all TOML-level migrations to config content.
///
/// Parses the TOML, applies all structural migrations, and returns the result.
/// Called by load paths that only need structural migration. `check_and_migrate()`
/// reuses the same migration path when it also needs to emit warnings.
pub fn migrate_content(content: &str) -> String {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return content.to_string();
    };
    migrate_content_from_doc(content, doc)
}

/// Copy approved-commands from config.toml to approvals.toml.
///
/// Called by `wt config update` before overwriting the config with migrated
/// content, so the approvals data survives the rewrite. `Ok(None)` for the
/// benign no-op cases (a valid `approvals.toml` already exists, or the config
/// has no approved-commands entries). Returns `Err` when the existing
/// approvals file cannot be validated or a copy was attempted but failed — the
/// caller must abort before rewriting config.toml, otherwise the legacy
/// approvals are silently lost.
pub fn copy_approved_commands_to_approvals_file(
    config_path: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let approvals_path = config_path.with_file_name("approvals.toml");
    let _lock = super::user::mutation::acquire_config_lock(&approvals_path)?;
    if approvals_path.exists() {
        validate_existing_approvals_file(&approvals_path)?;
        return Ok(None); // Already authoritative, don't overwrite
    }

    let approvals =
        super::approvals::Approvals::load_from_config_file(config_path).with_context(|| {
            format!(
                "Failed to read approved-commands from {} for migration",
                config_path.display()
            )
        })?;
    if approvals.projects().next().is_none() {
        return Ok(None); // Nothing to copy
    }

    approvals.save_to(&approvals_path).with_context(|| {
        format!(
            "Failed to write migrated approvals to {}",
            approvals_path.display()
        )
    })?;
    Ok(Some(approvals_path))
}

fn validate_existing_approvals_file(approvals_path: &Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(approvals_path).with_context(|| {
        format!(
            "Failed to read existing approvals file {}",
            crate::path::format_path_for_display(approvals_path)
        )
    })?;
    toml::from_str::<super::approvals::Approvals>(&content).with_context(|| {
        format!(
            "Failed to parse existing approvals file {}",
            crate::path::format_path_for_display(approvals_path)
        )
    })?;
    Ok(())
}

/// Merge args array into command string
///
/// Converts: command = "llm", args = ["-m", "haiku"]
/// To: command = "llm -m haiku"
///
/// Merging needs a string `command` and an array of strings; anything else
/// leaves both keys as the user wrote them. `[commit.generation]` has no
/// `args` field, so an unmerged `args` is removed and reported by
/// [`drop_unsupported_keys`]: `args` was only ever a way of spelling part of
/// `command`, and one that can't be merged has nowhere to go.
///
/// Declining keeps that removal visible. Joining `args = [1, "--ok"]` would
/// drop the `1`, rewrite `command` with a value the user never wrote, and
/// report neither; dropping the whole key shows up in the deprecation warning
/// and in the `wt config update` diff.
fn merge_args_into_command(table: &mut toml_edit::Table) {
    let Some(args) = table
        .get("args")
        .and_then(|a| a.as_array())
        .and_then(|a| a.iter().map(|v| v.as_str()).collect::<Option<Vec<_>>>())
    else {
        return;
    };
    // Join before taking `command` mutably, while `args` is still borrowed.
    // An empty `args` merges away without touching `command`.
    let joined = (!args.is_empty()).then(|| shell_join(&args));

    let Some(command) = table.get_mut("command").and_then(|c| c.as_value_mut()) else {
        return;
    };
    let Some(cmd) = command.as_str() else {
        return;
    };
    if let Some(joined) = joined {
        let merged = if cmd.is_empty() {
            joined
        } else {
            format!("{cmd} {joined}")
        };
        *command = toml_edit::Value::from(merged);
    }
    table.remove("args");
}

/// Join arguments with proper shell quoting using shell_escape
fn shell_join(args: &[&str]) -> String {
    args.iter()
        .map(|arg| escape(Cow::Borrowed(*arg)).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Information about deprecated config patterns that were found.
///
/// Detection result plus display context (paths, labels). No filesystem side
/// effects — `check_and_migrate` never touches the filesystem; `wt config
/// update` rewrites the config and copies approvals under an explicit user
/// action.
#[derive(Debug)]
pub struct DeprecationInfo {
    /// Path to the config file with deprecations
    pub config_path: PathBuf,
    /// All detected deprecations
    pub deprecations: Deprecations,
    /// Which config file this is; derives the display label.
    pub kind: ConfigFileKind,
    /// Main worktree path when viewing from a linked worktree (for `-C` in hints)
    pub main_worktree_path: Option<PathBuf>,
}

impl DeprecationInfo {
    /// Returns true if any deprecations were found.
    pub fn has_deprecations(&self) -> bool {
        !self.deprecations.is_empty()
    }

    /// Display label for this config file (e.g., "User config").
    pub fn label(&self) -> &'static str {
        self.kind.label()
    }
}

/// Result of checking config content for deprecations.
///
/// `migrated_content` is the structurally migrated TOML used for serde loading.
/// `info` is present only when user-visible deprecations were detected.
#[derive(Debug)]
pub struct CheckAndMigrateResult {
    pub info: Option<DeprecationInfo>,
    pub migrated_content: String,
}

/// Check config content for deprecated patterns.
///
/// Detects:
/// - Deprecated template variables (repo_root → repo_path, etc.)
/// - Deprecated [commit-generation] sections → [commit.generation]
/// - Deprecated args field (merged into command)
/// - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
///
/// Pure with respect to the filesystem — never rewrites config or copies
/// approvals. The user materializes migrations by running `wt config update`
/// or exports the result instead of applying it in place via
/// `wt config update --output <path>`. Deprecation warnings still go to stderr
/// when `emit_inline_warnings` is set.
///
/// Set `warn_and_migrate` to false when project config is not actionable. A
/// linked worktree cannot update the file, but read-only output remains
/// actionable and passes true.
///
/// `kind` names the config file being checked and derives the warning label.
///
/// `repo` is used to resolve the primary worktree path for the "run this from
/// the main worktree" hint when viewing project config from a linked worktree.
///
/// When `emit_inline_warnings` is true, per-kind deprecation warnings are printed to stderr
/// with a hint pointing at `wt config show`/`wt config update`. When false, nothing is
/// printed and the caller is expected to render via `format_deprecation_details`. Use this for commands other than `config show`.
///
/// Warnings are deduplicated per path per process.
///
/// Returns the structurally migrated content for serde loading, plus optional
/// deprecation info when user-visible deprecations were found.
pub fn check_and_migrate(
    path: &Path,
    content: &str,
    warn_and_migrate: bool,
    kind: ConfigFileKind,
    repo: Option<&crate::git::Repository>,
    emit_inline_warnings: bool,
) -> anyhow::Result<CheckAndMigrateResult> {
    // Parse once — shared by detection and migration.
    // Contract: unparsable content collapses to empty deprecations so downstream
    // `compute_migrated_content` (invoked by `config show`/`config update` only when
    // `info` is `Some`) can assume the content parses.
    let (deprecations, migrated_content) = match content.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => {
            let deprecations = detect_deprecations_from_doc(&doc);
            let migrated_content = migrate_content_from_doc(content, doc);
            (deprecations, migrated_content)
        }
        Err(_) => (Vec::new(), content.to_string()),
    };

    if deprecations.is_empty() {
        return Ok(CheckAndMigrateResult {
            info: None,
            migrated_content,
        });
    }

    let info = DeprecationInfo {
        config_path: path.to_path_buf(),
        deprecations,
        kind,
        main_worktree_path: if !warn_and_migrate {
            repo.and_then(|r| r.repo_path().ok())
                .map(|p| p.to_path_buf())
        } else {
            None
        },
    };

    // Skip warning entirely if not in main worktree (for project config)
    if !warn_and_migrate {
        return Ok(CheckAndMigrateResult {
            info: Some(info),
            migrated_content,
        });
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_DEPRECATED_PATHS
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock deprecation warning tracker: {e}"))?;
        if guard.contains(&canonical_path) {
            return Ok(CheckAndMigrateResult {
                info: Some(info),
                migrated_content,
            });
        }
        guard.insert(canonical_path);
    }

    // For non-config-show commands, emit per-kind warnings but skip the diff.
    // The diff is reserved for `wt config show`, where the user has opted into
    // details.
    if emit_inline_warnings && !warnings_suppressed() {
        let warnings = format_warning_lines(info.deprecations.iter(), info.label());
        eprint!("{warnings}");
        if DEPRECATION_HINT_EMITTED.set(()).is_ok() {
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "To see details, run <underline>wt config show</>; to apply updates, run <underline>wt config update</>"
                ))
            );
        }
        std::io::stderr().flush().ok();
    }

    Ok(CheckAndMigrateResult {
        info: Some(info),
        migrated_content,
    })
}

/// Apply all deprecation fixes to `content` in memory and return the migrated
/// TOML string.
///
/// Applies variable renames (cosmetic, string-level), structural section and
/// field migrations, and removes `approved-commands` under `[projects]` (which
/// `wt config update` copies to `approvals.toml` before overwriting).
///
/// Pure function — no filesystem access. Idempotent: feeding its own output
/// back in is a no-op. Callers materialize the result via `wt config update`
/// or display it via `wt config show`.
pub fn compute_migrated_content(content: &str) -> String {
    // Callers (`wt config show`, `wt config update`, `format_deprecation_details`)
    // all run content through `check_and_migrate` first, so it is known to parse.
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .expect("compute_migrated_content called with content that failed TOML parse; callers must funnel through check_and_migrate first");

    if apply_rules(&mut doc, RulePass::Update, &mut Vec::new()) {
        doc.to_string()
    } else {
        content.to_string()
    }
}

/// Render the `Proposed diff:` block for a migration, or a warning line when
/// git cannot produce the patch.
///
/// The three outcomes of `format_migration_diff` stay distinct here: an
/// identical pair renders nothing, a differing pair renders the patch, and a
/// git failure renders a warning rather than disappearing. Both consumers
/// (`wt config show` and `wt config update`) go through this so neither can
/// present a failed diff as "no changes"; the migration itself is computed in
/// memory and is unaffected, so a broken renderer degrades the preview rather
/// than failing the command.
///
/// Returns a string ending in a newline, or empty when there is nothing to show.
pub fn format_migration_diff_block(original: &str, migrated: &str, label: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    match format_migration_diff(original, migrated, label) {
        Ok(Some(diff)) => {
            let _ = writeln!(out, "{}", info_message("Proposed diff:"));
            let _ = writeln!(out, "{diff}");
        }
        Ok(None) => {}
        Err(e) => {
            let _ = writeln!(
                out,
                "{}",
                warning_message("Could not render the proposed diff")
            );
            // `{e:#}` rather than `to_string()`: the git-failure arm bails with
            // the whole payload, but a spawn or tempfile failure carries its
            // cause one `.context` layer down, and plain Display drops it.
            let _ = writeln!(out, "{}", format_with_gutter(&format!("{e:#}"), None));
        }
    }
    out
}

/// Render a colored unified diff between `original` and `migrated`, with
/// `label` shown as the file name in the diff header (e.g. `config.toml`).
///
/// Uses a private tempdir containing two files named `<label>/current` and
/// `<label>/migrated`; `git diff --no-index` is invoked from inside that
/// tempdir so the diff header shows clean relative paths. The tempdir is
/// dropped on return. Returns `Ok(None)` when the contents match.
///
/// `--no-ext-diff` keeps the patch worktrunk's own: a user's `diff.external`
/// program would otherwise be handed these two temp files and could emit
/// something that isn't a patch, block on a GUI, or die and take the preview
/// with it.
///
/// `git diff --no-index` exits 0 when the files match and 1 when they differ,
/// so those two are the answer and anything else is a failure. Branching on
/// stdout alone conflated "no changes" with "git refused to run" — the shape
/// this guards against (#4118).
fn format_migration_diff(
    original: &str,
    migrated: &str,
    label: &str,
) -> anyhow::Result<Option<String>> {
    let dir = tempfile::tempdir().context("failed to create tempdir for migration diff")?;
    let subdir = dir.path().join(label);
    std::fs::create_dir(&subdir).context("failed to create subdir in fresh tempdir")?;
    std::fs::write(subdir.join("current"), original)
        .context("failed to write current config to tempfile")?;
    std::fs::write(subdir.join("migrated"), migrated)
        .context("failed to write migrated config to tempfile")?;

    let output = Cmd::new("git")
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--color=always",
            "-U3",
            "--",
        ])
        .arg(format!("{label}/current"))
        .arg(format!("{label}/migrated"))
        .current_dir(dir.path())
        .run()
        .context("failed to run git diff --no-index")?;

    match output.status.code() {
        Some(0) => Ok(None),
        Some(1) => Ok(Some(format_with_gutter(
            String::from_utf8_lossy(&output.stdout).trim_end(),
            None,
        ))),
        // `ExitStatus`'s own rendering covers a signal-killed child too, so
        // there is no separate arm for one.
        _ => anyhow::bail!(
            "git diff --no-index, {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ),
    }
}

/// Format deprecation warning lines (without apply hints or diff).
///
/// Lists which deprecated patterns were found: template variables, config sections,
/// approved-commands. Used by both `format_deprecation_details` (which adds the
/// `wt config update` hint and diff) and `wt config update` (which applies directly).
pub fn format_deprecation_warnings(info: &DeprecationInfo) -> String {
    format_warning_lines(&info.deprecations, info.label())
}

/// Render one `warning_message` line per kind (the commit-generation kind can
/// emit several). The kinds arrive in emission order, so a single pass
/// reproduces the original output verbatim.
///
/// A rewrite that has already happened is named by [`format_applied_lines`]
/// instead: these lines say what is still outstanding.
fn format_warning_lines<'a>(
    kinds: impl IntoIterator<Item = &'a DeprecationKind>,
    label: &str,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    for kind in kinds {
        match kind {
            DeprecationKind::TemplateVar { old, new } => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: template variable <bold>{old}</> is deprecated in favor of <bold>{new}</>"
                    ))
                );
            }
            DeprecationKind::CommitGeneration(scopes) => {
                if scopes.has_top_level {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[commit-generation]</> is deprecated in favor of <bold>[commit.generation]</>"
                        ))
                    );
                }
                for k in &scopes.project_keys {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[projects.\"{k}\".commit-generation]</> is deprecated in favor of <bold>[projects.\"{k}\".commit.generation]</>"
                        ))
                    );
                }
            }
            DeprecationKind::ApprovedCommands => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>approved-commands</> under <bold>[projects]</> is deprecated in favor of <bold>approvals.toml</>"
                    ))
                );
            }
            DeprecationKind::Select(scopes) => {
                if scopes.has_top_level {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[select]</> is deprecated in favor of <bold>[switch.picker]</>"
                        ))
                    );
                }
                for k in &scopes.project_keys {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[projects.\"{k}\".select]</> is deprecated in favor of <bold>[projects.\"{k}\".switch.picker]</>"
                        ))
                    );
                }
            }
            DeprecationKind::UnsupportedKey { section, key } => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>{section} {key}</> is no longer supported and will be removed"
                    ))
                );
            }
            DeprecationKind::CiSection => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>[ci]</> is deprecated in favor of <bold>[forge]</>"
                    ))
                );
            }
            DeprecationKind::NoFf => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>merge.no-ff</> is deprecated in favor of <bold>merge.ff</> (inverted)"
                    ))
                );
            }
            DeprecationKind::NoCd => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>switch.no-cd</> is deprecated in favor of <bold>switch.cd</> (inverted)"
                    ))
                );
            }
            DeprecationKind::ListTaskTimeout => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>list.task-timeout-ms</> is no longer used — <bold>list.timeout-ms</> bounds the collect phase"
                    ))
                );
            }
        }
    }

    out
}

/// Render one `warning_message` line per kind, for migrations already applied.
///
/// The sibling of [`format_warning_lines`], in the tense the config mutations
/// need: they report what their write just did to the file, where the load
/// path's "is deprecated in favor of" and "will be removed" name work still
/// ahead. Both live here so a new [`DeprecationKind`] is worded in one place.
pub(crate) fn format_applied_lines<'a>(
    kinds: impl IntoIterator<Item = &'a DeprecationKind>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut line = |text: String| {
        let _ = writeln!(out, "{}", warning_message(text));
    };

    for kind in kinds {
        match kind {
            DeprecationKind::TemplateVar { old, new } => line(cformat!(
                "Renamed template variable <bold>{old}</> to <bold>{new}</>"
            )),
            DeprecationKind::CommitGeneration(scopes) => {
                if scopes.has_top_level {
                    line(cformat!(
                        "Moved <bold>[commit-generation]</> to <bold>[commit.generation]</>"
                    ));
                }
                for k in &scopes.project_keys {
                    line(cformat!(
                        "Moved <bold>[projects.\"{k}\".commit-generation]</> to <bold>[projects.\"{k}\".commit.generation]</>"
                    ));
                }
            }
            DeprecationKind::ApprovedCommands => line(cformat!(
                "Moved <bold>approved-commands</> under <bold>[projects]</> to <bold>approvals.toml</>"
            )),
            DeprecationKind::Select(scopes) => {
                if scopes.has_top_level {
                    line(cformat!(
                        "Moved <bold>[select]</> to <bold>[switch.picker]</>"
                    ));
                }
                for k in &scopes.project_keys {
                    line(cformat!(
                        "Moved <bold>[projects.\"{k}\".select]</> to <bold>[projects.\"{k}\".switch.picker]</>"
                    ));
                }
            }
            DeprecationKind::UnsupportedKey { section, key } => line(cformat!(
                "Removed <bold>{section} {key}</>, which its replacement has no field for"
            )),
            DeprecationKind::CiSection => line(cformat!("Moved <bold>[ci]</> to <bold>[forge]</>")),
            DeprecationKind::NoFf => line(cformat!(
                "Replaced <bold>merge.no-ff</> with <bold>merge.ff</> (inverted)"
            )),
            DeprecationKind::NoCd => line(cformat!(
                "Replaced <bold>switch.no-cd</> with <bold>switch.cd</> (inverted)"
            )),
            DeprecationKind::ListTaskTimeout => line(cformat!(
                "Removed <bold>list.task-timeout-ms</>, which nothing reads"
            )),
        }
    }

    out
}

/// Format deprecation details for display (for use by `wt config show`).
///
/// Returns formatted output including:
/// - Warning message listing deprecated patterns
/// - Migration hint with apply command
/// - Inline diff showing the changes
///
/// `original_content` is the current on-disk config; the migrated content is
/// derived in memory via [`compute_migrated_content`] so this function has no
/// filesystem side effects other than the tempdir used briefly for `git diff`.
pub fn format_deprecation_details(info: &DeprecationInfo, original_content: &str) -> String {
    use std::fmt::Write;
    let mut out = format_deprecation_warnings(info);

    if let Some(main_path) = &info.main_worktree_path {
        // In a linked worktree — the user needs to run update from the primary.
        let cmd = suggest_command_in_dir(main_path, "config", &["update"], &[]);
        let _ = writeln!(
            out,
            "{}",
            hint_message(cformat!("To apply: <underline>{cmd}</>"))
        );
        return out;
    }

    let _ = writeln!(
        out,
        "{}",
        hint_message(cformat!("To apply: <underline>wt config update</>"))
    );

    let migrated = compute_migrated_content(original_content);
    let label = info
        .config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    out.push_str(&format_migration_diff_block(
        original_content,
        &migrated,
        &label,
    ));

    out
}

/// Returns the config location where this key belongs, if it's in the wrong config.
///
/// Generic over `C`, the config type where the key was found. If the key would
/// be valid in `C::Other`, returns that config's description.
///
/// For example, `key_belongs_in::<ProjectConfig>("skip-shell-integration-prompt")`
/// returns `Some("user config")`.
/// Returns `None` if the key is truly unknown (not valid in either config).
pub fn key_belongs_in<C: WorktrunkConfig>(key: &str) -> Option<&'static str> {
    C::Other::is_valid_key(key).then(C::Other::description)
}

/// `[commit.generation]` sub-keys (canonical/migrated form) that belong only
/// in user config.
///
/// `[commit.generation]` is itself a valid *project* config section — but only
/// for `template-append`, the project-wide commit convention shared across the
/// team. The LLM `command` and the full prompt templates are resolved from
/// user/system config only. Putting them in a project `.config/wt.toml` is a
/// common, easily-missed mistake (see #2774).
///
/// `is_valid_key` only knows top-level keys, so misplaced *nested* keys can't
/// be classified by [`key_belongs_in`]. Since `commit` is now a legitimate
/// project section, the round-trip flags just these offending leaves; without
/// this list they'd surface as a bare "unknown field" instead of a redirect.
///
/// These paths are schema-valid in user config, so they only ever surface as
/// unknown-nested under *project* config; [`nested_key_belongs_in`] therefore
/// needs no config-type gate. The list is kept in sync with
/// `CommitGenerationConfig` by `user_only_commit_generation_paths_track_schema`.
const USER_ONLY_COMMIT_GENERATION_PATHS: &[&str] = &[
    "commit.generation.command",
    "commit.generation.template",
    "commit.generation.squash-template",
];

/// Returns the config where a misplaced *nested* key belongs.
///
/// The nested analog of [`key_belongs_in`], for the one case that needs a
/// redirect: user-only `[commit.generation]` keys placed in project config
/// (see `USER_ONLY_COMMIT_GENERATION_PATHS`). Returns `None` for ordinary
/// unknown nested paths (typos), which stay "unknown field".
pub fn nested_key_belongs_in<C: WorktrunkConfig>(path: &str) -> Option<&'static str> {
    USER_ONLY_COMMIT_GENERATION_PATHS
        .contains(&path)
        .then(C::Other::description)
}

/// Note appended to a "belongs in the other config" warning when the
/// misplaced key also has a `[projects."<id>"]` form in user config, which is
/// usually what the user was reaching for. A user key in project config is an
/// attempt to scope a personal setting to one repo; a project key in user
/// config (`forge`) is an attempt to set it without touching the repo's
/// committed file. The `[projects."<id>"]` table does both directly.
///
/// `key` is the misplaced key (`worktree-path`, or a dotted path like
/// `list.columns`); the note only fires when its top-level segment is a field
/// of that table (see [`is_user_project_override_key`](crate::config::is_user_project_override_key)).
/// Root-only user settings — `skip-shell-integration-prompt`,
/// `skip-commit-generation-prompt` — have no `[projects."<id>"]` form and no
/// per-repo semantics, so following the note would just produce a fresh
/// "unknown field"; they get no note.
///
/// Keyed off the destination *description* rather than a config-type gate so
/// both warning formatters (load-time and `config show`) can share it — they
/// hold only the `other_description` string, not the config type.
pub fn scope_to_repo_note(other_description: &str, key: &str) -> Option<&'static str> {
    let top_level = key.split('.').next().unwrap_or(key);
    if !crate::config::is_user_project_override_key(top_level) {
        return None;
    }
    if other_description == crate::config::UserConfig::description() {
        return Some(r#"to scope it to this repo, add it under [projects."<id>"] in user config"#);
    }
    // The reverse redirect — a project-config key found in user config — earns
    // the note only for a whole top-level table (`forge`): its
    // `[projects."<id>"]` field has the same shape, so the move is valid as
    // written. A nested path (`list.url`) names a field the projects-entry
    // type doesn't have, and its correct home really is project config.
    (other_description == crate::config::ProjectConfig::description() && !key.contains('.'))
        .then_some(r#"to set it from user config, add it under [projects."<id>"]"#)
}

/// Classification of an unknown config key for warning purposes.
pub enum UnknownKeyKind {
    /// Deprecated key in its correct config type — the deprecation system
    /// owns the message. Registration is all this says: whether the migration
    /// actually ran is the caller's to check (see `collect_unknown_warnings`),
    /// since a declined rewrite leaves the key unread and unreported.
    DeprecatedHandled,
    /// Deprecated key in the wrong config type
    DeprecatedWrongConfig {
        other_description: &'static str,
        canonical_display: &'static str,
    },
    /// Non-deprecated key that belongs in the other config type
    WrongConfig { other_description: &'static str },
    /// Truly unknown key (not valid in either config type)
    Unknown,
}

/// Classify an unknown config key: deprecated (right/wrong file), misplaced, or unknown.
pub fn classify_unknown_key<C: WorktrunkConfig>(key: &str) -> UnknownKeyKind {
    if let Some(dep) = DEPRECATED_SECTION_KEYS.iter().find(|d| d.key == key) {
        return if C::is_valid_key(dep.canonical_top_key) {
            UnknownKeyKind::DeprecatedHandled
        } else {
            UnknownKeyKind::DeprecatedWrongConfig {
                other_description: C::Other::description(),
                canonical_display: dep.canonical_display,
            }
        };
    }
    match key_belongs_in::<C>(key) {
        Some(other) => UnknownKeyKind::WrongConfig {
            other_description: other,
        },
        None => UnknownKeyKind::Unknown,
    }
}

/// Warn about unknown fields in a config file.
///
/// Generic over `C`, the config type being loaded. Classification is shared
/// with `config show` via [`collect_unknown_warnings`](crate::config::collect_unknown_warnings);
/// this wrapper adds per-path deduplication and stderr emission.
///
/// `kind` derives the label shown in the warning message.
pub fn warn_unknown_fields<C: WorktrunkConfig>(
    raw_contents: &str,
    path: &Path,
    kind: ConfigFileKind,
) {
    let label = kind.label();
    if warnings_suppressed() {
        return;
    }

    let warnings = crate::config::collect_unknown_warnings::<C>(raw_contents);
    if warnings.is_empty() {
        return;
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_UNKNOWN_PATHS.lock().unwrap();
        if guard.contains(&canonical_path) {
            return; // Already warned, skip
        }
        guard.insert(canonical_path);
    }

    for warning in warnings {
        eprintln!("{}", warning_message(format_load_warning(label, &warning)));
    }

    // Flush stderr to ensure output appears before any subsequent messages
    std::io::stderr().flush().ok();
}

fn format_load_warning(label: &str, warning: &crate::config::UnknownWarning) -> String {
    use crate::config::UnknownWarning;
    match warning {
        UnknownWarning::TopLevelUnknown { key } => {
            cformat!("{label} has unknown field <bold>{key}</> (will be ignored)")
        }
        UnknownWarning::TopLevelWrongConfig {
            key,
            other_description,
        } => with_scope_note(
            cformat!(
                "{label} has key <bold>{key}</> which belongs in {other_description} (will be ignored)"
            ),
            other_description,
            key,
        ),
        UnknownWarning::TopLevelDeprecatedWrongConfig {
            key,
            other_description,
            canonical_display,
        } => cformat!(
            "{label} has key <bold>{key}</> which belongs in {other_description} as {canonical_display}"
        ),
        UnknownWarning::NestedWrongConfig {
            path,
            other_description,
        } => with_scope_note(
            cformat!(
                "{label} has key <bold>{path}</> which belongs in {other_description} (will be ignored)"
            ),
            other_description,
            path,
        ),
        UnknownWarning::NestedUnknown { path } => {
            cformat!("{label} has unknown field <bold>{path}</> (will be ignored)")
        }
    }
}

/// Append the project-scoped-user-config note to `message` when the misplaced
/// `key`'s destination is user config and it's a `[projects."<id>"]` field
/// (see [`scope_to_repo_note`]). Joined with a semicolon per the house style
/// for related clauses. The note is plain text so its `[projects."<id>"]`
/// placeholder isn't parsed as color-print markup. Shared with `config show`
/// via [`crate::config::with_scope_note`] so the caveat lives in one place.
pub fn with_scope_note(message: String, other_description: &str, key: &str) -> String {
    match scope_to_repo_note(other_description, key) {
        Some(note) => format!("{message}; {note}"),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;
    use insta::assert_snapshot;

    /// `USER_ONLY_COMMIT_GENERATION_PATHS` must stay in sync with
    /// `CommitGenerationConfig`: every key except the project-valid
    /// `template-append`. If a field is added to that struct without updating
    /// the list, a misplaced key would silently degrade to "unknown field".
    #[test]
    fn user_only_commit_generation_paths_track_schema() {
        let mut expected =
            crate::config::schema_property_names::<crate::config::CommitGenerationConfig>();
        expected.retain(|k| k != "template-append");
        let mut expected: Vec<String> = expected
            .iter()
            .map(|k| format!("commit.generation.{k}"))
            .collect();
        expected.sort();

        let mut actual: Vec<String> = USER_ONLY_COMMIT_GENERATION_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();
        actual.sort();

        assert_eq!(actual, expected);
    }

    // Test helpers bridging string fixtures to the entry points. The find_*
    // helpers extract one kind from `detect_deprecations`; migration tests
    // call `migrate_content` / `compute_migrated_content` directly. The
    // template-var and approved-commands helpers wrap internal functions
    // whose isolation or parse-failure semantics have no public seam.

    fn replace_deprecated_vars(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if migrate_template_vars_doc(&mut doc).is_empty() {
            return content.to_string();
        }
        // `toml_edit` always serializes a document with a trailing newline.
        // These helper tests pass fragments without one and assert on the
        // substituted text, not serialization shape — mirror the input.
        let out = doc.to_string();
        if !content.ends_with('\n') {
            out.strip_suffix('\n').map(str::to_owned).unwrap_or(out)
        } else {
            out
        }
    }

    fn find_deprecated_vars(content: &str) -> Vec<(&'static str, &'static str)> {
        detect_deprecations(content)
            .into_iter()
            .filter_map(|k| match k {
                DeprecationKind::TemplateVar { old, new } => Some((old, new)),
                _ => None,
            })
            .collect()
    }

    /// True when `deprecations` carries a kind matching `pred`. Lets the
    /// per-kind tests assert presence of a single deprecation against the
    /// `Vec<DeprecationKind>`, the way they used to read a boolean field.
    fn has_kind(deprecations: &Deprecations, pred: impl Fn(&DeprecationKind) -> bool) -> bool {
        deprecations.iter().any(pred)
    }

    fn find_commit_generation_deprecations(content: &str) -> ScopedSections {
        detect_deprecations(content)
            .into_iter()
            .find_map(|k| match k {
                DeprecationKind::CommitGeneration(found) => Some(found),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn find_approved_commands_deprecation(content: &str) -> bool {
        has_kind(&detect_deprecations(content), |k| {
            matches!(k, DeprecationKind::ApprovedCommands)
        })
    }

    fn find_select_deprecation(content: &str) -> bool {
        has_kind(&detect_deprecations(content), |k| {
            matches!(k, DeprecationKind::Select(_))
        })
    }

    fn remove_approved_commands_from_config(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if remove_approved_commands_doc(&mut doc).is_empty() {
            content.to_string()
        } else {
            doc.to_string()
        }
    }

    #[test]
    fn test_find_deprecated_vars_empty() {
        let content = r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
"#;
        let found = find_deprecated_vars(content);
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_deprecated_vars_repo_root() {
        let content = r#"
post-start = "ln -sf {{ repo_root }}/node_modules node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_worktree() {
        let content = r#"
post-start = "cd {{ worktree }} && npm install"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("worktree", "worktree_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_main_worktree() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("main_worktree", "repo")]);
    }

    #[test]
    fn test_find_deprecated_vars_main_worktree_path() {
        let content = r#"
post-start = "ln -sf {{ main_worktree_path }}/node_modules ."
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("main_worktree_path", "primary_worktree_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_multiple() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
post-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(
            found,
            vec![
                ("repo_root", "repo_path"),
                ("worktree", "worktree_path"),
                ("main_worktree", "repo"),
            ]
        );
    }

    /// Every retired variable is rewritten on load, not just by
    /// `wt config update`: nothing supplies the old names at render time, so a
    /// template that reached a renderer un-rewritten would fail its expansion
    /// (`worktree-path`) or render nothing (`squash-template`). Detection
    /// reports the same pairs either way.
    #[test]
    fn test_retired_vars_migrate_on_load_and_on_update() {
        let content = r#"worktree-path = "../{{ repo_root }}.{{ branch }}"

[commit.generation]
squash-template = "{% for c in commits %}{{ c }}\n{% endfor %}"
"#;
        assert_eq!(
            find_deprecated_vars(content),
            vec![("repo_root", "repo_path"), ("commits", "commit_details")]
        );

        for (label, migrated) in [
            ("load", migrate_content(content)),
            ("update", compute_migrated_content(content)),
        ] {
            assert!(
                migrated.contains("{{ repo_path }}")
                    && migrated.contains("for c in commit_details"),
                "{label} must rewrite both retired vars: {migrated}"
            );
            assert!(
                !migrated.contains("repo_root") && !migrated.contains("in commits"),
                "{label} must leave no retired var behind: {migrated}"
            );
        }
    }

    #[test]
    fn test_find_deprecated_vars_with_filter() {
        let content = r#"
post-start = "ln -sf {{ repo_root | something }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_deduplicates() {
        let content = r#"
post-start = "{{ repo_root }}/a {{ repo_root }}/b"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_does_not_match_suffix() {
        // Should NOT match "worktree_path" when looking for "worktree"
        let content = r#"
post-start = "cd {{ worktree_path }} && npm install"
"#;
        let found = find_deprecated_vars(content);
        assert!(
            found.is_empty(),
            "Should not match worktree_path as worktree"
        );
    }

    #[test]
    fn test_replace_deprecated_vars_simple() {
        let content = r#"cmd = "{{ repo_root }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{ repo_path }}""#);
    }

    #[test]
    fn test_replace_deprecated_vars_with_filter() {
        let content = r#"cmd = "{{ repo_root | sanitize }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{ repo_path | sanitize }}""#);
    }

    /// Regression: when the deprecated var sits next to an escaped quote, the
    /// decoded string value does not appear verbatim in the raw file text, so
    /// the old `str::replace`-on-content path silently skipped the migration
    /// while detection still warned. The toml_edit-tree rewrite handles it.
    #[test]
    fn test_replace_deprecated_vars_with_escaped_quotes() {
        // Source TOML: pre-start = "echo \"{{ repo_root }}\""
        let content = r#"pre-start = "echo \"{{ repo_root }}\"""#;
        let result = replace_deprecated_vars(content);
        assert!(
            !result.contains("repo_root"),
            "deprecated var must be migrated even with escaped quotes; got: {result}"
        );
        assert!(
            result.contains("repo_path"),
            "migrated var must be present; got: {result}"
        );
    }

    /// Same, exercised through the public `compute_migrated_content` entry.
    #[test]
    fn test_compute_migrated_content_escaped_quotes() {
        let content = "pre-start = \"echo \\\"{{ repo_root }}\\\"\"\n";
        let migrated = compute_migrated_content(content);
        assert!(
            !migrated.contains("repo_root"),
            "compute_migrated_content must migrate vars inside escaped strings; got: {migrated}"
        );
        assert!(migrated.contains("repo_path"));
    }

    #[test]
    fn test_replace_deprecated_vars_no_spaces() {
        let content = r#"cmd = "{{repo_root}}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{repo_path}}""#); // Preserves original formatting
    }

    #[test]
    fn test_replace_deprecated_vars_filter_no_spaces() {
        let content = r#"cmd = "{{repo_root|sanitize}}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{repo_path|sanitize}}""#); // Preserves original formatting
    }

    #[test]
    fn test_replace_deprecated_vars_multiple() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
post-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
post-start = "ln -sf {{ repo_path }}/node_modules {{ worktree_path }}/node_modules"
"#
        );
    }

    #[test]
    fn test_replace_deprecated_vars_preserves_other_content() {
        let content = r#"
# This is a comment
worktree-path = "../{{ repo }}.{{ branch }}"

[hooks]
post-start = "echo hello"
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, content); // No changes since no deprecated vars
    }

    #[test]
    fn test_replace_deprecated_vars_preserves_whitespace() {
        let content = r#"cmd = "{{  repo_root  }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{  repo_path  }}""#); // Preserves original formatting
    }

    /// The tree walker must recurse into arrays of tables and inline tables —
    /// not just top-level tables — so a deprecated var is migrated wherever it
    /// appears, while non-string scalars are left untouched.
    #[test]
    fn test_replace_deprecated_vars_walks_array_of_tables_and_inline_table() {
        let content = r#"
[[steps]]
run = "build {{ repo_root }}"

[env]
script = { cmd = "{{ repo_root }}/x" }
timeout = 30
"#;
        let result = replace_deprecated_vars(content);
        assert!(
            result.contains("build {{ repo_path }}"),
            "array-of-tables var migrated: {result}"
        );
        assert!(
            result.contains("{{ repo_path }}/x"),
            "inline-table var migrated: {result}"
        );
        assert!(
            result.contains("timeout = 30"),
            "non-string scalar left untouched: {result}"
        );
    }

    /// `into_table` underpins every "peek before remove" structural migration:
    /// callers gate on `is_table_like`, so the non-table arm must return `None`
    /// rather than panic if that contract is ever violated.
    #[test]
    fn test_into_table_returns_none_for_non_table() {
        let scalar = toml_edit::Item::Value(toml_edit::Value::from(5));
        assert!(into_table(scalar).is_none());
    }

    /// Canonical config with no deprecations must round-trip through
    /// `compute_migrated_content` byte-for-byte (the unmodified branch).
    #[test]
    fn test_compute_migrated_content_noop_returns_input_unchanged() {
        let content = "pre-start = \"echo {{ repo_path }}\"\n";
        assert_eq!(compute_migrated_content(content), content);
    }

    #[test]
    fn test_compute_migrated_content_does_not_rewrite_literal_text_when_other_template_uses_deprecated_var()
     {
        let content = "pre-merge = \"echo repo_root\"\npost-merge = \"echo {{ repo_root }}\"\n";
        let migrated = compute_migrated_content(content);
        assert_eq!(
            migrated,
            "pre-merge = \"echo repo_root\"\npost-merge = \"echo {{ repo_path }}\"\n"
        );
    }

    /// The `replace_deprecated_vars` helper must return the input untouched
    /// when it cannot be parsed as TOML, rather than panicking.
    #[test]
    fn test_replace_deprecated_vars_returns_input_on_parse_error() {
        let content = "this is = = not valid toml";
        assert_eq!(replace_deprecated_vars(content), content);
    }

    #[test]
    fn test_replace_does_not_match_suffix() {
        // Should NOT replace "worktree_path" when looking for "worktree"
        let content = r#"cmd = "{{ worktree_path }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result, r#"cmd = "{{ worktree_path }}""#,
            "Should not modify worktree_path"
        );
    }

    #[test]
    fn test_replace_in_statement_blocks() {
        let content = r#"cmd = "{% if repo_root %}echo {{ repo_root }}{% endif %}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"cmd = "{% if repo_path %}echo {{ repo_path }}{% endif %}""#
        );
    }

    // Tests for normalize_template_vars (single template string normalization)

    #[test]
    fn test_normalize_no_deprecated_vars() {
        let template = "ln -sf {{ repo_path }}/node_modules";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_does_not_rewrite_literal_text() {
        let template = "echo repo_root";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_only_rewrites_template_identifiers() {
        let template = "echo repo_root && echo {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "echo repo_root && echo {{ repo_path }}");
    }

    /// When `repo_root` is bound as a `{% set %}` local it is no longer the
    /// deprecated global, so minijinja reports no undeclared `repo_root` and
    /// the template is left untouched — the local name is not silently renamed.
    #[test]
    fn test_normalize_skips_set_assignment_target() {
        let template = "{% set repo_root = \"x\" %}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    /// A `}}` inside a quoted string does not end the tag. Scanning for the
    /// first textual `}}` cut the tag short, so the real reference after the
    /// string was never reached, and literal text in a later string was
    /// rewritten as though it were a tag.
    #[test]
    fn test_normalize_ignores_delimiters_inside_quoted_strings() {
        assert_eq!(
            normalize_template_vars(r#"{{ "}} " ~ repo_root }}"#),
            r#"{{ "}} " ~ repo_path }}"#
        );
        // The literal `{{ repo_root }}` inside the string is output text, not
        // a reference; only the real tag after it changes.
        assert_eq!(
            normalize_template_vars(r#"{{ "}} {{ repo_root }}" }}{{ repo_root }}"#),
            r#"{{ "}} {{ repo_root }}" }}{{ repo_path }}"#
        );
        // Same for a single-quoted string and a block tag.
        assert_eq!(
            normalize_template_vars(r#"{% if '%}' ~ repo_root %}x{% endif %}"#),
            r#"{% if '%}' ~ repo_path %}x{% endif %}"#
        );
    }

    /// A deprecated name that a block binds somewhere in the template is left
    /// alone everywhere: the rewriter has no scope tracking, so renaming the
    /// global reference would also rename the later local use. Binding the
    /// canonical name holds the rename back for the same reason from the other
    /// side — the rename would walk a genuine global use into that binding.
    /// Either way the pair is dropped from the replacement set that detection
    /// reads too, so the template stops warning as well — the price of not
    /// renaming half a scope.
    #[test]
    fn test_normalize_skips_names_bound_by_a_block() {
        for template in [
            // read as the global first, then rebound and read as the local
            r#"{{ repo_root }}{% set repo_root = "local" %}{{ repo_root }}"#,
            r#"{{ repo_root }}{% for repo_root in items %}{{ repo_root }}{% endfor %}"#,
            r#"{{ repo_root }}{% with repo_root = "local" %}{{ repo_root }}{% endwith %}"#,
            // `+` is whitespace control just as `-` is, on either end
            r#"{{ repo_root }}{%+ set repo_root = "local" %}{{ repo_root }}"#,
            r#"{{ repo_root }}{% set repo_root = "local" +%}{{ repo_root }}"#,
            // parenthesized tuple targets, which MiniJinja's `parse_assignment`
            // accepts for all three keywords and nests arbitrarily
            r#"{{ repo_root }}{% for (repo_root, x) in items %}{{ repo_root }}{% endfor %}"#,
            r#"{{ repo_root }}{% for (a, (repo_root, x)) in items %}{{ repo_root }}{% endfor %}"#,
            r#"{{ repo_root }}{% set (repo_root, x) = items %}{{ repo_root }}"#,
            r#"{{ repo_root }}{% with (repo_root, x) = items %}{{ repo_root }}{% endwith %}"#,
            // the second pair of a multi-assignment `with`
            r#"{{ repo_root }}{% with a = 1, repo_root = 2 %}{{ repo_root }}{% endwith %}"#,
            // `set`'s block form binds its target before the filter chain the
            // `|` arm cuts the target region at
            r#"{{ repo_root }}{% set repo_root %}b{% endset %}{{ repo_root }}"#,
            r#"{{ repo_root }}{% set repo_root | default(1) %}b{% endset %}{{ repo_root }}"#,
            // binding the *canonical* name captures the use the rename
            // produces: `{{ repo_root }}` reads the global here and the local
            // once it is spelled `repo_path`
            r#"{{ repo_root }}{% set repo_path = "local" %}{{ repo_root }}"#,
            r#"{{ repo_root }}{% for repo_path in items %}{{ repo_root }}{% endfor %}"#,
            r#"{{ repo_root }}{% with repo_path = "local" %}{{ repo_root }}{% endwith %}"#,
            // the same collision in the squash template's pair
            r#"{% for commit_details in items %}{{ commits }}{% endfor %}"#,
        ] {
            let result = normalize_template_vars(template);
            assert!(
                matches!(result, Cow::Borrowed(_)),
                "should not rewrite: {template}"
            );
            assert_eq!(result, template);
            assert!(
                detect_deprecations(&format!("worktree-path = \"{template}\"\n")).is_empty(),
                "a template left unrewritten must not warn: {template}"
            );
        }
    }

    /// `{% raw %}` content is literal text to MiniJinja, so a binding-shaped
    /// tag inside one binds nothing and must not hold back a real use outside
    /// it. The raw contents are left exactly as written, `+` whitespace
    /// control included.
    #[test]
    fn test_normalize_ignores_raw_block_contents() {
        assert_eq!(
            normalize_template_vars(
                r#"{% raw %}{% set repo_root = "x" %}{% endraw %}{{ repo_root }}"#
            ),
            r#"{% raw %}{% set repo_root = "x" %}{% endraw %}{{ repo_path }}"#
        );
        assert_eq!(
            normalize_template_vars("{{ repo_root }}{%+ raw %}{{ repo_root }}{%+ endraw %}"),
            "{{ repo_path }}{%+ raw %}{{ repo_root }}{%+ endraw %}"
        );
    }

    /// A template MiniJinja can't parse has no reading to migrate against, so
    /// it is left untouched rather than guessed at — and it raises no warning
    /// either, since detection reads the same parse.
    #[test]
    fn test_normalize_leaves_an_unparsable_template_untouched() {
        let template = "{{ repo_root";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "should not rewrite");
        assert_eq!(result, template);
        assert!(
            detect_deprecations(&format!("worktree-path = \"{template}\"\n")).is_empty(),
            "a template left unrewritten must not warn"
        );
    }

    /// `{% raw %}` content is literal text, so a tag opened inside one never
    /// has to be terminated. The hand-rolled scan this replaced had to find
    /// that tag's end itself and gave up on the whole template; MiniJinja's
    /// parser reads the raw block as the literal it is, and the reference
    /// before it migrates.
    #[test]
    fn test_normalize_migrates_past_a_tag_opened_inside_a_raw_block() {
        assert_eq!(
            normalize_template_vars(r#"{{ repo_root }}{% raw %}{{ " {% endraw %}"#),
            r#"{{ repo_path }}{% raw %}{{ " {% endraw %}"#
        );
    }

    /// Binding detection only looks at the binding keywords — a deprecated
    /// name merely *used* inside a block tag still migrates.
    #[test]
    fn test_normalize_rewrites_names_used_but_not_bound_in_blocks() {
        assert_eq!(
            normalize_template_vars("{% if repo_root %}{{ repo_root }}{% endif %}"),
            "{% if repo_path %}{{ repo_path }}{% endif %}"
        );
        assert_eq!(
            normalize_template_vars("{% for x in repo_root %}{{ x }}{% endfor %}"),
            "{% for x in repo_path %}{{ x }}{% endfor %}"
        );
        assert_eq!(
            normalize_template_vars("{% set p = repo_root %}{{ p }}"),
            "{% set p = repo_path %}{{ p }}"
        );
        // A value region the target scan must not claim: a `for` filter, a
        // comparison whose `=` is not an assignment, a collection literal.
        assert_eq!(
            normalize_template_vars("{% for x in items if repo_root %}{{ x }}{% endfor %}"),
            "{% for x in items if repo_path %}{{ x }}{% endfor %}"
        );
        assert_eq!(
            normalize_template_vars(r#"{% set a = repo_root == "x" %}{{ a }}"#),
            r#"{% set a = repo_path == "x" %}{{ a }}"#
        );
        assert_eq!(
            normalize_template_vars("{% with a = [repo_root, 1] %}{{ a }}{% endwith %}"),
            "{% with a = [repo_path, 1] %}{{ a }}{% endwith %}"
        );
        // A dotted target mutates an attribute of the global rather than
        // binding it, so the global still migrates.
        assert_eq!(
            normalize_template_vars("{% set repo_root.x = 1 %}{{ repo_root }}"),
            "{% set repo_path.x = 1 %}{{ repo_path }}"
        );
        // `set` takes one assignment, so the comma after its `=` builds a
        // tuple value — the name beside it reads the global and migrates,
        // unlike the second pair of a `with`.
        assert_eq!(
            normalize_template_vars("{% set a = 1, repo_root %}{{ repo_root }}"),
            "{% set a = 1, repo_path %}{{ repo_path }}"
        );
        // A `set` block's filter chain is value too: only `x` is bound, so
        // the argument and the later global both migrate.
        assert_eq!(
            normalize_template_vars("{% set x | default(repo_root) %}b{% endset %}{{ repo_root }}"),
            "{% set x | default(repo_path) %}b{% endset %}{{ repo_path }}"
        );
        // The squash-template migration this must not regress.
        assert_eq!(
            normalize_template_vars("{% for c in commits %}{{ c }}{% endfor %}"),
            "{% for c in commit_details %}{{ c }}{% endfor %}"
        );
    }

    /// Identifier positions that are not variable reads: a keyword argument's
    /// name and a map key. Neither resolves against the render context, so
    /// neither is rewritten — while a real reference beside it still is.
    #[test]
    fn test_normalize_skips_identifiers_that_are_not_reads() {
        assert_eq!(
            normalize_template_vars("{{ dict(repo_root=1) }}{{ repo_root }}"),
            "{{ dict(repo_root=1) }}{{ repo_path }}"
        );
        assert_eq!(
            normalize_template_vars(r#"{{ {"repo_root": repo_root} }}"#),
            r#"{{ {"repo_root": repo_path} }}"#
        );
    }

    /// The parser hands reads back in visit order, which is not source order:
    /// a conditional expression is visited test-first, and a map's keys all
    /// precede its values. Two migrations whose visit order inverts their
    /// positions must still splice into one coherent template.
    #[test]
    fn test_normalize_rewrites_reads_out_of_visit_order() {
        assert_eq!(
            normalize_template_vars("{{ repo_root if worktree }}"),
            "{{ repo_path if worktree_path }}"
        );
        assert_eq!(
            normalize_template_vars("{{ {a: repo_root, worktree: b} }}"),
            "{{ {a: repo_path, worktree_path: b} }}"
        );
    }

    /// `{% do %}` is a statement of its own, not an expression emitted by a
    /// `{{ }}`, so its call's arguments are reads like any other.
    #[test]
    fn test_normalize_rewrites_inside_a_do_statement() {
        assert_eq!(
            normalize_template_vars("{% do dict(x=repo_root) %}"),
            "{% do dict(x=repo_path) %}"
        );
    }

    /// A read is a read wherever the expression puts it, so every node the
    /// walk descends through has to reach the `Expr::Var` underneath. One case
    /// per shape the parser can produce.
    #[test]
    fn test_normalize_rewrites_reads_in_every_expression_shape() {
        for (template, expected) in [
            ("{{ items[repo_root:] }}", "{{ items[repo_path:] }}"),
            (
                "{{ items[:repo_root:worktree] }}",
                "{{ items[:repo_path:worktree_path] }}",
            ),
            ("{{ items[repo_root] }}", "{{ items[repo_path] }}"),
            ("{{ not repo_root }}", "{{ not repo_path }}"),
            ("{{ -repo_root }}", "{{ -repo_path }}"),
            // `Expr::Compare` needs a *chained* comparison: `parse_compare`
            // lowers a single one to `Expr::BinOp`, so `repo_root == "x"`
            // never reaches the arm that walks the operand list.
            ("{{ 1 < repo_root < 3 }}", "{{ 1 < repo_path < 3 }}"),
            // A conditional's `else` arm, which the visit-order case above
            // leaves off.
            ("{{ a if b else repo_root }}", "{{ a if b else repo_path }}"),
            (
                "{{ repo_root ~ worktree }}",
                "{{ repo_path ~ worktree_path }}",
            ),
            ("{{ repo_root is defined }}", "{{ repo_path is defined }}"),
            (
                "{{ repo_root | default(worktree) }}",
                "{{ repo_path | default(worktree_path) }}",
            ),
            ("{{ dict(**repo_root) }}", "{{ dict(**repo_path) }}"),
            ("{{ dict(*repo_root) }}", "{{ dict(*repo_path) }}"),
            (
                "{{ repo_root(worktree) }}",
                "{{ repo_path(worktree_path) }}",
            ),
            (
                "{% autoescape repo_root %}{{ worktree }}{% endautoescape %}",
                "{% autoescape repo_path %}{{ worktree_path }}{% endautoescape %}",
            ),
            (
                "{% filter upper %}{{ repo_root }}{% endfilter %}",
                "{% filter upper %}{{ repo_path }}{% endfilter %}",
            ),
            (
                "{% for x in items %}{% else %}{{ repo_root }}{% endfor %}",
                "{% for x in items %}{% else %}{{ repo_path }}{% endfor %}",
            ),
            (
                "{% if a %}{% else %}{{ repo_root }}{% endif %}",
                "{% if a %}{% else %}{{ repo_path }}{% endif %}",
            ),
        ] {
            assert_eq!(
                normalize_template_vars(template),
                expected,
                "for: {template}"
            );
        }
    }

    /// Identifiers inside `{# #}` comments must not be rewritten.
    #[test]
    fn test_normalize_skips_comment_tags() {
        let template = "{# repo_root #}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{# repo_root #}{{ repo_path }}");
    }

    /// Identifiers inside `{% raw %}…{% endraw %}` blocks are verbatim text,
    /// not template references, so they must be left alone — only a genuine
    /// reference outside the raw block is rewritten.
    #[test]
    fn test_normalize_skips_raw_blocks() {
        let template = "{% raw %}{{ repo_root }}{% endraw %}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(
            result,
            "{% raw %}{{ repo_root }}{% endraw %}{{ repo_path }}"
        );
    }

    /// A deprecated name that appears as a quoted string literal inside a tag
    /// is not an identifier and must not be rewritten.
    #[test]
    fn test_normalize_skips_string_literals_in_tags() {
        let template = "{{ \"repo_root\" }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ \"repo_root\" }} {{ repo_path }}");
    }

    /// A deprecated name used as an attribute (`obj.repo_root`) is a member of
    /// another value, not the deprecated global, so it must not be rewritten.
    #[test]
    fn test_normalize_skips_attribute_access() {
        let template = "{{ obj.repo_root }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ obj.repo_root }} {{ repo_path }}");
    }

    /// A bare `{` that does not open a tag is literal text; the scan steps past
    /// it and still rewrites a genuine reference later in the string.
    #[test]
    fn test_normalize_skips_bare_brace() {
        let template = "{ literal {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{ literal {{ repo_path }}");
    }

    /// A backslash-escaped quote inside an in-tag string literal does not end
    /// the literal early, so its contents are preserved verbatim.
    #[test]
    fn test_normalize_handles_escaped_quote_in_tag_string() {
        let template = "{{ \"a\\\"repo_root\" }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ \"a\\\"repo_root\" }} {{ repo_path }}");
    }

    #[test]
    fn test_normalize_repo_root() {
        let template = "ln -sf {{ repo_root }}/node_modules";
        let result = normalize_template_vars(template);
        assert_eq!(result, "ln -sf {{ repo_path }}/node_modules");
    }

    #[test]
    fn test_normalize_worktree() {
        let template = "cd {{ worktree }} && npm install";
        let result = normalize_template_vars(template);
        assert_eq!(result, "cd {{ worktree_path }} && npm install");
    }

    #[test]
    fn test_normalize_main_worktree() {
        let template = "../{{ main_worktree }}.{{ branch }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "../{{ repo }}.{{ branch }}");
    }

    #[test]
    fn test_normalize_multiple_vars() {
        let template = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules";
        let result = normalize_template_vars(template);
        assert_eq!(
            result,
            "ln -sf {{ repo_path }}/node_modules {{ worktree_path }}/node_modules"
        );
    }

    #[test]
    fn test_normalize_does_not_match_suffix() {
        // Should NOT replace "worktree_path" when looking for "worktree"
        let template = "cd {{ worktree_path }}";
        let result = normalize_template_vars(template);
        // Note: may allocate due to coarse quick check, but result is unchanged
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_with_filter() {
        let template = "{{ repo_root | sanitize }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ repo_path | sanitize }}");
    }

    // Tests for approved-commands array handling

    #[test]
    fn test_find_deprecated_vars_in_array_of_tables() {
        // Exercises the ArrayOfTables arm in collect_strings_from_edit_item
        let content = r#"
[[hooks]]
command = "ln -sf {{ repo_root }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_in_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_root }}/node_modules",
    "cd {{ worktree }} && npm install",
]
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(
            found,
            vec![("repo_root", "repo_path"), ("worktree", "worktree_path"),]
        );
    }

    #[test]
    fn test_replace_deprecated_vars_in_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_root }}/node_modules",
    "cd {{ worktree }} && npm install",
]
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_path }}/node_modules",
    "cd {{ worktree_path }} && npm install",
]
"#
        );
    }

    #[test]
    fn test_check_and_migrate_write_failure() {
        // Test the write error path by using a non-existent directory
        let content = "[merge]\nno-ff = true\n";
        let non_existent_path = std::path::Path::new("/nonexistent/dir/config.toml");

        // Should return Ok(Some(_)) even if write fails - the function logs error but doesn't fail
        let result = check_and_migrate(
            non_existent_path,
            content,
            true,
            ConfigFileKind::User,
            None,
            false,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().info.is_some());
    }

    #[test]
    fn test_check_and_migrate_deduplicates_warnings() {
        // Test that calling twice with same path skips the second warning
        let content = "[merge]\nno-ff = true\n";
        // Use a unique path that won't collide with other tests
        let unique_path = std::path::Path::new("/nonexistent/dedup_test_12345/config.toml");

        // First call should process normally
        let result1 = check_and_migrate(
            unique_path,
            content,
            true,
            ConfigFileKind::User,
            None,
            false,
        );
        assert!(result1.is_ok());
        assert!(result1.unwrap().info.is_some());

        // Second call with same path should early-return (hits the deduplication branch)
        let result2 = check_and_migrate(
            unique_path,
            content,
            true,
            ConfigFileKind::User,
            None,
            false,
        );
        assert!(result2.is_ok());
        assert!(result2.unwrap().info.is_some());
    }

    #[test]
    fn test_check_and_migrate_returns_migrated_content() {
        let content = r#"
[select]
pager = "delta"
"#;

        let result = check_and_migrate(
            std::path::Path::new("/tmp/config.toml"),
            content,
            true,
            ConfigFileKind::User,
            None,
            false,
        )
        .unwrap();

        assert_eq!(result.migrated_content, migrate_content(content));
        assert!(result.info.is_some());
    }

    // Tests for commit-generation section migration

    #[test]
    fn test_find_commit_generation_deprecations_none() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_commit_generation_deprecations_top_level() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level);
        assert!(result.project_keys.is_empty());
    }

    #[test]
    fn test_find_commit_generation_deprecations_project_level() {
        let content = r#"
[projects."github.com/user/repo".commit-generation]
command = "llm -m gpt-4"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(!result.has_top_level);
        assert_eq!(result.project_keys, vec!["github.com/user/repo"]);
    }

    #[test]
    fn test_find_commit_generation_deprecations_multiple_projects() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"

[projects."github.com/user/repo1".commit-generation]
command = "llm -m gpt-4"

[projects."github.com/user/repo2".commit-generation]
command = "llm -m opus"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level);
        assert_eq!(result.project_keys.len(), 2);
        assert!(
            result
                .project_keys
                .contains(&"github.com/user/repo1".to_string())
        );
        assert!(
            result
                .project_keys
                .contains(&"github.com/user/repo2".to_string())
        );
    }

    #[test]
    fn test_migrate_commit_generation_args_with_spaces() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude haiku 4.5"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "llm -m 'claude haiku 4.5'"
        "#);
    }

    #[test]
    fn test_migrate_commit_generation_preserves_other_fields() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
template = "Write commit: {{ diff }}"
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "llm -m haiku"
        template = "Write commit: {{ diff }}"
        "#);
    }

    #[test]
    fn test_migrate_no_changes_needed() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_migrate_skips_when_new_section_exists() {
        let content = r#"
[commit.generation]
command = "new-command"

[commit-generation]
command = "old-command"
"#;
        let result = migrate_content(content);
        // Old section left as-is since new already exists
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "new-command"

        [commit-generation]
        command = "old-command"
        "#);
    }

    #[test]
    fn test_find_deprecations_skips_when_new_section_exists() {
        // When new section exists, don't flag old section as deprecated
        let content = r#"
[commit.generation]
command = "new-command"

[commit-generation]
command = "old-command"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag deprecation when new section exists"
        );
    }

    #[test]
    fn test_find_deprecations_skips_empty_section() {
        // Empty old section should not be flagged
        let content = r#"
[commit-generation]
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag empty deprecated section"
        );
    }

    #[test]
    fn test_shell_join_simple() {
        assert_eq!(shell_join(&["-m", "haiku"]), "-m haiku");
    }

    #[test]
    fn test_shell_join_with_spaces() {
        assert_eq!(shell_join(&["-m", "claude haiku"]), "-m 'claude haiku'");
    }

    #[test]
    fn test_shell_join_with_quotes() {
        assert_eq!(shell_join(&["echo", "it's"]), r"echo 'it'\''s'");
    }

    #[test]
    fn test_combined_migrations_template_vars_and_section_rename() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#;
        let step1 = replace_deprecated_vars(content);
        let step2 = migrate_content(&step1);
        insta::assert_snapshot!(step2, @r#"

        worktree-path = "../{{ repo }}.{{ branch }}"

        [commit.generation]
        command = "llm -m haiku"
        "#);
    }

    // Tests for inline table handling

    #[test]
    fn test_find_deprecations_inline_table_top_level() {
        // Inline table format: commit-generation = { command = "llm" }
        let content = r#"
commit-generation = { command = "llm -m haiku" }
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level, "Should detect inline table format");
    }

    #[test]
    fn test_find_deprecations_inline_table_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = { command = "llm -m gpt-4" }
"#;
        let result = find_commit_generation_deprecations(content);
        assert_eq!(
            result.project_keys,
            vec!["github.com/user/repo"],
            "Should detect project-level inline table"
        );
    }

    #[test]
    fn test_migrate_inline_table_top_level() {
        let content = r#"
commit-generation = { command = "llm", args = ["-m", "haiku"] }
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[commit.generation]") || result.contains("[commit]"),
            "Should migrate inline table"
        );
        assert!(
            result.contains("command = \"llm -m haiku\""),
            "Should merge args into command"
        );
        assert!(
            !result.contains("commit-generation"),
            "Should remove old inline table"
        );
    }

    #[test]
    fn test_find_deprecations_malformed_generation_not_table() {
        // If commit.generation is a string (malformed), should still warn about old format
        let content = r#"
[commit]
generation = "not a table"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            result.has_top_level,
            "Should flag deprecated section when new section is malformed"
        );
    }

    #[test]
    fn test_migrate_inline_table_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = { command = "llm", args = ["-m", "gpt-4"] }
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[projects.\"github.com/user/repo\".commit.generation]")
                || result.contains("[projects.\"github.com/user/repo\".commit]"),
            "Should migrate project-level inline table"
        );
        assert!(
            result.contains("command = \"llm -m gpt-4\""),
            "Should merge args into command"
        );
        assert!(
            !result.contains("commit-generation"),
            "Should remove old inline table"
        );
    }

    #[test]
    fn test_find_deprecations_empty_inline_table() {
        // Empty inline table should not be flagged
        let content = r#"
commit-generation = {}
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag empty inline table as deprecated"
        );
    }

    /// `args` with no `command` to merge into has nowhere to go —
    /// `[commit.generation]` has no `args` field — so it is dropped and
    /// reported rather than moved to a path that would warn forever.
    #[test]
    fn test_migrate_args_without_command_dropped() {
        let content = r#"
[commit-generation]
args = ["-m", "haiku"]
template = "some template"
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        template = "some template"
        "#);
        assert!(has_kind(
            &detect_deprecations(content),
            |k| matches!(k, DeprecationKind::UnsupportedKey { section, key }
                if section == "[commit-generation]" && key == "args")
        ));
    }

    /// A non-string `command` blocks the merge, so `args` is dropped for the
    /// same reason — and `command` itself survives for serde's type error.
    #[test]
    fn test_migrate_args_with_non_string_command() {
        let content = r#"
[commit-generation]
command = 123
args = ["-m", "haiku"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = 123
        "#);
    }

    #[test]
    fn test_migrate_empty_command_with_args() {
        let content = r#"
[commit-generation]
command = ""
args = ["-m", "haiku"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "-m haiku"
        "#);
    }

    /// An empty `args` contributes nothing to join, so `command` is left as
    /// written and the key merges away without a separate drop report.
    #[test]
    fn test_migrate_empty_args_leaves_command_unchanged() {
        let content = r#"
[commit-generation]
command = "llm"
args = []
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "llm"
        "#);
        // `command` is the section's only other key and it is supported, so
        // any unsupported-key report from this input would name `args`.
        assert!(
            !has_kind(&detect_deprecations(content), |k| matches!(
                k,
                DeprecationKind::UnsupportedKey { .. }
            )),
            "empty args merges away rather than being reported as unsupported"
        );
    }

    #[test]
    fn test_migrate_malformed_string_value_unchanged() {
        // When commit-generation is a string (malformed), migration must leave
        // it in place — silently dropping it would lose user config.
        let content = r#"
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("[commit.generation]"),
            "Should not create new section for malformed input"
        );
        assert!(
            result.contains("commit-generation = \"not a table\""),
            "Malformed value must be preserved; got: {result}"
        );
    }

    #[test]
    fn test_migrate_malformed_project_level_string_unchanged() {
        // When project-level commit-generation is a string, migration must
        // leave it in place rather than dropping it.
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("[projects.\"github.com/user/repo\".commit.generation]"),
            "Should not create new section for malformed project-level input"
        );
        assert!(
            result.contains("commit-generation = \"not a table\""),
            "Malformed project-level value must be preserved; got: {result}"
        );
    }

    /// Malformed deprecated section + a valid sibling migration: the bug was
    /// that doc.remove() happened before the malformed-value check, so a
    /// sibling migration would serialize the doc with the section already
    /// dropped. The fix peeks before removing.
    #[test]
    fn test_malformed_section_preserved_with_sibling_migration() {
        let content = r#"commit-generation = "keep me"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains(r#"commit-generation = "keep me""#),
            "Malformed commit-generation must survive sibling migrations; got:\n{result}"
        );
        // Sibling migration should still apply.
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated to merge.ff = false; got:\n{result}"
        );
    }

    /// Same shape for [select]: a malformed select value next to a valid
    /// sibling migration must be preserved.
    #[test]
    fn test_malformed_select_preserved_with_sibling_migration() {
        let content = r#"select = "not a table"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains(r#"select = "not a table""#),
            "Malformed select must survive sibling migrations; got:\n{result}"
        );
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    /// Scalar `commit = "x"` blocks `[commit.generation]` insertion. The
    /// migration must NOT remove `[commit-generation]` — doing so previously
    /// dropped the section since the new key could not be written under a
    /// scalar parent.
    #[test]
    fn test_commit_generation_preserved_when_commit_is_scalar() {
        let content = r#"commit = "x"

[commit-generation]
template = "tpl"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[commit-generation]") && result.contains(r#"template = "tpl""#),
            "[commit-generation] must survive when scalar `commit` blocks the new key; got:\n{result}"
        );
        // Source must still be the scalar — nothing inserted under it.
        assert!(
            result.contains(r#"commit = "x""#),
            "scalar `commit` must be preserved unchanged; got:\n{result}"
        );
        // Sibling migration still applies.
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    #[test]
    fn test_commit_generation_migrates_when_commit_parent_is_inline_table() {
        let content = r#"commit = { stage = "tracked" }

[commit-generation]
command = "llm"
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let commit = doc["commit"].as_table().expect("commit table");
        assert_eq!(
            commit["stage"].as_str(),
            Some("tracked"),
            "inline parent fields must survive: {result}"
        );
        assert_eq!(
            commit["generation"]["command"].as_str(),
            Some("llm"),
            "deprecated section should move under commit.generation: {result}"
        );
        assert!(
            doc.get("commit-generation").is_none(),
            "old section should be removed after migration: {result}"
        );
    }

    #[test]
    fn test_project_commit_generation_migrates_when_commit_parent_is_inline_table() {
        let content = r#"
[projects."github.com/user/repo"]
commit = { stage = "tracked" }
commit-generation = { command = "llm" }
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let project = doc["projects"]["github.com/user/repo"]
            .as_table()
            .expect("project table");
        let commit = project["commit"].as_table().expect("project commit table");
        assert_eq!(
            commit["stage"].as_str(),
            Some("tracked"),
            "inline project parent fields must survive: {result}"
        );
        assert_eq!(
            commit["generation"]["command"].as_str(),
            Some("llm"),
            "project deprecated section should move under commit.generation: {result}"
        );
        assert!(
            project.get("commit-generation").is_none(),
            "old project section should be removed after migration: {result}"
        );
    }

    /// Same shape for `[select]` when `switch = "x"` is scalar.
    #[test]
    fn test_select_preserved_when_switch_is_scalar() {
        let content = r#"switch = "x"

[select]
preview = "p"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[select]") && result.contains(r#"preview = "p""#),
            "[select] must survive when scalar `switch` blocks the new key; got:\n{result}"
        );
        assert!(
            result.contains(r#"switch = "x""#),
            "scalar `switch` must be preserved unchanged; got:\n{result}"
        );
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    /// `args = [1, "--ok"]`: a single non-string element blocks the merge —
    /// joining it would silently filter the non-string out and corrupt
    /// `command`. `command` is therefore left as written, and the unmergeable
    /// `args` is dropped and reported rather than moved into a
    /// `[commit.generation]` that has no field for it.
    #[test]
    fn test_commit_generation_args_dropped_when_non_string_element() {
        let content = r#"[commit-generation]
command = "echo"
args = [1, "--ok"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"
        [commit.generation]
        command = "echo"
        "#);
        assert!(has_kind(
            &detect_deprecations(content),
            |k| matches!(k, DeprecationKind::UnsupportedKey { section, key }
                if section == "[commit-generation]" && key == "args")
        ));
    }

    /// `[ci]` migration only owns `platform`; other keys in the same section
    /// must be preserved, not dropped along with the section. The new
    /// `[forge]` lands directly after the surviving `[ci]` remainder, not at
    /// the end of the file.
    #[test]
    fn test_ci_migration_preserves_other_keys() {
        let content = r#"[ci]
platform = "github"
hostname = "ghe.example"

[merge]
ff = false
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"
        [ci]
        hostname = "ghe.example"

        [forge]
        platform = "github"

        [merge]
        ff = false
        "#);
    }

    /// The migrated `[forge]` takes over `[ci]`'s file position — and its
    /// decor (the comment above) when the section is fully consumed — instead
    /// of rendering as a fresh position-less table at the end of the file.
    /// Comments on the `platform` line itself survive the move too.
    #[test]
    fn test_ci_migration_keeps_section_position() {
        let content = r#"# which forge to talk to
[ci]
platform = "github" # not gitlab

[merge]
ff = false
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"
        # which forge to talk to
        [forge]
        platform = "github" # not gitlab

        [merge]
        ff = false
        "#);
    }

    /// A `forge` that isn't a table suppresses the `[ci]` migration — a scalar
    /// `forge = "x"` must not be overwritten by the migrated table (serde's
    /// type error points at it instead).
    #[test]
    fn test_ci_migration_suppressed_by_scalar_forge() {
        let content = r#"forge = "x"

[ci]
platform = "github"

[list]
json-schema = 1
"#;
        assert_eq!(migrate_content(content), content);
        assert!(detect_deprecations(content).is_empty());
    }

    /// A `[forge]` table that only sets `hostname` has the `platform` slot
    /// free, so the deprecated key moves into it rather than being left behind.
    /// This is the shape a self-hosted GitLab or GHE user lands in by adding
    /// `[forge] hostname` to a config that already carried `[ci] platform`;
    /// suppressing on the table's mere existence left them un-migrated and
    /// unwarned. `[ci]` was consumed entirely, so it goes.
    #[test]
    fn test_ci_migration_fills_free_slot_in_existing_forge() {
        let content = r#"[ci]
platform = "gitlab"

[forge]
hostname = "gitlab.example.com"
"#;
        assert!(matches!(
            detect_deprecations(content).as_slice(),
            [DeprecationKind::CiSection]
        ));
        insta::assert_snapshot!(migrate_content(content), @r#"
        [forge]
        hostname = "gitlab.example.com"
        platform = "gitlab"
        "#);
    }

    /// An existing `[forge]` is wherever the user wrote it, so there is nowhere
    /// to carry a comment sitting above `[ci]`. An emptied but commented `[ci]`
    /// stays as an empty section — it contributes no config and raises no
    /// warning, and dropping it would drop the user's prose with it.
    #[test]
    fn test_ci_migration_keeps_commented_ci_when_forge_exists() {
        let content = r#"# talk to the internal instance
[ci]
platform = "gitlab"

[forge]
hostname = "gitlab.example.com"
"#;
        insta::assert_snapshot!(migrate_content(content), @r#"
        # talk to the internal instance
        [ci]

        [forge]
        hostname = "gitlab.example.com"
        platform = "gitlab"
        "#);
    }

    /// A surviving `[ci]` key keeps the section either way; only `platform`
    /// moves, and it lands in the existing `[forge]` rather than a new one.
    #[test]
    fn test_ci_migration_into_existing_forge_preserves_other_keys() {
        let content = r#"[ci]
platform = "gitea"
hostname = "ci.example"

[forge]
hostname = "forge.example"
"#;
        insta::assert_snapshot!(migrate_content(content), @r#"
        [ci]
        hostname = "ci.example"

        [forge]
        hostname = "forge.example"
        platform = "gitea"
        "#);
    }

    /// The framework invariant: a warning fires exactly when `wt config
    /// update` would change the file. Degenerate configs that can't be safely
    /// rewritten produce no warning and no rewrite; deprecated configs
    /// produce both, and what the update writes is schema-clean — a rewritten
    /// file must not warn about a key the user never typed. Silent renames
    /// change the file without warning by design and aren't part of this
    /// battery.
    #[test]
    fn test_warning_fires_iff_update_changes() {
        let untouched = [
            // [ci] without a usable platform: empty, non-string, or absent
            "[ci]\nplatform = \"\"\n",
            "[ci]\nplatform = 42\n",
            "[ci]\nhostname = \"ghe.example\"\n",
            // the destination key is occupied, or `forge` is not a table
            "[forge]\nplatform = \"gitlab\"\n\n[ci]\nplatform = \"github\"\n",
            "forge = \"x\"\n\n[ci]\nplatform = \"github\"\n",
            "forge = { hostname = \"ghe.example\" }\n\n[ci]\nplatform = \"github\"\n",
            // empty deprecated sections contribute no config
            "[commit-generation]\n",
            "[select]\n",
            // a scalar occupant blocks the destination
            "switch = \"x\"\n\n[select]\npreview = \"p\"\n",
            "commit = \"x\"\n\n[commit-generation]\ncommand = \"llm\"\n",
            // non-bool negated key is left for the unknown-field warning,
            // whether or not the new key is present (section or inline form)
            "[merge]\nno-ff = \"yes\"\n",
            "[merge]\nff = true\nno-ff = \"yes\"\n",
            "merge = { no-ff = \"yes\" }\n",
            // retired deprecations are left for generic unknown-field handling
            "switch = { picker = { timeout-ms = 500 } }\n",
            // a scalar destination blocks the whole rule, unsupported keys
            // included — nothing is dropped from a section that can't move
            "switch = \"x\"\n\n[select]\nheight = \"50%\"\n",
            // empty approved-commands is not deprecated
            "[projects.\"github.com/u/r\"]\napproved-commands = []\n",
            // a retired name a block binds is not the global, so the
            // rewriter leaves the whole template alone
            "worktree-path = \"{{ repo_root }}{% set repo_root = 'x' %}{{ repo_root }}\"\n",
            // `+` is whitespace control too, so the binding still counts
            "worktree-path = \"{{ repo_root }}{%+ set repo_root = 'x' %}{{ repo_root }}\"\n",
            // a parenthesized tuple target binds just as a bare one does
            "worktree-path = \"{{ repo_root }}{% for (repo_root, x) in items %}{{ repo_root }}{% endfor %}\"\n",
            // binding the *canonical* name holds the rename back from the
            // other side
            "worktree-path = \"{{ repo_root }}{% for repo_path in items %}{{ repo_root }}{% endfor %}\"\n",
            // a template MiniJinja can't parse has no reading to migrate
            // against, and the renderer won't take it either
            "worktree-path = \"{{ repo_root\"\n",
            // Live variables whose names merely contain a retired one. The
            // rewrite matches whole identifiers, and it now runs on every
            // load, so a substring match here would mangle a current variable
            // in every user's config on every command — `worktree` inside
            // `worktree_path`, `main_worktree` inside `main_worktree_path`
            // (itself retired, to a different name), `commits` inside
            // `recent_commits`
            "[commit.generation]\nsquash-template = \"{{ recent_commits | length }}\"\n",
            "worktree-path = \"{{ worktree_path }}\"\n",
            "post-start = \"ln -sf {{ primary_worktree_path }}/node_modules .\"\n",
        ];
        for content in untouched {
            assert!(
                detect_deprecations(content).is_empty(),
                "no warning expected for:\n{content}"
            );
            assert_eq!(
                compute_migrated_content(content),
                content,
                "no rewrite expected for:\n{content}"
            );
        }

        let rewritten = [
            "[ci]\nplatform = \"github\"\n",
            // a `[forge]` table whose `platform` slot is free is a destination,
            // not a blocker — with and without a comment holding `[ci]` open
            "[ci]\nplatform = \"github\"\n\n[forge]\nhostname = \"ghe.example\"\n",
            "# internal\n[ci]\nplatform = \"github\"\n\n[forge]\nhostname = \"ghe.example\"\n",
            "[merge]\nff = true\nno-ff = true\n",
            // negated bool written inline (`merge = { no-ff = true }`) migrates
            // like the section form
            "merge = { no-ff = true }\n",
            "switch = { no-cd = true }\n",
            "[select]\ntimeout-ms = 500\n",
            // a key the destination has no field for is dropped, not
            // relocated: on its own (the section goes away entirely), and
            // alongside a supported one
            "[select]\nheight = \"50%\"\n",
            "[select]\npager = \"delta\"\nheight = \"50%\"\n",
            "[commit-generation]\ncommand = \"llm\"\nmodel = \"haiku\"\n",
            "[projects.\"github.com/u/r\".select]\nheight = \"50%\"\n",
            // an `args` that can't be merged into `command` has nowhere to go
            "[commit-generation]\nargs = [\"-m\", \"haiku\"]\n",
            // list.task-timeout-ms, section and inline forms
            "[projects.\"github.com/u/r\".list]\ntask-timeout-ms = 500\n",
            "[projects.\"github.com/u/r\"]\nlist = { task-timeout-ms = 500 }\n",
            // the project scope itself written inline, and the whole
            // `projects` container written inline: the scope walk reaches
            // both, so the shape doesn't decide whether a rule fires
            "[projects]\n\"github.com/u/r\" = { merge = { no-ff = true } }\n",
            "[projects]\n\"github.com/u/r\" = { list = { task-timeout-ms = 500 } }\n",
            "[projects]\n\"github.com/u/r\" = { select = { timeout-ms = 500 } }\n",
            "projects = { \"github.com/u/r\" = { switch = { no-cd = true } } }\n",
            // Retired template variables, rewritten on load as well as by
            // update. `main_worktree_path` is the near-miss of the row above
            // it in the table and must reach its own replacement.
            "worktree-path = \"../{{ repo_root }}.{{ branch }}\"\n",
            "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"\n",
            "post-start = \"ln -sf {{ main_worktree_path }}/node_modules .\"\n",
            "post-start = \"cp {{ worktree }}/.env .\"\n",
            "[commit.generation]\nsquash-template = \"{{ commits | length }}\"\n",
            // a `}}` inside a quoted string doesn't end the tag
            "worktree-path = '{{ \"}} \" ~ repo_root }}'\n",
            // a binding-shaped tag inside `{% raw %}` binds nothing
            "worktree-path = \"{% raw %}{% set repo_root = 'x' %}{% endraw %}{{ repo_root }}\"\n",
            // the comma after a `set`'s `=` starts a tuple value, not a target
            "worktree-path = \"{% set a = 1, repo_root %}{{ repo_root }}\"\n",
            // a `set` block's filter chain is value, not target
            "worktree-path = \"{% set x | default(repo_root) %}b{% endset %}{{ repo_root }}\"\n",
            "[projects.\"github.com/u/r\"]\napproved-commands = [\"npm test\"]\n",
        ];
        for content in rewritten {
            assert!(
                !detect_deprecations(content).is_empty(),
                "warning expected for:\n{content}"
            );
            let migrated = compute_migrated_content(content);
            assert_ne!(&migrated, content, "rewrite expected for:\n{content}");
            // The user-visible loop closes: applying the update silences the
            // warning.
            assert!(
                detect_deprecations(&migrated).is_empty(),
                "no warning expected after update for:\n{migrated}"
            );
            // …and closes it for the *other* warning channel too: the update
            // must not write a key that the unknown-field check then flags
            // forever. A misplaced-but-known key (`forge` in user config) is
            // the user's own to move; a key with no home anywhere is the
            // migration's to drop.
            let unknown: Vec<_> =
                crate::config::collect_unknown_warnings::<crate::config::UserConfig>(&migrated)
                    .into_iter()
                    .filter(|w| {
                        matches!(
                            w,
                            crate::config::UnknownWarning::TopLevelUnknown { .. }
                                | crate::config::UnknownWarning::NestedUnknown { .. }
                        )
                    })
                    .collect();
            assert!(
                unknown.is_empty(),
                "update must not write unknown fields, got {unknown:?} for:\n{migrated}"
            );
        }
    }

    /// A retired `timeout-ms` key under `[select]` is removed instead of
    /// being carried into `[switch.picker]`, and reported after the section
    /// rename: the rename lines first, then one line per unsupported key.
    #[test]
    fn test_select_timeout_ms_removed_alongside_rename() {
        let content = "[select]\npager = \"delta\"\ntimeout-ms = 500\n";
        let deprecations = detect_deprecations(content);
        assert!(matches!(
            deprecations.as_slice(),
            [
                DeprecationKind::Select(scopes),
                DeprecationKind::UnsupportedKey { section, key }
            ] if scopes.has_top_level
                && scopes.project_keys.is_empty()
                && section == "[select]"
                && key == "timeout-ms"
        ));
        insta::assert_snapshot!(migrate_content(content), @r#"
        [switch.picker]
        pager = "delta"
        "#);
    }

    /// A `[select]` whose every key is unsupported writes no `[switch.picker]`
    /// at all, since an empty section would just be noise. Only the key is
    /// reported; a rename that didn't happen is not.
    #[test]
    fn test_select_with_only_unsupported_keys_leaves_no_section() {
        let content = "[select]\nheight = \"50%\"\n";
        let deprecations = detect_deprecations(content);
        assert!(matches!(
            deprecations.as_slice(),
            [DeprecationKind::UnsupportedKey { section, key }]
                if section == "[select]" && key == "height"
        ));
        assert_eq!(migrate_content(content), "");
    }

    /// Both lines name the scope they came from, so a project-scoped
    /// `[select]` never reports a top-level `[select]` the user doesn't have.
    #[test]
    fn test_select_warnings_name_the_project_scope() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: detect_deprecations(
                "[projects.\"github.com/u/r\".select]\npager = \"delta\"\nheight = \"50%\"\n",
            ),
            kind: ConfigFileKind::User,
            main_worktree_path: None,
        };
        assert_snapshot!(format_deprecation_warnings(&info).ansi_strip(), @r#"
        ▲ User config: [projects."github.com/u/r".select] is deprecated in favor of [projects."github.com/u/r".switch.picker]
        ▲ User config: [projects."github.com/u/r".select] height is no longer supported and will be removed
        "#);
    }

    /// The union of the destination's schemas across config files is what
    /// separates a key with no home from one that is merely misplaced. A
    /// user-only `command` written into project config is valid *somewhere*,
    /// so the migration carries it into `[commit.generation]` and lets the
    /// unknown-field check redirect it; a key valid nowhere goes.
    #[test]
    fn test_project_config_keeps_misplaced_user_only_keys() {
        let content = "[commit-generation]\ncommand = \"llm\"\nbogus = 1\n";
        assert!(has_kind(
            &detect_deprecations(content),
            |k| matches!(k, DeprecationKind::UnsupportedKey { section, key }
                if section == "[commit-generation]" && key == "bogus")
        ));
        let migrated = compute_migrated_content(content);
        insta::assert_snapshot!(migrated, @r#"
        [commit.generation]
        command = "llm"
        "#);
        // `command` survives to collect its redirect, not an "unknown field".
        assert!(matches!(
            crate::config::collect_unknown_warnings::<crate::config::ProjectConfig>(&migrated)
                .as_slice(),
            [crate::config::UnknownWarning::NestedWrongConfig { path, .. }]
                if path == "commit.generation.command"
        ));
    }

    #[test]
    fn test_migrate_invalid_toml_returns_unchanged() {
        // When content is not valid TOML, return it unchanged
        let content = "this is [not valid {toml";
        let result = migrate_content(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    // Snapshot tests for migration output (showing diffs)

    /// Generate a unified diff between original and migrated content
    fn migration_diff(original: &str, migrated: &str) -> String {
        use similar::{ChangeTag, TextDiff};
        let diff = TextDiff::from_lines(original, migrated);
        let mut output = String::new();
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            output.push_str(&format!("{}{}", sign, change));
        }
        output
    }

    #[test]
    fn snapshot_migrate_commit_generation_simple() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_commit_generation_with_args() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_with_trailing_sections() {
        // This is the bug case: [commit-generation] in the middle of the file
        // followed by other sections. The migration should not add an extra
        // [commit] section at the end.
        let content = r#"# Config file
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]

[list]
branches = true
remotes = false
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_preserves_existing_commit_section() {
        let content = r#"
[commit]
stage = "all"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm test"]

[projects."github.com/user/repo".commit-generation]
command = "llm"
args = ["-m", "gpt-4"]
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_combined_top_and_project() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"

[projects."github.com/user/repo".commit-generation]
command = "llm -m gpt-4"

[list]
branches = true
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    // Tests for approved-commands deprecation detection

    #[test]
    fn test_find_approved_commands_deprecation_none() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_present() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#;
        assert!(find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_empty_array() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = []
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_no_projects() {
        let content = r#"
worktree-path = "../{{ repo }}.{{ branch }}"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_project_without_approvals() {
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    // Tests for remove_approved_commands_from_config

    #[test]
    fn test_remove_approved_commands_multiple_projects() {
        let content = r#"
[projects."github.com/user/repo1"]
approved-commands = ["npm install"]

[projects."github.com/user/repo2"]
approved-commands = ["cargo test"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(result, @r#"

        [projects."github.com/user/repo2"]
        worktree-path = ".worktrees/{{ branch | sanitize }}"
        "#);
    }

    #[test]
    fn test_remove_approved_commands_no_change() {
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        assert_eq!(result, content);
    }

    /// An empty `approved-commands = []` is not deprecated and must survive
    /// the rewrite untouched — including when a sibling project's non-empty
    /// array triggers the rule. (Previously the whole-document migration ran
    /// once any project fired, removing empty arrays along the way.)
    #[test]
    fn test_remove_approved_commands_keeps_empty_sibling() {
        let content = r#"
[projects."github.com/user/repo1"]
approved-commands = ["npm install"]

[projects."github.com/user/repo2"]
approved-commands = []
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(result, @r#"

        [projects."github.com/user/repo2"]
        approved-commands = []
        "#);
    }

    #[test]
    fn snapshot_remove_approved_commands() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_remove_approved_commands_entire_section() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn test_detect_deprecations_includes_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::ApprovedCommands
        )));
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn test_remove_approved_commands_invalid_toml() {
        let content = "this is { not valid toml";
        let result = remove_approved_commands_from_config(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    /// The three outcomes the block renderer has to keep apart: identical
    /// content, changed content, and (covered by the integration tests that
    /// break `git diff`) a failure. Before #4118 a failure rendered as the
    /// first of these.
    #[test]
    fn test_migration_diff_block_separates_identical_from_changed() {
        // This test spawns the real `git diff`, and no fixture constructor runs
        // here to latch the floor for it — without this the child reads the
        // developer's own global config, where a single unparsable `diff.*`
        // value turns the first assertion into the failure arm.
        crate::shell_exec::enable_hermetic_test_env();

        let original = "worktree-path = \"../{{ repo }}.{{ branch }}\"\n";
        assert_eq!(
            format_migration_diff_block(original, original, "config.toml"),
            "",
            "identical content renders nothing"
        );

        let block = format_migration_diff_block(
            original,
            "worktree-path = \"../{{ repo }}.{{ branch | sanitize }}\"\n",
            "config.toml",
        );
        assert!(
            block.contains("Proposed diff:") && block.contains("sanitize"),
            "changed content renders the patch, got:\n{block}"
        );
    }

    #[test]
    fn test_format_deprecation_details_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: vec![DeprecationKind::ApprovedCommands],
            kind: ConfigFileKind::User,
            main_worktree_path: None,
        };
        let output = format_deprecation_details(&info, content);
        assert!(
            output.contains("approved-commands"),
            "Should mention approved-commands in output: {}",
            output
        );
        assert!(
            output.contains("approvals.toml"),
            "Should mention approvals.toml: {}",
            output
        );
    }

    #[test]
    fn test_compute_migrated_content_removes_approved_commands() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let migrated = compute_migrated_content(content);
        assert!(!migrated.contains("approved-commands"));
    }

    #[test]
    fn test_copy_approved_commands_creates_approvals_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]

[projects."github.com/other/repo"]
approved-commands = ["cargo build"]
"#;
        std::fs::write(&config_path, content).unwrap();

        let result =
            copy_approved_commands_to_approvals_file(&config_path).expect("copy should succeed");
        assert!(result.is_some(), "Should create approvals.toml");

        let approvals_path = result.unwrap();
        assert_eq!(approvals_path, temp_dir.path().join("approvals.toml"));

        let approvals_content = std::fs::read_to_string(&approvals_path).unwrap();
        assert!(
            approvals_content.contains("npm install"),
            "Should contain npm install: {}",
            approvals_content
        );
        assert!(
            approvals_content.contains("npm test"),
            "Should contain npm test: {}",
            approvals_content
        );
        assert!(
            approvals_content.contains("cargo build"),
            "Should contain cargo build: {}",
            approvals_content
        );
    }

    #[test]
    fn test_copy_approved_commands_skips_when_approvals_exists() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let approvals_path = temp_dir.path().join("approvals.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();
        std::fs::write(&approvals_path, "# existing approvals\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path)
            .expect("skip should not surface error");
        assert!(result.is_none(), "Should skip when approvals.toml exists");

        // Verify existing file was not overwritten
        let existing = std::fs::read_to_string(&approvals_path).unwrap();
        assert_eq!(existing, "# existing approvals\n");
    }

    #[test]
    fn test_copy_approved_commands_errors_when_existing_approvals_invalid() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let approvals_path = temp_dir.path().join("approvals.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();
        std::fs::write(&approvals_path, "this is = = not valid toml\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(
            result.is_err(),
            "Invalid existing approvals.toml must surface as Err; got {result:?}"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse existing approvals file"),
            "Error should identify the invalid approvals file"
        );
    }

    #[test]
    fn test_copy_approved_commands_skips_when_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        std::fs::write(&config_path, content).unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path)
            .expect("empty case should not surface error");
        assert!(
            result.is_none(),
            "Should skip when no approved-commands exist"
        );
    }

    /// Regression: when approvals.toml cannot be written (e.g. the directory
    /// is read-only), the copy must return Err rather than silently signaling
    /// "nothing to copy", otherwise the caller would proceed to rewrite
    /// config.toml and drop the legacy approvals.
    #[cfg(unix)]
    #[test]
    fn test_copy_approved_commands_surfaces_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();

        // Make the directory read-only so approvals.toml creation fails.
        let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(temp_dir.path(), perms).unwrap();

        // Root ignores directory permissions, so the write would succeed and
        // the assertion below would spuriously fail (Claude Code web, Docker).
        // Probe and skip when not actually restricted — matching the pattern
        // in tests/integration_tests/approval_save.rs.
        if std::fs::write(temp_dir.path().join("__probe"), "").is_ok() {
            let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(temp_dir.path(), perms).unwrap();
            std::eprintln!("Skipping permission test - running with elevated privileges");
            return;
        }

        let result = copy_approved_commands_to_approvals_file(&config_path);

        // Restore writable perms so the tempdir can be cleaned up.
        let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(temp_dir.path(), perms).unwrap();

        assert!(
            result.is_err(),
            "Write failure must surface as Err, not Ok(None); got {result:?}"
        );
    }

    /// Regression: when the source config cannot be read or parsed, the copy
    /// must surface the error (with context) rather than silently signaling
    /// "nothing to copy" — same data-loss class as the write-failure case.
    #[test]
    fn test_copy_approved_commands_surfaces_read_failure() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, "this is = = not valid toml\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(
            result.is_err(),
            "Unparsable source config must surface as Err; got {result:?}"
        );
    }

    #[test]
    fn test_set_implicit_suppresses_parent_header() {
        // Verifies that set_implicit(true) prevents an empty parent table from
        // rendering its own header. This is the key technique used in
        // ensure_standard_table_parent to avoid creating spurious [commit]
        // headers when migrating [commit-generation] to [commit.generation].
        use toml_edit::{DocumentMut, Item, Table};

        let mut doc: DocumentMut = "[foo]\nbar = 1\n".parse().unwrap();
        let mut commit_table = Table::new();
        commit_table.set_implicit(true);
        let mut gen_table = Table::new();
        gen_table.insert("command", toml_edit::value("llm"));
        commit_table.insert("generation", Item::Table(gen_table));
        doc.insert("commit", Item::Table(commit_table));
        let result = doc.to_string();

        assert!(
            !result.contains("\n[commit]\n"),
            "set_implicit should suppress separate [commit] header"
        );
        assert!(
            result.contains("[commit.generation]"),
            "Should have [commit.generation] header"
        );
    }

    // Tests for [select] → [switch.picker] deprecation

    #[test]
    fn test_find_select_deprecation_none() {
        let content = r#"
[switch.picker]
pager = "delta --paging=never"
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_present() {
        let content = r#"
[select]
pager = "delta --paging=never"
"#;
        assert!(find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_empty_not_flagged() {
        let content = r#"
[select]
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_skips_when_new_exists() {
        // When both [select] and [switch.picker] exist, don't flag
        let content = r#"
[select]
pager = "old"

[switch.picker]
pager = "new"
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_inline_table() {
        let content = r#"
select = { pager = "delta" }
"#;
        assert!(find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_empty_inline_table() {
        let content = r#"
select = {}
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_migrate_select_simple() {
        let content = r#"
[select]
pager = "delta --paging=never"
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[switch.picker]"),
            "Should have [switch.picker]: {result}"
        );
        assert!(
            result.contains("pager = \"delta --paging=never\""),
            "Should preserve pager: {result}"
        );
        assert!(
            !result.contains("[select]"),
            "Should remove [select]: {result}"
        );
    }

    #[test]
    fn test_migrate_commented_inline_parent_keeps_the_config_loadable() {
        // The parent has to become a standard table before `[commit.generation]`
        // can be added, and the key's decor — the comment above it — renders
        // inside the header brackets. Left there it wrote
        // `[# my commit settings\ncommit ]`, and because this rule runs before
        // serde on every load, the user's config stopped parsing entirely.
        let content = r#"# my commit settings
commit = { stage = "tracked" }
commit-generation = { command = "llm" }
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("# my commit settings\n[commit]"),
            "the comment belongs above the header, not inside it: {result}"
        );

        let config = crate::config::UserConfig::load_from_str(content)
            .unwrap_or_else(|e| panic!("config must still load: {e}\n{result}"));
        assert_eq!(config.commit.stage, Some(crate::config::StageMode::Tracked));
        assert_eq!(
            config
                .commit
                .generation
                .and_then(|generation| generation.command)
                .as_deref(),
            Some("llm"),
        );
    }

    #[test]
    fn test_migrate_inline_parent_after_blank_line_keeps_the_config_loadable() {
        // Same decor path with no comment: a blank line before the inline
        // section is prefix decor too, which makes any inline section past the
        // first line of the file a candidate.
        let content = r#"skip-shell-integration-prompt = true

switch = { cd = false }

[select]
pager = "delta"
"#;
        let config = crate::config::UserConfig::load_from_str(content)
            .unwrap_or_else(|e| panic!("config must still load: {e}"));
        assert_eq!(config.switch.cd, Some(false));
        assert_eq!(
            config
                .switch
                .picker
                .and_then(|picker| picker.pager)
                .as_deref(),
            Some("delta"),
        );
    }

    #[test]
    fn test_migrate_select_when_switch_parent_is_inline_table() {
        let content = r#"switch = { cd = false }

[select]
pager = "delta"
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let switch = doc["switch"].as_table().expect("switch table");
        assert_eq!(
            switch["cd"].as_bool(),
            Some(false),
            "inline switch fields must survive: {result}"
        );
        assert_eq!(
            switch["picker"]["pager"].as_str(),
            Some("delta"),
            "select should move under switch.picker: {result}"
        );
        assert!(
            doc.get("select").is_none(),
            "old select section should be removed after migration: {result}"
        );
    }

    #[test]
    fn test_migrate_select_skips_when_new_exists() {
        let content = r#"
[select]
pager = "old"

[switch.picker]
pager = "new"
"#;
        let result = migrate_content(content);
        assert_eq!(
            result, content,
            "Should not migrate when new section exists"
        );
    }

    #[test]
    fn test_migrate_select_invalid_toml() {
        let content = "this is { not valid toml";
        let result = migrate_content(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    #[test]
    fn test_migrate_select_no_select_section() {
        let content = r#"
[list]
full = true
"#;
        let result = migrate_content(content);
        assert_eq!(result, content, "No [select] section means no migration");
    }

    #[test]
    fn test_detect_deprecations_includes_select() {
        let content = r#"
[select]
pager = "delta"
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::Select(_)
        )));
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn snapshot_migrate_select_to_switch_picker() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[select]
pager = "delta --paging=never"

[list]
branches = true
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn test_format_deprecation_details_select() {
        let content = r#"[select]
pager = "delta --paging=never"
"#;
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: vec![DeprecationKind::Select(ScopedSections {
                has_top_level: true,
                project_keys: Vec::new(),
            })],
            kind: ConfigFileKind::User,
            main_worktree_path: None,
        };
        let output = format_deprecation_details(&info, content);
        assert!(
            output.contains("[select]"),
            "Should mention [select] in output: {output}"
        );
        assert!(
            output.contains("[switch.picker]"),
            "Should mention [switch.picker]: {output}"
        );
    }

    #[test]
    fn test_compute_migrated_content_renames_select() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[select]
pager = "delta --paging=never"
"#;
        let migrated = compute_migrated_content(content);
        assert!(
            migrated.contains("[switch.picker]"),
            "Migrated content should have [switch.picker]: {migrated}"
        );
        assert!(
            !migrated.contains("[select]"),
            "Migrated content should not have [select]: {migrated}"
        );
    }

    /// `[commit-generation]` migrates into `commit`, which TOML forbids
    /// extending when the file wrote it inline — so it becomes a standard
    /// table, and the line's trailing comment lands after the new header's `]`.
    #[test]
    fn test_migrate_carries_an_inline_parent_comment_onto_its_header() {
        let content = r#"commit = { stage = "all" }  # how to stage

[commit-generation]
template = "MINE"
"#;
        insta::assert_snapshot!(migrate_content(content), @r#"
        [commit]  # how to stage
        stage = "all"

        [commit.generation]
        template = "MINE"
        "#);
    }

    /// The silent create-hooks rule renames the deprecated `pre-create`/`post-create`
    /// keys to canonical `pre-start`/`post-start`, preserving the value shape
    /// (string, `[table]`, `[[array-of-tables]]`) and the comment above the key,
    /// at the top level and inside `[projects."..."]` entries written either as
    /// tables or inline.
    #[test]
    fn test_migrate_create_hooks_renames_every_shape() {
        let content = r#"# install first
pre-create = "npm install"

[[post-create]]
lint = "cargo clippy"

[projects]
"inline-project" = { post-create = "make" }

[projects."my-project"]
pre-create = "cargo build"

[projects."my-project".post-create]
server = "npm run dev"
"#;
        insta::assert_snapshot!(migrate_content(content), @r#"
        # install first
        pre-start = "npm install"

        [[post-start]]
        lint = "cargo clippy"

        [projects]
        "inline-project" = { post-start = "make" }

        [projects."my-project"]
        pre-start = "cargo build"

        [projects."my-project".post-start]
        server = "npm run dev"
        "#);
    }

    /// When the canonical `-start` key already exists, the migrator leaves the
    /// deprecated `-create` key alone rather than clobbering the user's value.
    #[test]
    fn test_migrate_create_hooks_skips_when_start_exists() {
        let content = r#"pre-create = "old"
pre-start = "new"

[projects."my-project"]
post-create = "old"
post-start = "new"
"#;
        assert_eq!(
            migrate_content(content),
            content,
            "must not clobber an existing canonical key"
        );
    }

    #[test]
    fn test_migrate_create_hooks_invalid_toml() {
        let content = "this is { not valid toml";
        assert_eq!(migrate_content(content), content);
    }

    #[test]
    fn snapshot_migrate_create_to_start() {
        let content = r#"pre-create = "npm install"

[post-create]
server = "npm run dev"

[list]
json-schema = 1
"#;
        let migrated = compute_migrated_content(content);
        insta::assert_snapshot!(migration_diff(content, &migrated));
    }

    #[test]
    fn test_switch_picker_timeout_is_left_for_unknown_field_handling() {
        let content = r#"
[switch.picker]
pager = "delta"
timeout-ms = 500

[list]
json-schema = 1
"#;
        assert!(detect_deprecations(content).is_empty());
        assert_eq!(compute_migrated_content(content), content);
    }

    #[test]
    fn test_detect_list_task_timeout_top_level() {
        let content = r#"
[list]
branches = true
task-timeout-ms = 500
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::ListTaskTimeout
        )));
    }

    #[test]
    fn test_detect_list_task_timeout_project_level() {
        let content = r#"
[projects."github.com/user/repo".list]
task-timeout-ms = 300
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::ListTaskTimeout
        )));
    }

    #[test]
    fn test_detect_list_task_timeout_absent() {
        let content = r#"
[list]
timeout-ms = 500
"#;
        let deprecations = detect_deprecations(content);
        assert!(!has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::ListTaskTimeout
        )));
    }

    #[test]
    fn test_migrate_list_task_timeout_removes_key() {
        let content = r#"
[list]
branches = true
task-timeout-ms = 500
timeout-ms = 2000
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("task-timeout-ms"),
            "Should strip task-timeout-ms: {result}"
        );
        assert!(
            result.contains("timeout-ms = 2000") && result.contains("branches"),
            "Should preserve sibling keys: {result}"
        );
    }

    #[test]
    fn test_migrate_list_task_timeout_inline_table() {
        let content = r#"
list = { branches = true, task-timeout-ms = 500 }
"#;
        let result = migrate_content(content);
        assert!(!result.contains("task-timeout-ms"));
        assert!(result.contains("branches"));
    }

    #[test]
    fn test_migrate_list_task_timeout_noop_when_absent() {
        let content = r#"
[list]
timeout-ms = 500
"#;
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    // ==================== negated bool format + migration tests ====================

    #[test]
    fn test_format_deprecation_warnings_all_kinds() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            // Keep one representative of each warning kind in
            // DEPRECATION_RULES emission order. The two section renames
            // include both scopes because their formatters have distinct
            // branches.
            deprecations: vec![
                DeprecationKind::TemplateVar {
                    old: "repo_root",
                    new: "repo_path",
                },
                DeprecationKind::CommitGeneration(ScopedSections {
                    has_top_level: true,
                    project_keys: vec!["github.com/user/repo".to_string()],
                }),
                DeprecationKind::ApprovedCommands,
                DeprecationKind::Select(ScopedSections {
                    has_top_level: true,
                    project_keys: vec!["github.com/user/repo".to_string()],
                }),
                DeprecationKind::UnsupportedKey {
                    section: "[select]".to_string(),
                    key: "height".to_string(),
                },
                DeprecationKind::CiSection,
                DeprecationKind::NoFf,
                DeprecationKind::NoCd,
                DeprecationKind::ListTaskTimeout,
            ],
            kind: ConfigFileKind::User,
            main_worktree_path: None,
        };
        assert_snapshot!(format_deprecation_warnings(&info).ansi_strip(), @r#"
        ▲ User config: template variable repo_root is deprecated in favor of repo_path
        ▲ User config: [commit-generation] is deprecated in favor of [commit.generation]
        ▲ User config: [projects."github.com/user/repo".commit-generation] is deprecated in favor of [projects."github.com/user/repo".commit.generation]
        ▲ User config: approved-commands under [projects] is deprecated in favor of approvals.toml
        ▲ User config: [select] is deprecated in favor of [switch.picker]
        ▲ User config: [projects."github.com/user/repo".select] is deprecated in favor of [projects."github.com/user/repo".switch.picker]
        ▲ User config: [select] height is no longer supported and will be removed
        ▲ User config: [ci] is deprecated in favor of [forge]
        ▲ User config: merge.no-ff is deprecated in favor of merge.ff (inverted)
        ▲ User config: switch.no-cd is deprecated in favor of switch.cd (inverted)
        ▲ User config: list.task-timeout-ms is no longer used — list.timeout-ms bounds the collect phase
        "#);
    }

    /// The same kinds as above, in the tense a config mutation reports its
    /// write in — the second half of what "Adding a deprecation" words.
    #[test]
    fn test_format_applied_lines_all_kinds() {
        let kinds = vec![
            DeprecationKind::TemplateVar {
                old: "repo_root",
                new: "repo_path",
            },
            DeprecationKind::CommitGeneration(ScopedSections {
                has_top_level: true,
                project_keys: vec!["github.com/user/repo".to_string()],
            }),
            DeprecationKind::ApprovedCommands,
            DeprecationKind::Select(ScopedSections {
                has_top_level: true,
                project_keys: vec!["github.com/user/repo".to_string()],
            }),
            DeprecationKind::UnsupportedKey {
                section: "[select]".to_string(),
                key: "height".to_string(),
            },
            DeprecationKind::CiSection,
            DeprecationKind::NoFf,
            DeprecationKind::NoCd,
            DeprecationKind::ListTaskTimeout,
        ];
        assert_snapshot!(format_applied_lines(&kinds).ansi_strip(), @r#"
        ▲ Renamed template variable repo_root to repo_path
        ▲ Moved [commit-generation] to [commit.generation]
        ▲ Moved [projects."github.com/user/repo".commit-generation] to [projects."github.com/user/repo".commit.generation]
        ▲ Moved approved-commands under [projects] to approvals.toml
        ▲ Moved [select] to [switch.picker]
        ▲ Moved [projects."github.com/user/repo".select] to [projects."github.com/user/repo".switch.picker]
        ▲ Removed [select] height, which its replacement has no field for
        ▲ Moved [ci] to [forge]
        ▲ Replaced merge.no-ff with merge.ff (inverted)
        ▲ Replaced switch.no-cd with switch.cd (inverted)
        ▲ Removed list.task-timeout-ms, which nothing reads
        "#);
    }

    #[test]
    fn test_detect_no_ff_deprecation() {
        let deprecations = detect_deprecations("[merge]\nno-ff = true\n");
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
    }

    /// With both keys present, `ff` takes precedence: the deprecated key is
    /// removed without inverting it into a value, and — since the file changes
    /// on `wt config update` — a warning fires rather than dropping the key
    /// silently.
    #[test]
    fn test_no_ff_warned_and_removed_when_ff_exists() {
        let content = "[merge]\nff = true\nno-ff = true\n";
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
        insta::assert_snapshot!(migrate_content(content), @r#"
        [merge]
        ff = true
        "#);
    }

    #[test]
    fn test_detect_no_cd_deprecation() {
        let deprecations = detect_deprecations("[switch]\nno-cd = true\n");
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoCd
        )));
    }

    #[test]
    fn test_detect_no_ff_project_level() {
        let content = r#"
[projects."github.com/user/repo".merge]
no-ff = true
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
    }

    #[test]
    fn test_migrate_no_ff_to_ff() {
        let content = "[merge]\nno-ff = true\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "Should invert: {result}");
        assert!(!result.contains("no-ff"), "Should remove no-ff: {result}");
    }

    #[test]
    fn test_migrate_no_cd_to_cd() {
        let content = "[switch]\nno-cd = false\n";
        let result = migrate_content(content);
        assert!(result.contains("cd = true"), "Should invert: {result}");
        assert!(!result.contains("no-cd"), "Should remove no-cd: {result}");
    }

    #[test]
    fn test_migrate_negated_bool_inline_table() {
        // A section written inline (`merge = { no-ff = true }`) migrates like the
        // standard `[merge]` form, preserving the inline shape.
        insta::assert_snapshot!(migrate_content("merge = { no-ff = true }\n"), @"merge = { ff = false }");
        insta::assert_snapshot!(migrate_content("switch = { no-cd = true }\n"), @"switch = { cd = false }");
    }

    #[test]
    fn test_migrate_no_ff_project_level() {
        let content = r#"
[projects."github.com/user/repo".merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "Should migrate: {result}");
        assert!(!result.contains("no-ff"), "Should remove no-ff: {result}");
    }

    #[test]
    fn test_migrate_no_ff_inline_project_scope() {
        // A project entry written inline is the same config as the section
        // form, so it migrates the same way. `UserProjectOverrides` has no
        // `no-ff` alias, so an unmigrated key is dropped from the typed value
        // on load — the setting would silently stop applying.
        let content = "[projects]\n\"github.com/user/repo\" = { merge = { no-ff = true } }\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "should migrate: {result}");
        assert!(!result.contains("no-ff"), "should remove no-ff: {result}");

        let config = crate::config::UserConfig::load_from_str(content).unwrap();
        assert_eq!(
            config.projects["github.com/user/repo"].merge.ff,
            Some(false),
            "the migrated setting should reach the typed config"
        );
    }

    #[test]
    fn test_migrate_no_ff_fully_inline_projects_table() {
        // ...including when the `projects` container itself is inline.
        let content = "projects = { \"github.com/user/repo\" = { merge = { no-ff = true } } }\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "should migrate: {result}");
        assert!(!result.contains("no-ff"), "should remove no-ff: {result}");
    }

    #[test]
    fn test_migrate_leaves_unrelated_inline_project_keys_alone() {
        // The table round-trip an inline scope goes through must not disturb
        // keys no rule touched, and an entry no rule changed keeps its shape.
        let content = "[projects]\n\"a/b\" = { worktree-path = \"../{{ branch }}\" }\n\"c/d\" = { merge = { no-ff = true, squash = true } }\n";
        let result = migrate_content(content);
        assert!(
            result.contains("\"a/b\" = { worktree-path = \"../{{ branch }}\" }"),
            "untouched entry should keep its formatting: {result}"
        );
        assert!(result.contains("squash = true"), "should keep: {result}");
        assert!(result.contains("ff = false"), "should migrate: {result}");
    }

    #[test]
    fn test_migrate_leaves_project_entries_that_are_not_tables_alone() {
        // A project entry can be hand-written as something other than a table.
        // The scope walk has no scope to offer a rule there, so it skips the
        // entry and leaves the text for serde's own type error and the
        // unknown-field check — it must not panic or rewrite.
        for (content, untouched) in [
            // a scalar entry
            (
                "[projects]\n\"a/b\" = \"scalar\"\n[merge]\nno-ff = true\n",
                "\"a/b\" = \"scalar\"\n",
            ),
            // an array-of-tables entry
            (
                "[[projects.\"a/b\"]]\nno-ff = true\n[merge]\nno-ff = true\n",
                "[[projects.\"a/b\"]]\nno-ff = true\n",
            ),
        ] {
            let result = migrate_content(content);
            assert!(
                result.contains(untouched),
                "the entry should survive unrewritten: {result}"
            );
            // The top-level rule still fires, so the walk itself ran.
            assert!(
                result.contains("ff = false"),
                "top-level scope should still migrate: {result}"
            );
        }
    }

    #[test]
    fn test_migrate_negated_bool_non_boolean_value_preserved() {
        // Non-boolean `no-ff` value should be left alone
        let content = "[merge]\nno-ff = \"not-a-bool\"\n";
        let result = migrate_content(content);
        assert!(
            result.contains("no-ff"),
            "Non-boolean value should be preserved: {result}"
        );
    }

    #[test]
    fn test_migrate_no_ff_skips_when_ff_exists() {
        let content = "[merge]\nff = true\nno-ff = true\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = true"), "ff should be kept: {result}");
        assert!(
            !result.contains("no-ff"),
            "no-ff should be removed: {result}"
        );
    }

    // ==================== project-level select migration tests ====================

    #[test]
    fn test_detect_select_project_level() {
        let content = r#"
[projects."github.com/user/repo".select]
pager = "bat"
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::Select(_)
        )));
    }

    #[test]
    fn test_migrate_select_project_level() {
        let content = r#"
[projects."github.com/user/repo".select]
pager = "bat"
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[projects.\"github.com/user/repo\".switch.picker]"),
            "Should migrate project select: {result}"
        );
        assert!(
            !result.contains("[projects.\"github.com/user/repo\".select]"),
            "Should remove project select: {result}"
        );
    }

    // ==================== migrate_content tests ====================

    #[test]
    fn test_migrate_content_applies_all_structural_migrations() {
        let content = r#"
[commit-generation]
command = "llm"

[select]
pager = "delta"

[merge]
no-ff = true

[switch]
no-cd = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[commit.generation]"),
            "commit-generation: {result}"
        );
        assert!(
            result.contains("[switch.picker]"),
            "select to switch.picker: {result}"
        );
        assert!(result.contains("ff = false"), "no-ff to ff: {result}");
        assert!(result.contains("cd = false"), "no-cd to cd: {result}");
    }

    #[test]
    fn test_migrate_content_is_no_op_for_canonical_config() {
        let content = r#"
[commit.generation]
command = "llm"

[merge]
ff = true
"#;
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_warn_unknown_fields_deprecated_key_in_wrong_config() {
        use crate::config::{ProjectConfig, UnknownWarning, UserConfig, collect_unknown_warnings};

        // User-only commit-generation key in project config → nested redirect
        // to user config (the load path that `config show` mirrors).
        let warnings =
            collect_unknown_warnings::<ProjectConfig>("[commit-generation]\ncommand = \"llm\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::NestedWrongConfig { path, other_description }]
                    if path == "commit.generation.command" && *other_description == "user config"
            ),
            "expected one NestedWrongConfig → user config, got {warnings:?}"
        );

        // ci in user config → top-level deprecated-section redirect.
        let warnings = collect_unknown_warnings::<UserConfig>("[ci]\nplatform = \"github\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::TopLevelDeprecatedWrongConfig { other_description, .. }]
                    if *other_description == "project config"
            ),
            "expected one TopLevelDeprecatedWrongConfig → project config, got {warnings:?}"
        );

        // Exercise the stderr/dedup side-effect path itself.
        let path = std::env::temp_dir().join("test-deprecated-wrong-config-project.toml");
        warn_unknown_fields::<ProjectConfig>(
            "[commit-generation]\ncommand = \"llm\"\n",
            &path,
            ConfigFileKind::Project,
        );
    }

    /// A deprecated section the migration declined to rewrite still carries
    /// live user intent that nothing reads, so the unknown-field channel has
    /// to speak for it — the deprecation channel stays silent by design when
    /// the rewrite is unsafe.
    #[test]
    fn test_unmigrated_deprecated_section_warns_as_unknown() {
        use crate::config::{UnknownWarning, UserConfig, collect_unknown_warnings};

        // Malformed: `select` is a scalar, so `migrate_select_table` leaves it
        // alone rather than dropping the user's value.
        let warnings = collect_unknown_warnings::<UserConfig>("select = \"not a table\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::TopLevelUnknown { key }] if key == "select"
            ),
            "expected select → unknown field, got {warnings:?}"
        );

        // Destination already occupied: the rewrite is skipped so it cannot
        // clobber the live `[switch.picker]`, leaving `[select]` read by
        // nobody.
        let warnings = collect_unknown_warnings::<UserConfig>(
            "[switch.picker]\npager = \"delta\"\n\n[select]\npreview = \"p\"\n",
        );
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::TopLevelUnknown { key }] if key == "select"
            ),
            "expected occupied-destination select → unknown field, got {warnings:?}"
        );

        // A scalar occupant (`switch = "x"`) is a *type* error for a known
        // field, so the round-trip analysis is unreliable and serde's own
        // message is the diagnostic — this channel stays out of it.
        assert!(
            collect_unknown_warnings::<UserConfig>("switch = \"x\"\n\n[select]\npreview = \"p\"\n")
                .is_empty(),
            "a type error belongs to serde, not the unknown-field channel"
        );

        // An empty deprecated section contributes no config; it stays silent.
        assert!(
            collect_unknown_warnings::<UserConfig>("[select]\n").is_empty(),
            "empty [select] must stay silent"
        );

        // A section the migration did rewrite is handled — no second warning
        // from this channel.
        assert!(
            collect_unknown_warnings::<UserConfig>("[select]\npager = \"delta\"\n").is_empty(),
            "migrated [select] must not warn"
        );

        // The rule is the registry's, not `select`'s: a malformed
        // `commit-generation` is skipped by its own migration and needs the
        // same fallback. (`ci` is exempt — it is still a live
        // `ProjectConfig` field, so its leftovers already warn per key.)
        let warnings = collect_unknown_warnings::<UserConfig>("commit-generation = \"keep me\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::TopLevelUnknown { key }] if key == "commit-generation"
            ),
            "expected malformed commit-generation → unknown field, got {warnings:?}"
        );
    }

    #[test]
    fn test_nested_user_only_key_redirects_generally() {
        use crate::config::{ProjectConfig, UnknownWarning, UserConfig, collect_unknown_warnings};

        // `[list]` is a valid *shared* section (project config accepts `url`),
        // but `columns` / `full` are user-config display settings. Placing them
        // in project config redirects to user config rather than reading
        // "unknown field" — the general "valid in the other config" check, not
        // the hard-coded commit.generation list (#3469).
        let warnings = collect_unknown_warnings::<ProjectConfig>(
            "[list]\ncolumns = [\"branch\"]\nfull = true\n",
        );
        assert!(
            warnings.iter().all(|w| matches!(
                w,
                UnknownWarning::NestedWrongConfig { path, other_description }
                    if (path == "list.columns" || path == "list.full")
                        && *other_description == "user config"
            )) && warnings.len() == 2,
            "expected list.columns/list.full → user config, got {warnings:?}"
        );

        // A key unknown in *both* configs stays "unknown field".
        let warnings = collect_unknown_warnings::<ProjectConfig>("[list]\nnonsense-typo = true\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::NestedUnknown { path }] if path == "list.nonsense-typo"
            ),
            "expected list.nonsense-typo → unknown, got {warnings:?}"
        );

        // The reverse direction: `url` is project-only, so it redirects to
        // project config when found in user config — no scope-to-repo note
        // there (that only applies to user-config destinations).
        let warnings = collect_unknown_warnings::<UserConfig>("[list]\nurl = \"x\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::NestedWrongConfig { path, other_description }]
                    if path == "list.url" && *other_description == "project config"
            ),
            "expected list.url → project config, got {warnings:?}"
        );
    }

    #[test]
    fn test_scope_to_repo_note_destinations() {
        // Toward user config, the note fires for any misplaced key whose
        // top-level segment is a `[projects."<id>"]` field.
        assert!(scope_to_repo_note("user config", "list.columns").is_some());
        assert!(scope_to_repo_note("user config", "worktree-path").is_some());

        // Toward project config, only a whole top-level table (`forge`) earns
        // it: the projects-entry `list` is the user type, which has no `url`,
        // so that advice would be wrong.
        assert!(scope_to_repo_note("project config", "forge").is_some());
        assert!(scope_to_repo_note("project config", "list.url").is_none());
        assert!(scope_to_repo_note("project config", "forge.platform").is_none());

        // Root-only user scalars have no `[projects."<id>"]` form, so following
        // the note would just yield a fresh "unknown field" — no note.
        assert!(scope_to_repo_note("user config", "skip-shell-integration-prompt").is_none());
        assert!(scope_to_repo_note("user config", "skip-commit-generation-prompt").is_none());

        // It reaches the rendered load-warning for a user-config redirect...
        let note = "to scope it to this repo";
        let msg = format_load_warning(
            "Project config",
            &crate::config::UnknownWarning::NestedWrongConfig {
                path: "list.columns".to_string(),
                other_description: "user config",
            },
        );
        assert!(
            msg.contains(note),
            "user-config redirect should carry note: {msg}"
        );

        // ...but not a project-config redirect.
        let msg = format_load_warning(
            "User config",
            &crate::config::UnknownWarning::NestedWrongConfig {
                path: "list.url".to_string(),
                other_description: "project config",
            },
        );
        assert!(
            !msg.contains(note),
            "project-config redirect should not carry note: {msg}"
        );

        // ...and not a misplaced root-only user scalar, even though its
        // destination is user config.
        let msg = format_load_warning(
            "Project config",
            &crate::config::UnknownWarning::TopLevelWrongConfig {
                key: "skip-shell-integration-prompt".to_string(),
                other_description: "user config",
            },
        );
        assert!(
            !msg.contains(note),
            "root-only user scalar should not carry note: {msg}"
        );
    }

    /// Every `DEPRECATION_RULES` row's migration fires on one config, pinning
    /// cross-rule interactions the per-rule tests can't see — in particular
    /// the inserted `[forge]` staying in the mid-file spot where the user
    /// wrote `[ci]` (it would render at the end without an explicit position;
    /// see [`migrate_ci_doc`]) and a `timeout-ms` under `[select]` being
    /// moved into `[switch.picker]` and then stripped.
    #[test]
    fn snapshot_migrate_all_rules_combined() {
        let content = r#"worktree-path = "../{{ repo_root }}.{{ branch }}"
pre-create = "npm install"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]

[ci]
platform = "github"

[select]
pager = "delta"
timeout-ms = 500

[merge]
no-ff = true

[switch]
no-cd = true

[post-create]
server = "npm run dev"

[projects."github.com/user/repo"]
approved-commands = ["npm test"]
"#;
        let migrated = compute_migrated_content(content);
        assert_eq!(
            compute_migrated_content(&migrated),
            migrated,
            "migration must be idempotent"
        );
        let remaining = detect_deprecations(&migrated);
        assert!(
            remaining.is_empty(),
            "applying the update must silence every warning; got {remaining:?}"
        );
        insta::assert_snapshot!(migration_diff(content, &migrated));
    }
}
