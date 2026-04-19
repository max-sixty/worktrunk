//! Alias command implementation.
//!
//! Aliases are user-defined commands configured in `[aliases]` sections of user
//! or project config. They share the foreground execution path with hooks via
//! `execute_pipeline_foreground` in `command_executor`, which handles pipeline
//! structure, template expansion, concurrent steps, and error handling.
//!
//! ## Execution model
//!
//! Aliases build `ForegroundStep`s from `CommandConfig::steps()`, preserving
//! pipeline structure (`Single` vs `Concurrent`). All commands use lazy template
//! expansion — `vars.*` references resolve from git config at execution time,
//! so prior steps that set vars via `wt config state vars set` are visible to
//! later steps.
//!
//! ## Why foreground and background execution differ
//!
//! Foreground execution (aliases + foreground hooks) uses `execute_shell_command`
//! which streams stdout/stderr to the terminal. Concurrency needs OS threads
//! (`thread::scope`), one per command. Background pipeline execution spawns
//! shell processes with stdout/stderr redirected to log files — no threads
//! needed. The two share preparation (`PreparedStep`) but not execution.
//!
//! ## Trust model
//!
//! User-config aliases are trusted (skip approval). Project-config aliases
//! require command approval. When both define the same alias, both run — user
//! first, then project. The CD directive file is passed through to child
//! processes so inner `wt` invocations can redirect the parent shell's cwd;
//! the EXEC directive file is scrubbed so alias bodies cannot inject
//! arbitrary shell into the interactive session.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, bail};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use color_print::cformat;
use worktrunk::config::{
    ALIAS_ARGS_KEY, CommandConfig, HookStep, ProjectConfig, UserConfig, append_aliases,
    referenced_vars_for_config,
};
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, println, progress_message, warning_message};

use crate::commands::command_approval::approve_alias_commands;
use crate::commands::command_executor::{
    CommandContext, CommandOrigin, FailureStrategy, ForegroundStep, PreparedCommand, PreparedStep,
    build_hook_context, execute_pipeline_foreground,
};
use crate::commands::did_you_mean;
use crate::commands::hooks::{format_pipeline_summary_from_names, step_names_from_config};
use crate::output::DirectivePassthrough;

/// Built-in `wt step` subcommand names. Aliases with these names are
/// reachable via `wt <name>` (top-level) but shadowed via `wt step <name>`.
const BUILTIN_STEP_COMMANDS: &[&str] = &[
    "commit",
    "copy-ignored",
    "diff",
    "eval",
    "for-each",
    "promote",
    "prune",
    "push",
    "rebase",
    "relocate",
    "squash",
];

/// Built-in top-level `wt` subcommand names — visible and hidden. Aliases
/// with these names are fully unreachable: clap matches the built-in before
/// alias dispatch sees the name, so `wt list` always runs the built-in even
/// if `[aliases] list = …` is configured. Kept in sync with `Cli` via
/// `test_top_level_builtins_match_clap`.
pub(crate) const TOP_LEVEL_BUILTINS: &[&str] = &[
    "config", "hook", "list", "merge", "remove", "select", "step", "switch",
];

/// Whether `--help` or `-h` appears in `args` before any `--` literal-forward
/// terminator. Aliases have no clap-style help page — the template *is* the
/// help — so dispatchers intercept help requests and redirect the user to
/// `wt config alias show / dry-run`. After `--`, all tokens forward to
/// `{{ args }}` verbatim, so `wt <alias> -- --help` bypasses the intercept.
fn help_flag_requested(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--help" || arg == "-h" {
            return true;
        }
    }
    false
}

/// Print guidance when `wt <alias> --help` is invoked. Points at the canonical
/// inspection path and documents the `--` escape for forwarding `--help` into
/// the alias body.
fn emit_alias_help_hint(name: &str) {
    println!(
        "`{name}` is an alias. Inspect with:
  wt config alias show {name}
  wt config alias dry-run {name}
Forward `--help` to the alias body with `wt {name} -- --help`."
    );
}

/// Options parsed from alias-dispatch args (`wt step <alias>` or `wt <alias>`).
#[derive(Debug)]
pub struct AliasOptions {
    pub name: String,
    pub vars: Vec<(String, String)>,
    /// Tokens forwarded to the template as `{{ args }}` (a `ShellArgs`
    /// sequence). Contains plain positionals, `--KEY=VALUE` / `--KEY` tokens
    /// whose key isn't referenced by the template, and everything after `--`.
    /// Appears in CLI order: `wt s foo --env=prod bar` with no `{{ env }}`
    /// reference collects `["foo", "--env=prod", "bar"]`.
    pub positional_args: Vec<String>,
}

