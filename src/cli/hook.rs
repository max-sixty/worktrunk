//! `wt hook` CLI surface.
//!
//! Hook invocations share a single `#[command(external_subcommand)]` catch-all
//! ([`HookCommand::Run`]) that captures everything after `wt hook <type>` as
//! raw argv — including the `--` literal-forward separator, which clap eats
//! in a trailing_var_arg positional but preserves in external_subcommand.
//! [`HookOptions::parse`] walks the argv with the same smart-routing rule as
//! `AliasOptions::parse`: `--KEY=VALUE` binds `{{ KEY }}` when any hook
//! template references it; otherwise the token forwards to `{{ args }}`.
//!
//! The collapse mirrors the alias pattern. Completion for hook type names
//! and help-rendering of `wt hook --help` / `wt hook <type> --help` both go
//! through `completion::inject_hook_subcommands`, which grafts stub
//! subcommands onto an augmented `Command` tree used only for those two
//! surfaces — the real parser still dispatches through `HookCommand::Run`.
//!
//! # External interface: hooks vs. aliases
//!
//! Both surfaces share the core smart-routing rule (`--KEY=VALUE` binds if
//! referenced, else forwards to `{{ args }}`), post-`--` literal forwarding,
//! hyphen-to-underscore key canonicalization, and the `ShellArgs` rendering
//! of `{{ args }}`. Where they diverge:
//!
//! | Axis | Hooks | Aliases |
//! |------|-------|---------|
//! | Invocation | `wt hook <type> [args...]` — nested external_subcommand under the `hook` built-in | `wt <name> [args...]` — top-level external_subcommand (`Cli::Custom`) |
//! | Bare positionals | **Filter names** (`wt hook pre-merge test build` runs only `test` and `build`) | Forwarded to `{{ args }}` |
//! | Reach `{{ args }}` from positionals | Must use `--` (`wt hook pre-merge -- extra`) | Any bare positional lands there |
//! | Approval skip flag | Post-subcommand `--yes` / `-y` recognized by [`HookOptions::parse`] | Only the global form (`wt -y <alias>`); post-alias `--yes` falls through to `{{ args }}` |
//! | Source discrimination | `user:` / `project:` / `user:name` / `project:name` filter syntax | Aliases run user first, then project; no filter syntax |
//! | Force-bind escape | `--var KEY=VALUE` (deprecated; emits warning; still binds unconditionally) | No equivalent — smart routing is the only path |
//! | Name validation | [`parse_hook_type`] validates the hook type with a did-you-mean hint | No validation — unknown alias falls through to `wt-<name>` PATH binary lookup |
//! | `--help` | Clap-rendered via injected stubs (both `wt hook --help` and `wt hook <type> --help`) | `wt <alias> --help` redirects to `wt config alias show` / `dry-run` |
//! | Inspection | `wt hook show [type] [--expanded]` | `wt config alias show <name>` / `dry-run <name>` |
//! | Trust / approval | User hooks trusted; project hooks require approval per-hook-type | User aliases trusted; project aliases require approval per-alias |
//! | Hook-specific flags | `--dry-run`, `--foreground`, `--var` parsed by [`HookOptions::parse`] | None — aliases have no CLI-level knobs beyond smart routing |
//! | Template-context extras | `hook_type`, `hook_name`, per-type operation vars (`base`, `target`, `pr_number`, …) | `args` only, on top of the shared base vars |

use std::ffi::OsString;

use anyhow::{Context, bail};
use clap::Subcommand;
use worktrunk::HookType;

use super::config::ApprovalsCommand;

/// Canonical list of hook type names accepted after `wt hook`. Shared by
/// [`parse_hook_type`], `completion::inject_hook_subcommands`, and the
/// `wt hook show` value parser so drift is caught by tests rather than at
/// runtime. `post-create` is the deprecated alias for `pre-start`; it's
/// accepted by [`parse_hook_type`] but not listed here — completion lists
/// the canonical names only.
pub const HOOK_TYPE_NAMES: &[&str] = &[
    "pre-switch",
    "post-switch",
    "pre-start",
    "post-start",
    "pre-commit",
    "post-commit",
    "pre-merge",
    "post-merge",
    "pre-remove",
    "post-remove",
];

// Ordering: `show` first (read-only introspection), then the external
// subcommand catch-all, then hidden commands. Hook types aren't listed
// as clap variants — `Run` catches them.
/// Run configured hooks
#[derive(Subcommand)]
pub enum HookCommand {
    /// Show configured hooks
    ///
    /// Lists user and project hooks. Project hooks show approval status (❓ = needs approval).
    Show {
        /// Hook type to show (default: all)
        #[arg(value_parser = ["pre-switch", "post-switch", "pre-start", "post-start", "pre-commit", "post-commit", "pre-merge", "post-merge", "pre-remove", "post-remove"])]
        hook_type: Option<String>,

        /// Show expanded commands with current variables
        #[arg(long)]
        expanded: bool,
    },

    /// Internal: run a serialized pipeline from stdin
    #[command(hide = true, name = "run-pipeline")]
    RunPipeline,

    /// Deprecated: use `wt config approvals` instead
    #[command(hide = true)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },

    /// Captures `wt hook <type> [ARGS...]` as raw argv. First element is the
    /// hook type name (clap doesn't validate — [`HookOptions::parse`] does,
    /// with a did-you-mean error for typos). External_subcommand preserves
    /// the `--` literal-forward separator, which a `trailing_var_arg`
    /// positional would eat.
    #[command(external_subcommand)]
    Run(Vec<OsString>),
}

/// Parsed form of `wt hook <type> [ARGS...]` — hook type plus every flag,
/// filter name, shorthand binding, and forwarded arg, routed per the alias
/// smart-routing model. `run_hook` consumes this.
#[derive(Debug)]
pub struct HookOptions {
    pub hook_type: HookType,
    pub yes: bool,
    pub dry_run: bool,
    /// `Some(true)` forces foreground for `post-*` hooks that normally run
    /// in the background. `None` defers to the hook type's default.
    pub foreground: Option<bool>,
    /// Positional filter names (`wt hook pre-merge test build`).
    pub name_filters: Vec<String>,
    /// Explicit `--var KEY=VALUE` bindings (deprecated force-bind).
    pub explicit_vars: Vec<(String, String)>,
    /// Raw `--KEY=VALUE` shorthand tokens, stored as `KEY=VALUE` (the
    /// original hyphenated key is preserved for forwarding to `{{ args }}`
    /// when unreferenced).
    pub shorthand_vars: Vec<String>,
    /// Tokens after `--` that forward to `{{ args }}` verbatim.
    pub forwarded_args: Vec<String>,
}

/// Map a hook type name to its [`HookType`] variant. Emits a did-you-mean
/// hint on typos (same `did_you_mean` helper used for unknown subcommands).
///
/// `post-create` is the deprecated alias for `pre-start` — accepted here so
/// scripted invocations keep working. The deprecation warning is emitted by
/// the config loader when `[post-create]` appears in config; CLI invocations
/// map silently to `pre-start`.
pub fn parse_hook_type(name: &str) -> anyhow::Result<HookType> {
    match name {
        "pre-switch" => Ok(HookType::PreSwitch),
        "post-switch" => Ok(HookType::PostSwitch),
        "pre-start" | "post-create" => Ok(HookType::PreStart),
        "post-start" => Ok(HookType::PostStart),
        "pre-commit" => Ok(HookType::PreCommit),
        "post-commit" => Ok(HookType::PostCommit),
        "pre-merge" => Ok(HookType::PreMerge),
        "post-merge" => Ok(HookType::PostMerge),
        "pre-remove" => Ok(HookType::PreRemove),
        "post-remove" => Ok(HookType::PostRemove),
        other => {
            let candidates = HOOK_TYPE_NAMES.iter().map(|s| s.to_string());
            let suggestions = crate::commands::did_you_mean(other, candidates);
            if let Some(suggestion) = suggestions.first() {
                bail!("unknown hook type: `{other}` (did you mean `{suggestion}`?)");
            }
            bail!(
                "unknown hook type: `{other}` (expected one of: {})",
                HOOK_TYPE_NAMES.join(", ")
            );
        }
    }
}