impl AliasOptions {
    /// Parse alias options from `wt step <alias>` args, routing each token
    /// using `referenced_vars` (the union of `{{ key }}` references across the
    /// alias's pipeline templates).
    ///
    /// First element is the alias name. Remaining tokens are walked
    /// left-to-right under this grammar:
    ///
    /// - `--` — literal-forward escape: every later token goes straight into
    ///   `positional_args`, no var binding.
    /// - `--KEY=VALUE` or `--KEY VALUE` — binds `KEY=VALUE` if `KEY` is in
    ///   `referenced_vars`, otherwise forwards both parts as positionals. The
    ///   space form consumes the next token as the value unconditionally —
    ///   even when it starts with `--` — so `--env --other` with `env`
    ///   referenced binds `env=--other`. Use the `=` form or put flags
    ///   before the bound key to avoid this.
    /// - `--KEY` at end of args — forwards `--KEY` as a positional. No next
    ///   token to consume.
    /// - Anything else — forwards as a positional.
    ///
    /// `--yes`/`-y` is a top-level global flag (`wt -y <alias>`); the
    /// post-alias form is not recognized here. `--yes` follows the
    /// `--KEY` rule (forwards as positional unless `yes` is referenced),
    /// and `-y` is a bare positional. Clap's `global = true` doesn't
    /// propagate across `external_subcommand`, so the post-alias form
    /// never reaches the global parser anyway.
    ///
    /// `--dry-run` is no longer recognized — use `wt config alias dry-run <name>`
    /// instead. The parser raises an actionable error pointing at the new
    /// subcommand rather than silently forwarding the flag into `{{ args }}`.
    /// The bail fires only outside `literal_mode`, so `wt alias -- --dry-run`
    /// still forwards `--dry-run` as a positional.
    ///
    /// Hyphens in variable names are canonicalized to underscores before
    /// lookup and storage (minijinja parses `{{ my-var }}` as subtraction),
    /// so `--my-var=value` binds to `{{ my_var }}` when the template
    /// references it.
    ///
    /// `referenced_vars` is expected to contain the canonical underscore
    /// form. `referenced_vars_for_config` produces it directly from the
    /// alias's template.
    ///
    /// Returns `(options, warnings)`. Warnings are advisory — callers emit
    /// them; the parser stays pure so tests can inspect both halves.
    pub fn parse(
        args: Vec<String>,
        referenced_vars: &BTreeSet<String>,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let Some(name) = args.first().cloned() else {
            bail!("Missing alias name");
        };

        let mut vars = Vec::new();
        let mut positional_args = Vec::new();
        let mut warnings = Vec::new();
        let mut literal_mode = false;
        let mut i = 1;
        while i < args.len() {
            let arg = &args[i];
            if literal_mode {
                positional_args.push(arg.clone());
                i += 1;
                continue;
            }
            if arg == "--" {
                literal_mode = true;
                i += 1;
                continue;
            }
            if arg == "--dry-run" {
                bail!(
                    "--dry-run is no longer supported; use `wt config alias dry-run {name}` instead"
                );
            }
            if let Some(rest) = arg.strip_prefix("--") {
                if let Some((key, value)) = rest.split_once('=') {
                    if key.is_empty() {
                        bail!("invalid KEY=VALUE: key cannot be empty");
                    }
                    let canon = key.replace('-', "_");
                    if referenced_vars.contains(&canon) {
                        vars.push((canon, value.to_string()));
                    } else {
                        positional_args.push(arg.clone());
                    }
                    i += 1;
                    continue;
                }
                // Bare `--KEY`: mirror the atomic `--KEY=VALUE` form. When a
                // next token exists, consume both regardless of its shape —
                // bind if KEY is referenced, else forward both as positionals.
                // At end of args, forward `--KEY` alone.
                let canon = rest.replace('-', "_");
                if let Some(next) = args.get(i + 1) {
                    if referenced_vars.contains(&canon) {
                        // Warn on the footgun case: `--KEY --VALUE` with KEY
                        // referenced binds VALUE as the value. Almost always
                        // a typo — the user probably meant `--KEY=--VALUE`.
                        // Show the user's typed form (hyphenated) throughout;
                        // the template binding (underscored) is internal.
                        // Mirrors the `--var` TODO in hook_commands.rs.
                        if next.starts_with("--") {
                            warnings.push(format!(
                                "`--{rest} {next}` bound `{rest}` to `{next}` — use `--{rest}={next}` if that was intended"
                            ));
                        }
                        vars.push((canon, next.clone()));
                    } else {
                        positional_args.push(arg.clone());
                        positional_args.push(next.clone());
                    }
                    i += 2;
                    continue;
                }
                positional_args.push(arg.clone());
                i += 1;
                continue;
            }
            positional_args.push(arg.clone());
            i += 1;
        }

        Ok((
            Self {
                name,
                vars,
                positional_args,
            },
            warnings,
        ))
    }
}

/// Determine whether an alias requires project-config approval.
///
/// Returns the project-config commands for this alias, if any exist.
/// Project-config commands always need approval, regardless of whether
/// user config also defines the same alias — matching hook behavior.
fn alias_needs_approval(
    alias_name: &str,
    project_config: &Option<ProjectConfig>,
) -> Option<CommandConfig> {
    project_config
        .as_ref()
        .and_then(|pc| pc.aliases.get(alias_name))
        .cloned()
}

/// Synthesize clap's native `InvalidSubcommand` error for `wt step <name>`
/// and exit through `enhance_and_exit_error`, so the output matches what
/// `wt <typo>` produces at the top level. Suggestion candidates include both
/// the visible built-in `wt step` subcommands and the user's configured
/// aliases — `SuggestedSubcommand` takes arbitrary strings, so aliases show
/// up in the `tip:` line for typos like `wt step deplyo` → `'deploy'`.
fn unknown_step_command_exit(name: &str, alias_names: &[&str]) -> ! {
    let mut top = crate::cli::build_command();
    let step_cmd = top
        .find_subcommand_mut("step")
        .expect("`step` subcommand is defined in the CLI");
    // `render_usage` uses the command's `bin_name`, which clap only sets
    // after matching. When we synthesize the error ahead of that, the
    // subcommand has no bin_name and usage renders as `Usage: step
    // <COMMAND>` instead of `Usage: wt step [COMMAND]`. Set it to the same
    // display_name applied by `apply_help_template_recursive`.
    step_cmd.set_bin_name("wt step");
    let usage = step_cmd.render_usage();

    let builtins = step_cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name().to_string())
        .filter(|n| n != "help");
    let candidates = builtins.chain(alias_names.iter().map(|s| s.to_string()));
    let suggestions = did_you_mean(name, candidates);

    let mut err = clap::Error::new(ErrorKind::InvalidSubcommand).with_cmd(step_cmd);
    err.insert(
        ContextKind::InvalidSubcommand,
        ContextValue::String(name.to_string()),
    );
    if !suggestions.is_empty() {
        err.insert(
            ContextKind::SuggestedSubcommand,
            ContextValue::Strings(suggestions),
        );
    }
    err.insert(ContextKind::Usage, ContextValue::StyledStr(usage));
    crate::enhance_and_exit_error(err)
}

/// Format the "Running alias …" announcement.
///
/// For pipelines or concurrent groups with named commands, includes a summary
/// of the structure (e.g., `Running alias deploy: install; build, lint`).
/// When all commands are unnamed, falls back to the bare form
/// (`Running alias deploy`) — aliases have no natural fallback label for
/// unnamed steps the way hooks use `user`/`project`.
///
/// Sibling of `format_command_label` in `commands/mod.rs`, which builds the
/// non-pipeline `Running {type} {name}` form for hooks. Both apply bold
/// styling to the alias/command name — keep them in sync if styling evolves.
fn format_alias_announcement(name: &str, cmd_config: &CommandConfig) -> String {
    let step_names = step_names_from_config(cmd_config);
    let summary =
        format_pipeline_summary_from_names(&step_names, |n| cformat!("<bold>{n}</>"), |_| None);

    if summary.is_empty() {
        cformat!("Running alias <bold>{name}</>")
    } else {
        cformat!("Running alias <bold>{name}</>: {summary}")
    }
}

/// Load the merged alias map (user config + project config, in runtime order).
fn load_merged_aliases(
    repo: &Repository,
    user_config: &UserConfig,
    project_config: Option<&ProjectConfig>,
) -> BTreeMap<String, CommandConfig> {
    let project_id = repo.project_identifier().ok();
    let mut aliases = user_config.aliases(project_id.as_deref());
    if let Some(pc) = project_config {
        append_aliases(&mut aliases, &pc.aliases);
    }
    aliases
}

/// Try to run alias `name` with `rest` as its arg vector. Returns `Ok(None)`
/// when no alias by that name is configured — the caller can fall through to
/// other dispatch (e.g. `wt-<name>` PATH binary at the top level). Argument
/// parsing only runs after we've confirmed the alias is configured, so
/// non-alias `rest` (positional args meant for a `wt-<name>` PATH binary)
/// doesn't surface as a parse error.
///
/// `global_yes` is the top-level `--yes`/`-y` flag, passed through to
/// `run_alias`.
///
/// Alias execution needs a git repository; without one this returns `Ok(None)`
/// so the caller falls through to PATH lookup. Config load errors propagate —
/// a broken `wt.toml` should fail loudly here just as it does for `wt list`,
/// rather than silently turning into an "unrecognized subcommand" once we
/// fall through to PATH lookup.
pub fn try_alias(name: String, rest: Vec<String>, global_yes: bool) -> anyhow::Result<Option<()>> {
    let Ok(repo) = Repository::current() else {
        return Ok(None);
    };
    let user_config = UserConfig::load()?;
    let project_config = ProjectConfig::load(&repo, true)?;
    let aliases = load_merged_aliases(&repo, &user_config, project_config.as_ref());
    let Some(cmd_config) = aliases.get(&name) else {
        return Ok(None);
    };
    let referenced = referenced_vars_for_config(cmd_config)?;
    // Aliases can bind a `help` variable; only intercept `--help` when the
    // template doesn't reference it. `-h` is never a binding, so it's always
    // safe to intercept. Conservatively: intercept if any help flag appears
    // and no binding is claimed on `help`.
    if !referenced.contains("help") && help_flag_requested(&rest) {
        emit_alias_help_hint(&name);
        return Ok(Some(()));
    }
    let mut alias_args = Vec::with_capacity(1 + rest.len());
    alias_args.push(name);
    alias_args.extend(rest);
    let (opts, warnings) = AliasOptions::parse(alias_args, &referenced)?;
    run_alias(
        opts,
        warnings,
        repo,
        user_config,
        project_config,
        aliases,
        global_yes,
    )
    .map(Some)
}

/// Run a configured alias from `wt step <name>`. Errors with a clap-style
/// "unrecognized subcommand" if the alias isn't configured.
///
/// Argument parsing happens inside this function — not at the clap dispatch
/// site in `main.rs` — because the routing of `--KEY=VALUE` tokens depends on
/// which template variables the alias references, which requires the alias's
/// resolved `CommandConfig`.
///
/// `global_yes` is the top-level `--yes`/`-y` flag, passed through to
/// `run_alias`.
///
/// TODO: consider deprecating `wt step <alias>` in favor of top-level
/// `wt <alias>` (and `wt config alias show/dry-run` for inspection). The step
/// path exists today as the escape for shadowed names, but that could instead
/// be handled with a dedicated error hint.
pub fn step_alias(args: Vec<String>, global_yes: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let user_config = UserConfig::load()?;
    let project_config = ProjectConfig::load(&repo, true)?;
    let aliases = load_merged_aliases(&repo, &user_config, project_config.as_ref());
    let Some(name) = args.first().cloned() else {
        bail!("Missing alias name");
    };
    let Some(cmd_config) = aliases.get(&name) else {
        // Mirror clap's native `unrecognized subcommand` error so `wt step
        // <typo>` reads the same as `wt <typo>`. Aliases are fed into the
        // `SuggestedSubcommand` list so a typo like `wt step deplyo` still
        // gets `tip: ... 'deploy'` when `deploy` is user-defined. The
        // Aliases block in `wt step --help` is the full discovery surface —
        // the error just needs to point there via `Usage: wt step [COMMAND]`.
        let alias_names: Vec<&str> = aliases
            .keys()
            .filter(|k| !BUILTIN_STEP_COMMANDS.contains(&k.as_str()))
            .map(|k| k.as_str())
            .collect();
        unknown_step_command_exit(&name, &alias_names);
    };
    let referenced = referenced_vars_for_config(cmd_config)?;
    if !referenced.contains("help") && help_flag_requested(&args[1..]) {
        emit_alias_help_hint(&name);
        return Ok(());
    }
    let (opts, warnings) = AliasOptions::parse(args, &referenced)?;
    run_alias(
        opts,
        warnings,
        repo,
        user_config,
        project_config,
        aliases,
        global_yes,
    )
}

/// Return alias names for use as suggestions when a top-level subcommand is
/// not recognized. Best-effort — returns empty if config can't be loaded so
/// the suggestion list silently degrades to clap's built-in candidates.
pub fn alias_names_for_suggestions() -> Vec<String> {
    worktrunk::config::suppress_warnings();
    let Ok(repo) = Repository::current() else {
        return UserConfig::load()
            .map(|uc| uc.aliases(None).keys().cloned().collect())
            .unwrap_or_default();
    };
    let Ok(user_config) = UserConfig::load() else {
        return Vec::new();
    };
    let project_config = ProjectConfig::load(&repo, false).ok().flatten();
    load_merged_aliases(&repo, &user_config, project_config.as_ref())
        .keys()
        .cloned()
        .collect()
}