impl HookOptions {
    /// Parse `args` as `<hook-type> [FLAGS...] [NAME...] [--KEY=VALUE...] [-- TOKENS...]`.
    ///
    /// First element is the hook type name. Remaining tokens are walked
    /// left-to-right under this grammar:
    ///
    /// - `--yes` / `-y` — set `yes` (equivalent to the global `-y` flag,
    ///   supported post-type so `wt hook pre-merge --yes` works).
    /// - `--dry-run` — set `dry_run`.
    /// - `--foreground` — set `foreground = Some(true)` (post-* hooks).
    /// - `--var KEY=VALUE` / `--var=KEY=VALUE` — explicit force-bind;
    ///   appended to `explicit_vars`. Deprecated; dispatch warns.
    /// - `--KEY=VALUE` (other `--` flag with `=`) — captured as shorthand
    ///   for `run_hook` to smart-route (bind if referenced, else forward).
    /// - `--` — literal-forward escape; every later token goes into
    ///   `forwarded_args`.
    /// - Anything else — positional filter name.
    pub fn parse(args: &[OsString]) -> anyhow::Result<Self> {
        let first = args
            .first()
            .and_then(|s| s.to_str())
            .context("missing hook type after `wt hook`")?;
        let hook_type = parse_hook_type(first)?;

        let mut yes = false;
        let mut dry_run = false;
        let mut foreground: Option<bool> = None;
        let mut name_filters: Vec<String> = Vec::new();
        let mut explicit_vars: Vec<(String, String)> = Vec::new();
        let mut shorthand_vars: Vec<String> = Vec::new();
        let mut forwarded_args: Vec<String> = Vec::new();

        let mut literal_mode = false;
        let mut i = 1;
        while i < args.len() {
            let arg = args[i]
                .to_str()
                .with_context(|| format!("non-UTF-8 argument at position {i}"))?;
            if literal_mode {
                forwarded_args.push(arg.to_string());
                i += 1;
                continue;
            }
            match arg {
                "--" => {
                    literal_mode = true;
                    i += 1;
                    continue;
                }
                "--yes" | "-y" => {
                    yes = true;
                    i += 1;
                    continue;
                }
                "--dry-run" => {
                    dry_run = true;
                    i += 1;
                    continue;
                }
                "--foreground" => {
                    foreground = Some(true);
                    i += 1;
                    continue;
                }
                "--var" => {
                    let value = args
                        .get(i + 1)
                        .and_then(|s| s.to_str())
                        .context("--var requires KEY=VALUE")?;
                    push_var(&mut explicit_vars, value)?;
                    i += 2;
                    continue;
                }
                _ => {}
            }
            if let Some(rest) = arg.strip_prefix("--var=") {
                push_var(&mut explicit_vars, rest)?;
                i += 1;
                continue;
            }
            if let Some(rest) = arg.strip_prefix("--")
                && let Some((key, value)) = rest.split_once('=')
                && !key.is_empty()
            {
                // Unknown `--KEY=VALUE` — shorthand for smart routing.
                shorthand_vars.push(format!("{key}={value}"));
                i += 1;
                continue;
            }
            if arg.starts_with("--") {
                // Unknown bare flag (`--foo` with no `=`) — refuse rather
                // than absorb as a filter name, since that would silently
                // accept typos like `--dryrun` intended as `--dry-run`.
                bail!(
                    "unknown flag `{arg}` for `wt hook {first}` (use `--KEY=VALUE` for template \
                     variables, `--` to forward tokens to {{{{ args }}}})"
                );
            }
            // Bare positional → filter name.
            name_filters.push(arg.to_string());
            i += 1;
        }

        Ok(Self {
            hook_type,
            yes,
            dry_run,
            foreground,
            name_filters,
            explicit_vars,
            shorthand_vars,
            forwarded_args,
        })
    }
}