/// Execute `cmd_config` for `opts.name`. Caller must have already verified
/// `aliases.contains_key(&opts.name)`.
///
/// `global_yes` is the top-level `--yes`/`-y` flag and is the only source for
/// skipping approval — the post-alias form (`wt deploy --yes`) is no longer
/// recognized. Use `wt -y deploy` or `wt --yes deploy` instead.
fn run_alias(
    opts: AliasOptions,
    warnings: Vec<String>,
    repo: Repository,
    user_config: UserConfig,
    project_config: Option<ProjectConfig>,
    aliases: BTreeMap<String, CommandConfig>,
    global_yes: bool,
) -> anyhow::Result<()> {
    let cmd_config = aliases
        .get(&opts.name)
        .expect("caller verified alias is configured");

    // Surface parser advisories (e.g. `--KEY --VALUE` footgun) before
    // announcing the run so they're visible in execution output.
    for warning in &warnings {
        eprintln!("{}", warning_message(warning));
    }

    // Check if this alias needs project-config approval. project_id is required
    // for approval — re-derive with error propagation rather than relying on
    // `.ok()`. `global_yes` is the sole source for skipping approval now that
    // `wt <alias> --yes` (post-alias form) is unsupported.
    if let Some(project_commands) = alias_needs_approval(&opts.name, &project_config) {
        let project_id = repo
            .project_identifier()
            .context("Cannot determine project identifier for alias approval")?;
        let approved =
            approve_alias_commands(&project_commands, &opts.name, &project_id, global_yes)?;
        if !approved {
            return Ok(());
        }
    }

    // Build hook context for template expansion
    let wt = repo.current_worktree();
    let wt_path = wt.root().context("Failed to get worktree root")?;
    let branch = wt.branch().ok().flatten();
    let ctx = CommandContext::new(&repo, &user_config, branch.as_deref(), &wt_path, false);

    let extra_refs: Vec<(&str, &str)> = opts
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut context_map = build_hook_context(&ctx, &extra_refs)?;
    // Forward positional CLI args to templates as `{{ args }}`. Encoded as a
    // JSON list so it flows through the stable `HashMap<String, String>`
    // context — `expand_template` rehydrates it into a `ShellArgs` sequence.
    context_map.insert(
        ALIAS_ARGS_KEY.to_string(),
        serde_json::to_string(&opts.positional_args)
            .expect("Vec<String> serialization should never fail"),
    );

    // Build JSON context for stdin
    let context_json = serde_json::to_string(&context_map)
        .expect("HashMap<String, String> serialization should never fail");

    eprintln!(
        "{}",
        progress_message(format_alias_announcement(&opts.name, cmd_config))
    );

    // CD passed through, EXEC scrubbed (see `output::global` for rationale).
    let directives = DirectivePassthrough::inherit_from_env();

    // Build ForegroundSteps: all alias commands use lazy expansion so vars.*
    // references resolved from git config at execution time are visible to
    // later steps that set vars via `wt config state vars set`.
    let origin = CommandOrigin::Alias {
        name: opts.name.clone(),
    };
    let foreground_steps: Vec<ForegroundStep> = cmd_config
        .steps()
        .iter()
        .map(|step| {
            let prepared = match step {
                HookStep::Single(cmd) => {
                    PreparedStep::Single(alias_prepared_command(cmd, &context_json))
                }
                HookStep::Concurrent(cmds) => PreparedStep::Concurrent(
                    cmds.iter()
                        .map(|cmd| alias_prepared_command(cmd, &context_json))
                        .collect(),
                ),
            };
            ForegroundStep {
                step: prepared,
                origin: origin.clone(),
                concurrent: true,
            }
        })
        .collect();

    execute_pipeline_foreground(
        &foreground_steps,
        &repo,
        &wt_path,
        &directives,
        FailureStrategy::FailFast,
    )
}

/// Build a PreparedCommand for an alias, deferring template expansion to execution time.
fn alias_prepared_command(cmd: &worktrunk::config::Command, context_json: &str) -> PreparedCommand {
    PreparedCommand {
        name: cmd.name.clone(),
        expanded: cmd.template.clone(),
        context_json: context_json.to_string(),
        lazy_template: Some(cmd.template.clone()),
    }
}

/// Where an alias came from. When the same name is defined in both configs,
/// the listing shows both entries in runtime order (user first, then project)
/// rather than merging them, so users see the real commands from each source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AliasSource {
    User,
    Project,
}

impl AliasSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            AliasSource::User => "user",
            AliasSource::Project => "project",
        }
    }
}

/// Which help page is being augmented. Controls the "shadowed by built-in"
/// annotation: at the top level, only top-level built-ins shadow an alias
/// (`wt list` blocks an alias named `list`). Under `wt step`, only step
/// built-ins shadow it (`wt step commit` blocks an alias named `commit` from
/// running via that path — but `wt commit` still runs the alias).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpContext {
    TopLevel,
    Step,
}

/// Splice the Aliases section into clap-rendered help, if any aliases are
/// configured.
///
/// Called from the help path in `help.rs` — which covers `wt --help`,
/// `wt step --help`, and the bare `wt step` invocation (via
/// `arg_required_else_help`), all of which flow through clap's `DisplayHelp`
/// error. Tolerates running outside a repository: user-config aliases still
/// list, project-config aliases just get skipped.
pub(crate) fn augment_help(help: &str, context: HelpContext) -> String {
    // Help must not emit deprecation/unknown-field warnings or write `.new`
    // migration files as a side effect of rendering the alias list.
    worktrunk::config::suppress_warnings();

    let aliases = load_aliases_for_listing();
    if aliases.is_empty() {
        return help.to_string();
    }

    // Place the Aliases section right after Commands so it sits next to the
    // built-ins it extends. Clap has no template-level hook for inserting
    // between sections, so we splice around the `Options:` heading in the
    // rendered output. The search prefix is derived from the same style
    // clap uses (our `help_styles().get_header()`), so if the header
    // styling changes both sides move together.
    let aliases_section = render_aliases_section(&aliases, context);
    let options_heading = format!(
        "{}Options:",
        crate::cli::help_styles().get_header().render()
    );
    match help.find(&options_heading) {
        Some(pos) => format!("{}{aliases_section}\n\n{}", &help[..pos], &help[pos..]),
        None => {
            // Clap's styling changed; fall back to appending so we don't
            // silently drop the aliases list.
            format!("{help}\n{aliases_section}")
        }
    }
}

/// Format the list of aliases as a styled help section.
///
/// Matches clap's "Commands:" / "Options:" styling (bold+green heading,
/// bold+cyan names) so the Aliases section blends in with the rest of
/// `-h` output. Returns the block without leading or trailing blank lines —
/// the caller positions it.
///
/// When a name is defined in both user and project config, two rows are
/// shown (user first, then project, matching runtime order). Both rows
/// carry a source marker so the reader can tell which pipeline is which.
///
/// `context` controls the "shadowed by built-in" annotation — see
/// [`HelpContext`].
fn render_aliases_section(
    entries: &[(String, CommandConfig, AliasSource)],
    context: HelpContext,
) -> String {
    use std::fmt::Write as _;

    let shadowed_names: &[&str] = match context {
        HelpContext::TopLevel => TOP_LEVEL_BUILTINS,
        HelpContext::Step => BUILTIN_STEP_COMMANDS,
    };
    let is_shadowed = |name: &str| shadowed_names.contains(&name);

    // Names appearing in both sources need source markers to be distinguishable.
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, _, _) in entries {
        *counts.entry(name.as_str()).or_insert(0) += 1;
    }

    let mut out = String::new();
    let _ = writeln!(out, "{}", cformat!("<bold><green>Aliases:</></>"));
    let name_width = entries.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let mut first = true;
    for (name, cfg, source) in entries {
        if !first {
            out.push('\n');
        }
        first = false;
        let padding = " ".repeat(name_width - name.len());
        let summary = format_alias_summary(cfg);
        // Shadowed-by-builtin is a warning (yellow) and takes precedence over
        // the source marker so the row doesn't pile up suffixes.
        let suffix = if is_shadowed(name) {
            cformat!(" <yellow>(shadowed by built-in)</>")
        } else if counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
            match source {
                AliasSource::User => cformat!(" <dim>(user)</>"),
                AliasSource::Project => cformat!(" <dim>(project)</>"),
            }
        } else {
            String::new()
        };
        let _ = write!(
            out,
            "  {name_styled}{padding}  {summary}{suffix}",
            name_styled = cformat!("<bold><cyan>{name}</></>"),
        );
    }
    out
}

/// Load aliases for display as a flat list sorted by name, with source tagged.
///
/// Duplicate names (same alias in both user and project) appear twice — once
/// per source, user first, matching runtime execution order. Showing each
/// separately preserves the individual command text; merging them would
/// reduce to an uninformative step count when both are unnamed singles.
///
/// The caller (`augment_step_help`) latches `suppress_warnings()` before
/// reaching here so the standard `UserConfig::load()` stays quiet: no
/// deprecation warnings, no `.new` file writes, no approved-commands copy.
/// Project config is parsed directly from TOML rather than via
/// `ProjectConfig::load` because the `aliases` table has no deprecated forms
/// — skipping the migration avoids the unrelated warnings entirely.
///
/// Tolerates missing or unloadable config: this is a discovery surface, not
/// an execution surface, so we'd rather show the built-in commands than
/// error out when a repo isn't detected or a config file is malformed.
/// `step_alias` surfaces those errors at execution time.
fn load_aliases_for_listing() -> Vec<(String, CommandConfig, AliasSource)> {
    let repo = Repository::current().ok();
    let project_id = repo.as_ref().and_then(|r| r.project_identifier().ok());

    let user_aliases = UserConfig::load()
        .ok()
        .map(|uc| uc.aliases(project_id.as_deref()))
        .unwrap_or_default();

    let project_aliases = repo
        .as_ref()
        .and_then(load_project_aliases_silent)
        .unwrap_or_default();

    let mut entries: Vec<(String, CommandConfig, AliasSource)> = user_aliases
        .into_iter()
        .map(|(n, c)| (n, c, AliasSource::User))
        .chain(
            project_aliases
                .into_iter()
                .map(|(n, c)| (n, c, AliasSource::Project)),
        )
        .collect();

    // Sort by name; for ties, user before project (derived Ord on AliasSource)
    // so duplicates display in runtime execution order.
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    entries
}

/// Parse `.config/wt.toml` directly, extracting just `aliases`, without
/// triggering `ProjectConfig::load`'s deprecation warning and hint-writing
/// side effects. See `load_aliases_for_listing` for why.
fn load_project_aliases_silent(repo: &Repository) -> Option<BTreeMap<String, CommandConfig>> {
    let path = repo.project_config_path().ok().flatten()?;
    if !path.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&path).ok()?;
    let config: ProjectConfig = toml::from_str(&contents).ok()?;
    Some(config.aliases)
}