/// Parse `KEY=VALUE` and push `(canonical_key, value)` onto `out`, mirroring
/// the old `parse_key_val` behavior (hyphen → underscore canonicalization).
fn push_var(out: &mut Vec<(String, String)>, raw: &str) -> anyhow::Result<()> {
    let (key, val) = raw
        .split_once('=')
        .with_context(|| format!("--var expected KEY=VALUE, got `{raw}`"))?;
    if key.is_empty() {
        bail!("--var key cannot be empty");
    }
    out.push((key.replace('-', "_"), val.to_string()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<HookOptions> {
        let os: Vec<OsString> = args.iter().map(OsString::from).collect();
        HookOptions::parse(&os)
    }

    #[test]
    fn test_parse_minimal() {
        let opts = parse(&["pre-merge"]).unwrap();
        assert_eq!(opts.hook_type, HookType::PreMerge);
        assert!(!opts.yes);
        assert!(!opts.dry_run);
        assert_eq!(opts.foreground, None);
        assert!(opts.name_filters.is_empty());
        assert!(opts.explicit_vars.is_empty());
        assert!(opts.shorthand_vars.is_empty());
        assert!(opts.forwarded_args.is_empty());
    }

    #[test]
    fn test_parse_flags() {
        let opts = parse(&["post-start", "--yes", "--dry-run", "--foreground"]).unwrap();
        assert_eq!(opts.hook_type, HookType::PostStart);
        assert!(opts.yes);
        assert!(opts.dry_run);
        assert_eq!(opts.foreground, Some(true));

        // `-y` alias for `--yes`.
        let opts = parse(&["pre-merge", "-y"]).unwrap();
        assert!(opts.yes);
    }

    #[test]
    fn test_parse_name_filters() {
        let opts = parse(&["pre-merge", "test", "build"]).unwrap();
        assert_eq!(opts.name_filters, vec!["test", "build"]);
        assert!(opts.shorthand_vars.is_empty());

        // Source-prefix filters pass through as plain filter names.
        let opts = parse(&["pre-merge", "user:test", "project:"]).unwrap();
        assert_eq!(opts.name_filters, vec!["user:test", "project:"]);
    }

    #[test]
    fn test_parse_shorthand() {
        let opts = parse(&["pre-merge", "--branch=feature/x"]).unwrap();
        assert_eq!(opts.shorthand_vars, vec!["branch=feature/x"]);
        // Value with `=` inside (URL, etc.) preserves everything after the
        // first `=` as the value.
        let opts = parse(&["pre-start", "--url=http://host?a=1"]).unwrap();
        assert_eq!(opts.shorthand_vars, vec!["url=http://host?a=1"]);
        // Empty value.
        let opts = parse(&["pre-start", "--branch="]).unwrap();
        assert_eq!(opts.shorthand_vars, vec!["branch="]);
    }

    #[test]
    fn test_parse_explicit_var() {
        // Space form.
        let opts = parse(&["pre-merge", "--var", "branch=x"]).unwrap();
        assert_eq!(
            opts.explicit_vars,
            vec![("branch".to_string(), "x".to_string())]
        );
        assert!(opts.shorthand_vars.is_empty());

        // `=` form.
        let opts = parse(&["pre-merge", "--var=branch=x"]).unwrap();
        assert_eq!(
            opts.explicit_vars,
            vec![("branch".to_string(), "x".to_string())]
        );

        // Hyphen canonicalization to underscore.
        let opts = parse(&["pre-merge", "--var", "my-key=val"]).unwrap();
        assert_eq!(
            opts.explicit_vars,
            vec![("my_key".to_string(), "val".to_string())]
        );

        // Multiple `--var`.
        let opts = parse(&["pre-merge", "--var", "a=1", "--var", "b=2"]).unwrap();
        assert_eq!(
            opts.explicit_vars,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn test_parse_literal_forward_escape() {
        // Tokens after `--` forward verbatim, even if they look like flags
        // or would have bound before `--`.
        let opts = parse(&[
            "pre-merge",
            "--branch=x",
            "--",
            "--fast",
            "--branch=y",
            "extra",
        ])
        .unwrap();
        assert_eq!(opts.shorthand_vars, vec!["branch=x"]);
        assert_eq!(opts.forwarded_args, vec!["--fast", "--branch=y", "extra"]);

        // Trailing `--` with nothing after — consumed silently.
        let opts = parse(&["pre-merge", "--"]).unwrap();
        assert!(opts.forwarded_args.is_empty());

        // `--` preserves filter names parsed before it.
        let opts = parse(&["pre-merge", "test", "--", "extra"]).unwrap();
        assert_eq!(opts.name_filters, vec!["test"]);
        assert_eq!(opts.forwarded_args, vec!["extra"]);
    }

    #[test]
    fn test_parse_mixed() {
        // Every token type in one invocation.
        let opts = parse(&[
            "post-merge",
            "--yes",
            "test",
            "--branch=x",
            "--var",
            "override=1",
            "--dry-run",
            "--",
            "--fast",
        ])
        .unwrap();
        assert_eq!(opts.hook_type, HookType::PostMerge);
        assert!(opts.yes);
        assert!(opts.dry_run);
        assert_eq!(opts.name_filters, vec!["test"]);
        assert_eq!(opts.shorthand_vars, vec!["branch=x"]);
        assert_eq!(
            opts.explicit_vars,
            vec![("override".to_string(), "1".to_string())]
        );
        assert_eq!(opts.forwarded_args, vec!["--fast"]);
    }

    #[test]
    fn test_parse_hook_type_aliases() {
        // `post-create` is the deprecated alias for `pre-start`.
        let opts = parse(&["post-create"]).unwrap();
        assert_eq!(opts.hook_type, HookType::PreStart);
    }

    #[test]
    fn test_parse_errors() {
        // Missing hook type.
        assert!(HookOptions::parse(&[]).is_err());

        // Unknown hook type; typo that matches a canonical name produces a suggestion.
        let err = parse(&["pre-mrege"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("pre-merge"),
            "expected did-you-mean suggestion, got: {msg}"
        );

        // Completely unknown hook type lists all valid names.
        let err = parse(&["zzz"]).unwrap_err();
        assert!(err.to_string().contains("expected one of"));

        // `--var` missing value.
        let err = parse(&["pre-merge", "--var"]).unwrap_err();
        assert!(err.to_string().contains("--var"));

        // `--var` with empty key.
        let err = parse(&["pre-merge", "--var", "=value"]).unwrap_err();
        assert!(err.to_string().contains("empty"));

        // Unknown bare flag rejected (no silent filter-name fallback).
        let err = parse(&["pre-merge", "--dryrun"]).unwrap_err();
        assert!(err.to_string().contains("unknown flag"));
    }
}