/// One-line summary of an alias's command(s) suitable for a help listing.
///
/// Single-command aliases show the template's first line (with `…` if the
/// template spans multiple lines). Pipelines show the shared named-step
/// summary used by the "Running alias" announcement.
fn format_alias_summary(cfg: &CommandConfig) -> String {
    // `is_pipeline()` is `steps.len() > 1`, but a single-step concurrent
    // alias (one `HookStep::Concurrent` holding several commands) would
    // fall into the else branch and hide all but the first command. Count
    // actual commands instead.
    if cfg.commands().count() > 1 {
        let step_names = step_names_from_config(cfg);
        let summary = format_pipeline_summary_from_names(&step_names, |n| n.to_string(), |_| None);
        if summary.is_empty() {
            format!("<{} steps>", cfg.commands().count())
        } else {
            summary
        }
    } else {
        let cmd = cfg
            .commands()
            .next()
            .expect("CommandConfig always contains at least one command");
        let first = cmd.template.lines().next().unwrap_or("").trim_end();
        if cmd.template.lines().count() > 1 {
            format!("{first}…")
        } else {
            first.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;

    /// Parse with an explicit `referenced_vars` set. Tests build the set
    /// directly to exercise routing without needing a full template fixture.
    /// Drops warnings — see `parse_with_warnings` to inspect them.
    fn parse_with(args: &[&str], referenced: &[&str]) -> anyhow::Result<AliasOptions> {
        parse_with_warnings(args, referenced).map(|(opts, _)| opts)
    }

    /// Like `parse_with` but also returns the warning vector, for tests that
    /// assert on advisory output.
    fn parse_with_warnings(
        args: &[&str],
        referenced: &[&str],
    ) -> anyhow::Result<(AliasOptions, Vec<String>)> {
        let refs: BTreeSet<String> = referenced.iter().map(|s| s.to_string()).collect();
        AliasOptions::parse(args.iter().map(|s| s.to_string()).collect(), &refs)
    }

    /// Convenience wrapper for tests that don't care about var routing —
    /// every `--KEY=VALUE` token forwards as a positional.
    fn parse(args: &[&str]) -> anyhow::Result<AliasOptions> {
        parse_with(args, &[])
    }

    /// Parse a TOML snippet of the form `cmd = ...` into a CommandConfig.
    /// CommandConfig has no public multi-step constructor, so round-tripping
    /// through TOML is the simplest way to build pipeline fixtures.
    fn cfg_from_toml(toml_str: &str) -> CommandConfig {
        #[derive(serde::Deserialize)]
        struct Wrap {
            cmd: CommandConfig,
        }
        toml::from_str::<Wrap>(toml_str).unwrap().cmd
    }

    #[test]
    fn test_format_alias_announcement_single_unnamed() {
        let cfg = cfg_from_toml(r#"cmd = "echo hi""#);
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy");
    }

    #[test]
    fn test_format_alias_announcement_pipeline_all_unnamed() {
        // Pipeline of unnamed strings → no summary suffix.
        let cfg = cfg_from_toml(r#"cmd = ["echo a", "echo b"]"#);
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy");
    }

    #[test]
    fn test_format_alias_announcement_concurrent_named() {
        // Single concurrent step (named table form).
        let cfg = cfg_from_toml(
            r#"
[cmd]
build = "cargo build"
test = "cargo test"
"#,
        );
        let msg = format_alias_announcement("check", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias check: build, test");
    }

    #[test]
    fn test_format_alias_announcement_pipeline_named() {
        // Pipeline: serial named step then concurrent named step.
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        let msg = format_alias_announcement("deploy", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias deploy: install; build, lint");
    }

    #[test]
    fn test_format_alias_announcement_mixed_named_unnamed() {
        // Pipeline mixing anonymous strings and named steps. Unnamed entries
        // are skipped from the summary (aliases have no fallback label).
        let cfg = cfg_from_toml(
            r#"
cmd = [
    "echo first",
    { build = "cargo build", test = "cargo test" },
]
"#,
        );
        let msg = format_alias_announcement("ci", &cfg);
        insta::assert_snapshot!(msg.ansi_strip(), @"Running alias ci: build, test");
    }

    #[test]
    fn test_parse_built_in_flags() {
        use insta::assert_debug_snapshot;
        // Plain alias name only.
        assert_debug_snapshot!(parse(&["deploy"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [],
        }
        "#);
    }

    #[test]
    fn test_parse_key_value_routing() {
        use insta::assert_debug_snapshot;
        // --KEY=VALUE binds when the template references KEY.
        assert_debug_snapshot!(parse_with(&["deploy", "--env=staging"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "staging",
                ),
            ],
            positional_args: [],
        }
        "#);
        // --KEY=VALUE forwards as positional when KEY is NOT referenced.
        assert_debug_snapshot!(parse_with(&["deploy", "--env=staging"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--env=staging",
            ],
        }
        "#);
        // Equals-in-value still parses correctly when bound.
        assert_debug_snapshot!(parse_with(&["deploy", "--url=http://host?a=1"], &["url"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "url",
                    "http://host?a=1",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Pathological: multiple `=` in value. `split_once('=')` only splits on
        // the first, so everything past the first `=` is the value.
        assert_debug_snapshot!(parse_with(&["deploy", "--foo=a=b=c"], &["foo"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "foo",
                    "a=b=c",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Empty value accepted on bind.
        assert_debug_snapshot!(parse_with(&["deploy", "--env="], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Empty value forwarded literally when KEY is not referenced.
        assert_debug_snapshot!(parse_with(&["deploy", "--env="], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--env=",
            ],
        }
        "#);
    }

    #[test]
    fn test_parse_space_separated_routing() {
        use insta::assert_debug_snapshot;
        // --KEY VALUE binds when KEY is referenced.
        assert_debug_snapshot!(parse_with(&["deploy", "--env", "staging"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "staging",
                ),
            ],
            positional_args: [],
        }
        "#);
        // --KEY VALUE forwards both as positionals when KEY is NOT referenced.
        assert_debug_snapshot!(parse_with(&["deploy", "--env", "staging"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--env",
                "staging",
            ],
        }
        "#);
        // --KEY --other with KEY referenced: the space form consumes the next
        // token unconditionally, so `--other` binds as the value of `env`.
        // Use `--env=VALUE` or reorder flags to avoid this.
        assert_debug_snapshot!(parse_with(&["deploy", "--env", "--other"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "--other",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Same shape, unreferenced: both consumed as the pair and forwarded.
        // The next token is not re-examined, so `--other` is not processed
        // independently.
        assert_debug_snapshot!(parse_with(&["deploy", "--env", "--other"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--env",
                "--other",
            ],
        }
        "#);
        // --KEY at end of args: nothing to consume, forwards alone.
        assert_debug_snapshot!(parse_with(&["deploy", "--env"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--env",
            ],
        }
        "#);
    }

    #[test]
    fn test_parse_warns_on_footgun_space_form() {
        // `--KEY VALUE` binds VALUE even when it's flag-shaped. The parser
        // emits an advisory pointing at `--KEY=VALUE` so the user can
        // disambiguate if the bind was unintended.
        let (_, warnings) = parse_with_warnings(&["deploy", "--env", "--other"], &["env"]).unwrap();
        insta::assert_debug_snapshot!(warnings, @r#"
        [
            "`--env --other` bound `env` to `--other` — use `--env=--other` if that was intended",
        ]
        "#);

        // Unreferenced pair: both tokens forward as positionals — not a
        // binding, so no warning.
        let (_, warnings) = parse_with_warnings(&["deploy", "--env", "--other"], &[]).unwrap();
        assert!(warnings.is_empty());

        // Ordinary bind: warning stays silent.
        let (_, warnings) = parse_with_warnings(&["deploy", "--env", "prod"], &["env"]).unwrap();
        assert!(warnings.is_empty());

        // Hyphenated key: warning shows the user's typed form throughout
        // (`my-env`), not the canonicalized template name (`my_env`).
        let (_, warnings) =
            parse_with_warnings(&["deploy", "--my-env", "--other"], &["my_env"]).unwrap();
        insta::assert_debug_snapshot!(warnings, @r#"
        [
            "`--my-env --other` bound `my-env` to `--other` — use `--my-env=--other` if that was intended",
        ]
        "#);
    }

    #[test]
    fn test_parse_duplicate_key_last_write_wins() {
        use insta::assert_debug_snapshot;
        // Duplicate `--KEY=` bindings are kept in order in the Vec;
        // `build_hook_context` inserts them into a HashMap so the last one
        // wins at expansion time. Parser preserves order so that behavior is
        // testable and deterministic.
        assert_debug_snapshot!(parse_with(&["deploy", "--env=a", "--env=b"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "a",
                ),
                (
                    "env",
                    "b",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Mixed space/equals forms follow the same rule.
        assert_debug_snapshot!(parse_with(&["deploy", "--env", "a", "--env=b"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "a",
                ),
                (
                    "env",
                    "b",
                ),
            ],
            positional_args: [],
        }
        "#);
    }

    #[test]
    fn test_parse_hyphen_canonicalization() {
        use insta::assert_debug_snapshot;
        // Hyphens in the key are canonicalized to underscores before lookup
        // and storage. The set is keyed in canonical form.
        assert_debug_snapshot!(parse_with(&["deploy", "--my-var=value"], &["my_var"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Already-underscored keys pass through.
        assert_debug_snapshot!(parse_with(&["deploy", "--my_var=value"], &["my_var"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "my_var",
                    "value",
                ),
            ],
            positional_args: [],
        }
        "#);
        // Hyphens in the value are preserved (only the key is canonicalized).
        assert_debug_snapshot!(parse_with(&["deploy", "--region=us-east-1"], &["region"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "region",
                    "us-east-1",
                ),
            ],
            positional_args: [],
        }
        "#);
    }

    #[test]
    fn test_parse_literal_forward_escape() {
        use insta::assert_debug_snapshot;
        // After `--`, every token is positional regardless of whether it
        // looks like a flag or whether the template would have bound it.
        assert_debug_snapshot!(parse_with(&["deploy", "--env=staging", "--", "--env=other", "x"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "staging",
                ),
            ],
            positional_args: [
                "--env=other",
                "x",
            ],
        }
        "#);
        // `--` itself is consumed but does not appear in positionals;
        // built-in flags after `--` forward as positional too.
        assert_debug_snapshot!(parse_with(&["deploy", "--", "--yes", "-y", "--dry-run"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--yes",
                "-y",
                "--dry-run",
            ],
        }
        "#);
        // Trailing `--` with nothing after: consumed silently, no positionals.
        assert_debug_snapshot!(parse_with(&["deploy", "--"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [],
        }
        "#);
        // A second `--` inside literal mode is just another positional —
        // literal_mode never resets, matching POSIX `--` semantics.
        assert_debug_snapshot!(parse_with(&["deploy", "--", "a", "--", "b"], &[]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "a",
                "--",
                "b",
            ],
        }
        "#);
    }

    #[test]
    fn test_parse_mixed_pipeline() {
        use insta::assert_debug_snapshot;
        // Multiple referenced vars bind from a mix of `--KEY VALUE` and
        // `--KEY=VALUE` forms; bare positionals interleave.
        assert_debug_snapshot!(
            parse_with(
                &["deploy", "--env", "prod", "--region=us-east", "thing"],
                &["env", "region"],
            ).unwrap(),
            @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "prod",
                ),
                (
                    "region",
                    "us-east",
                ),
            ],
            positional_args: [
                "thing",
            ],
        }
        "#
        );
    }

    #[test]
    fn test_parse_positionals() {
        use insta::assert_debug_snapshot;
        // Single positional forwarded.
        assert_debug_snapshot!(parse(&["s", "some-branch"]).unwrap(), @r#"
        AliasOptions {
            name: "s",
            vars: [],
            positional_args: [
                "some-branch",
            ],
        }
        "#);
        // Multiple positionals preserve CLI order.
        assert_debug_snapshot!(parse(&["deploy", "one", "two", "three"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "one",
                "two",
                "three",
            ],
        }
        "#);
        // Positionals can interleave with flags; flags bind when referenced, positionals keep order.
        assert_debug_snapshot!(parse_with(&["deploy", "foo", "--env=prod", "bar"], &["env"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [
                (
                    "env",
                    "prod",
                ),
            ],
            positional_args: [
                "foo",
                "bar",
            ],
        }
        "#);
        // Positionals with shell metacharacters pass through verbatim —
        // escaping happens at template render time.
        assert_debug_snapshot!(parse(&["deploy", "foo bar", "x;rm -rf /"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "foo bar",
                "x;rm -rf /",
            ],
        }
        "#);
        // Post-alias `--yes` / `-y` are NOT recognized as approval-skip
        // flags — the global `wt -y <alias>` form is the only path.
        // `--yes` follows the `--KEY`-no-value rule and forwards as
        // positional (since `yes` won't be a referenced template var).
        // `-y` is a bare positional. Clap's `global = true` doesn't
        // propagate across `external_subcommand`, so post-alias forms
        // never reach the global parser anyway.
        assert_debug_snapshot!(parse(&["deploy", "--yes"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "--yes",
            ],
        }
        "#);
        assert_debug_snapshot!(parse(&["deploy", "-y"]).unwrap(), @r#"
        AliasOptions {
            name: "deploy",
            vars: [],
            positional_args: [
                "-y",
            ],
        }
        "#);
    }

    #[test]
    fn test_parse_errors() {
        use insta::assert_snapshot;
        assert_snapshot!(parse(&[]).unwrap_err(), @"Missing alias name");
        // `--=value` has an empty key — caught even when bind would forward.
        assert_snapshot!(parse(&["deploy", "--=value"]).unwrap_err(), @"invalid KEY=VALUE: key cannot be empty");
        // Retired `--dry-run` flag gives an actionable error pointing at the new subcommand.
        assert_snapshot!(parse(&["deploy", "--dry-run"]).unwrap_err(), @"--dry-run is no longer supported; use `wt config alias dry-run deploy` instead");
    }

    /// `referenced_vars_for_config` unions across pipeline steps so a var
    /// referenced in any one command is a binding candidate for the whole
    /// alias.
    #[test]
    fn test_referenced_vars_for_config_unions_steps() {
        let cfg = cfg_from_toml(
            r#"
cmd = [
    "echo {{ env }}",
    { build = "make {{ target }}", lint = "lint {{ args }}" },
]
"#,
        );
        let refs = worktrunk::config::referenced_vars_for_config(&cfg).unwrap();
        let names: Vec<&str> = refs.iter().map(String::as_str).collect();
        assert_eq!(names, vec!["args", "env", "target"]);
    }

    /// Verify BUILTIN_STEP_COMMANDS stays in sync with the actual StepCommand variants.
    ///
    /// If a new step subcommand is added without updating BUILTIN_STEP_COMMANDS,
    /// this test fails — preventing aliases from silently conflicting with built-ins.
    #[test]
    fn test_builtin_step_commands_matches_clap() {
        use crate::cli::Cli;
        use clap::CommandFactory;

        let app = Cli::command();
        let step_cmd = app
            .get_subcommands()
            .find(|c| c.get_name() == "step")
            .expect("step subcommand exists");

        let clap_names: Vec<&str> = step_cmd.get_subcommands().map(|s| s.get_name()).collect();

        // Every clap subcommand should be in BUILTIN_STEP_COMMANDS
        for name in &clap_names {
            assert!(
                BUILTIN_STEP_COMMANDS.contains(name),
                "Step subcommand '{name}' is missing from BUILTIN_STEP_COMMANDS. \
                 Add it to prevent aliases from silently conflicting with the built-in."
            );
        }

        // Every BUILTIN_STEP_COMMANDS entry should still be a real subcommand
        for name in BUILTIN_STEP_COMMANDS {
            assert!(
                clap_names.contains(name),
                "BUILTIN_STEP_COMMANDS contains '{name}' but no such step subcommand exists. \
                 Remove it from the list."
            );
        }
    }

    /// Verify TOP_LEVEL_BUILTINS stays in sync with the actual `Cli` enum.
    ///
    /// Hidden subcommands like `select` count — clap still matches them
    /// before falling through to alias dispatch, so they shadow aliases of
    /// the same name. Only `help` (clap-internal) and custom subcommands
    /// are excluded.
    #[test]
    fn test_top_level_builtins_match_clap() {
        use crate::cli::Cli;
        use clap::CommandFactory;

        let app = Cli::command();
        let clap_names: Vec<&str> = app
            .get_subcommands()
            .map(|s| s.get_name())
            .filter(|n| *n != "help")
            .collect();

        for name in &clap_names {
            assert!(
                TOP_LEVEL_BUILTINS.contains(name),
                "Top-level subcommand '{name}' is missing from TOP_LEVEL_BUILTINS. \
                 Add it so the help splice annotates aliases unreachable at the top level."
            );
        }
        for name in TOP_LEVEL_BUILTINS {
            assert!(
                clap_names.contains(name),
                "TOP_LEVEL_BUILTINS contains '{name}' but no such top-level subcommand exists. \
                 Remove it from the list."
            );
        }
    }

    #[test]
    fn test_format_alias_summary_single_command() {
        let cfg = cfg_from_toml(r#"cmd = "echo hello""#);
        assert_eq!(format_alias_summary(&cfg), "echo hello");
    }

    #[test]
    fn test_format_alias_summary_multiline_gets_ellipsis() {
        let cfg = cfg_from_toml(
            r#"cmd = """
git fetch --all --prune
git rebase @{u}
""""#,
        );
        assert_eq!(format_alias_summary(&cfg), "git fetch --all --prune…");
    }

    #[test]
    fn test_format_alias_summary_pipeline_named() {
        let cfg = cfg_from_toml(
            r#"
cmd = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" },
]
"#,
        );
        assert_eq!(format_alias_summary(&cfg), "install; build, lint");
    }

    #[test]
    fn test_format_alias_summary_concurrent_named() {
        // Single-step concurrent form: `[aliases.check]\nbuild=…\ntest=…`
        // — one step, multiple commands. Must use the pipeline formatter,
        // not fall back to "show first command's template".
        let cfg = cfg_from_toml(
            r#"
[cmd]
build = "cargo build"
test = "cargo test"
"#,
        );
        assert_eq!(format_alias_summary(&cfg), "build, test");
    }

    #[test]
    fn test_format_alias_summary_pipeline_all_unnamed() {
        // Anonymous pipeline entries fall back to a step count.
        let cfg = cfg_from_toml(r#"cmd = ["echo a", "echo b"]"#);
        assert_eq!(format_alias_summary(&cfg), "<2 steps>");
    }

    #[test]
    fn test_render_aliases_section_source_annotations() {
        // Names unique to one source have no annotation. Names defined in
        // both sources show two rows (user first, matching runtime order)
        // and each row carries a source marker so the reader can tell them
        // apart. Shadowed-by-builtin takes precedence over the source marker.
        let entries = vec![
            (
                "only-user".to_string(),
                cfg_from_toml(r#"cmd = "echo u""#),
                AliasSource::User,
            ),
            (
                "only-project".to_string(),
                cfg_from_toml(r#"cmd = "echo p""#),
                AliasSource::Project,
            ),
            (
                "shared".to_string(),
                cfg_from_toml(r#"cmd = "echo from-user""#),
                AliasSource::User,
            ),
            (
                "shared".to_string(),
                cfg_from_toml(r#"cmd = "echo from-project""#),
                AliasSource::Project,
            ),
        ];
        // Caller passes pre-sorted entries; mirror that here.
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let rendered = render_aliases_section(&sorted, HelpContext::Step);
        let rendered = rendered.ansi_strip();
        insta::assert_snapshot!(rendered, @r"
        Aliases:
          only-project  echo p
          only-user     echo u
          shared        echo from-user (user)
          shared        echo from-project (project)
        ");
    }

    #[test]
    fn test_render_aliases_section_top_level_shadowing() {
        // Top-level shadowing: an alias named after a top-level built-in (e.g.
        // `list`) is unreachable from `wt list` because clap matches first.
        // Step-only built-in names (e.g. `commit`) are NOT shadowed at the top
        // level — `wt commit` runs the alias.
        let entries = vec![
            (
                "list".to_string(),
                cfg_from_toml(r#"cmd = "ls""#),
                AliasSource::User,
            ),
            (
                "commit".to_string(),
                cfg_from_toml(r#"cmd = "git commit""#),
                AliasSource::User,
            ),
            (
                "deploy".to_string(),
                cfg_from_toml(r#"cmd = "make deploy""#),
                AliasSource::User,
            ),
        ];
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let rendered = render_aliases_section(&sorted, HelpContext::TopLevel);
        let rendered = rendered.ansi_strip();
        insta::assert_snapshot!(rendered, @r"
        Aliases:
          commit  git commit
          deploy  make deploy
          list    ls (shadowed by built-in)
        ");
    }
}
