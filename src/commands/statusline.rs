//! Statusline output for shell prompts and editors.
//!
//! Outputs a single-line status for the current worktree:
//! `branch  status  ±working  commits  upstream  [ci]`
//!
//! This command reuses the data collection infrastructure from `wt list`,
//! avoiding duplication of git operations.

use std::collections::HashMap;
use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::{Component, Path};

use dunce::canonicalize;

use ansi_str::AnsiStr;
use anyhow::{Context, Result};
use worktrunk::git::Repository;
use worktrunk::styling::{
    fix_dim_after_color_reset, terminal_width_for_statusline, truncate_visible,
};

use super::list::{self, CollectOptions, StatuslineSegment, json_output};
use crate::cli::OutputFormat;

/// Claude Code context parsed from stdin JSON
struct ClaudeCodeContext {
    /// Working directory from `.workspace.current_dir`
    current_dir: String,
    /// Model name from `.model.display_name`
    model_name: Option<String>,
    /// Context window usage percentage from `.context_window.used_percentage`
    context_used_percentage: Option<f64>,
    /// Rate-limit window readings from `.rate_limits.{five_hour,seven_day}`.
    /// Empty when absent (non-subscribers, or before the first API response).
    rate_limits: Vec<RateLimitReading>,
}

/// A single rate-limit window reading parsed from Claude Code's JSON.
#[derive(Clone, Debug)]
struct RateLimitReading {
    /// Window key in Claude Code's JSON: `five_hour` or `seven_day`.
    name: &'static str,
    /// 0–100 from `.rate_limits.<window>.used_percentage`.
    used_percentage: f64,
    /// Unix epoch seconds from `.rate_limits.<window>.resets_at`.
    resets_at: i64,
    /// Window length in seconds (5h or 7d).
    window_secs: i64,
    /// Prior parameters for this window.
    priors: &'static WindowPriors,
}

impl ClaudeCodeContext {
    /// Parse Claude Code context from a JSON string.
    /// Returns None if not valid JSON or missing required fields.
    fn parse(input: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(input).ok()?;

        // current_dir is required - if missing, treat as invalid JSON
        let current_dir = v
            .pointer("/workspace/current_dir")
            .and_then(|v| v.as_str())?
            .to_string();

        let model_name = v
            .pointer("/model/display_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let context_used_percentage = v
            .pointer("/context_window/used_percentage")
            .and_then(|v| v.as_f64());

        let rate_limits = parse_rate_limits(&v);

        Some(Self {
            current_dir,
            model_name,
            context_used_percentage,
            rate_limits,
        })
    }

    /// Try to read and parse Claude Code context from stdin.
    /// Returns None if stdin is a terminal or not valid JSON.
    fn from_stdin() -> Option<Self> {
        if io::stdin().is_terminal() {
            return None;
        }

        let mut input = String::new();
        io::stdin().read_to_string(&mut input).ok()?;
        Self::parse(&input)
    }
}

/// Format a directory path in fish-style (abbreviated parent directories).
///
/// Examples:
/// - `/home/user/workspace/project` -> `~/w/project`
/// - `/home/user` -> `~`
/// - `/tmp/test` -> `/t/test`
fn format_directory_fish_style(path: &Path) -> String {
    // Replace home directory prefix with ~
    let (suffix, tilde_prefix) = worktrunk::path::home_dir()
        .and_then(|home| path.strip_prefix(&home).ok().map(|s| (s, true)))
        .unwrap_or((path, false));

    // Collect normal components (skip RootDir, CurDir, etc.)
    let components: Vec<_> = suffix
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect();

    // Build result: ~/a/b/last or /a/b/last
    let abbreviated = components
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if i == components.len() - 1 {
                s.to_string() // Keep last component full
            } else {
                s.chars().next().map(String::from).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>();

    match (tilde_prefix, abbreviated.is_empty()) {
        (true, true) => "~".to_string(),
        (true, false) => format!("~/{}", abbreviated.join("/")),
        (false, _) if path.is_absolute() => format!("/{}", abbreviated.join("/")),
        (false, _) => abbreviated.join("/"),
    }
}

/// Priority for directory segment (Claude Code only).
/// Highest priority - directory context is essential.
const PRIORITY_DIRECTORY: u8 = 0;

/// Priority for model name segment (Claude Code only).
/// Same as Branch - model identity is important.
const PRIORITY_MODEL: u8 = 1;

/// Priority for context gauge segment (Claude Code only).
/// Lower priority than model (higher number = dropped first when truncating).
const PRIORITY_CONTEXT: u8 = 2;

/// Priority for the rate-limit segment (Claude Code only).
/// Lower priority than context — drops first when the line is tight.
const PRIORITY_RATE_LIMITS: u8 = 3;

// ---------------------------------------------------------------------------
// Rate-limit pace prediction
//
// Claude Code exposes two rolling rate-limit windows (5-hour and weekly) with
// `used_percentage` and `resets_at`. We want to surface a one-line statusline
// warning when the user is likely to actually hit the cap — *not* whenever
// they're momentarily ahead of pace, which is normal early-window noise.
//
// ## The question
//
// Given fraction-of-limit-used `u` and fraction-of-window-elapsed `t`, what
// is the probability that final usage `C(1) ≥ 1`?
//
// The naive answer is the linear projection `u/t`: alarm when `u > t`. That
// triggers wildly early in a window — 5% used at 3% elapsed projects to
// 167%, but you've barely sampled anything. The fix isn't a different
// estimator; it's wrapping uncertainty around the projection so a thin slice
// of data carries less weight than a fat one.
//
// ## The model
//
// Cumulative consumption as Brownian-with-drift:
//
//     C(t) = λ·t + σ·W(t)
//
// `λ` is the user's long-run rate (λ=1 ⇒ "they'll exactly fill the window"),
// `σ` is within-window burstiness, `W(t)` is standard Brownian motion.
//
// `λ` is unknown. Prior: `λ ~ N(m0, s0²)`. With `m0 ≈ 0.8` the prior says
// "most windows finish under the cap" — that's what suppresses the
// hair-trigger on early data.
//
// Observing `C(t) = u` updates the posterior on `λ` by Bayes (Gaussian on
// Gaussian = precision-weighted blend of prior and the observed rate `u/t`).
// The predictive distribution for `C(1)` is itself Gaussian, and the answer
// is its upper tail at 1.0.
//
// ## σ for the week
//
// If consumption were a sum of independent bursts, `σ_7d` would be
// `σ_5h / √(168/5) ≈ σ_5h / 6` by CLT. It isn't — busy days cluster, busy
// weeks cluster — so `σ_7d` is only modestly below `σ_5h`. The values below
// are calibrated by feel; with real `(u, t, resets_at)` traces logged, fit
// them from data and delete this paragraph.
// ---------------------------------------------------------------------------

/// Prior parameters for one rate-limit window's pace prediction.
#[derive(Debug)]
struct WindowPriors {
    /// Within-window volatility — residual scatter of `C(t)` around `λ·t`.
    sigma: f64,
    /// Prior mean rate. `m0 < 1` ⇒ we expect most windows to finish under.
    m0: f64,
    /// Prior std on rate. Bigger = weaker prior = data takes over sooner.
    s0: f64,
}

const FIVE_HOUR_PRIORS: WindowPriors = WindowPriors {
    sigma: 0.35,
    m0: 0.8,
    s0: 0.5,
};
const SEVEN_DAY_PRIORS: WindowPriors = WindowPriors {
    sigma: 0.30,
    m0: 0.8,
    s0: 0.5,
};

const FIVE_HOUR_SECS: i64 = 5 * 3600;
const SEVEN_DAY_SECS: i64 = 7 * 86400;

/// Show the rate-limit segment when P(over) crosses this threshold.
const RATE_LIMIT_P_THRESHOLD: f64 = 0.50;

/// Above this usage (in percent), display used % instead of pace.
///
/// Close to the cap, proximity beats velocity: pace answers "will the limit
/// be hit?", but once usage is nearly there that's all but settled, and "how
/// much is left?" becomes the actionable number.
const RATE_LIMIT_NEAR_CAP_PCT: f64 = 90.0;

/// Extract any present rate-limit windows from the Claude Code JSON.
///
/// Both windows are independently optional: subscribers see `rate_limits`
/// only after the first API response, and each window may be missing.
fn parse_rate_limits(v: &serde_json::Value) -> Vec<RateLimitReading> {
    // Ground truth for debugging window selection: which windows Claude Code
    // actually sent, before any parsing or clamping. The explicit absent case
    // distinguishes "no rate_limits key" from "stdin context never parsed"
    // (which logs nothing at all).
    match v.get("rate_limits") {
        Some(rl) => log::debug!("rate_limits input: {rl}"),
        None => log::debug!("rate_limits input: absent"),
    }
    let mut out = Vec::new();
    for (key, window_secs, priors) in [
        ("five_hour", FIVE_HOUR_SECS, &FIVE_HOUR_PRIORS),
        ("seven_day", SEVEN_DAY_SECS, &SEVEN_DAY_PRIORS),
    ] {
        let used = v
            .pointer(&format!("/rate_limits/{key}/used_percentage"))
            .and_then(|x| x.as_f64());
        let resets = v
            .pointer(&format!("/rate_limits/{key}/resets_at"))
            .and_then(|x| x.as_i64());
        if let (Some(u), Some(r)) = (used, resets) {
            out.push(RateLimitReading {
                name: key,
                used_percentage: u,
                resets_at: r,
                window_secs,
                priors,
            });
        }
    }
    out
}

/// Standard normal CDF via Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7).
fn standard_normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Error function via Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7).
///
/// For `x ≥ 0`, with `t = 1 / (1 + p·x)`:
///
/// ```text
///     erf(x) ≈ 1 − (a₁·t + a₂·t² + a₃·t³ + a₄·t⁴ + a₅·t⁵) · exp(−x²)
/// ```
///
/// `p` and the `aᵢ` are numerically fit — they don't come from a closed
/// form. For `x < 0` we use the odd symmetry `erf(−x) = −erf(x)`.
fn erf(x: f64) -> f64 {
    const P: f64 = 0.3275911;
    const COEFFS: [f64; 5] = [
        0.254829592,
        -0.284496736,
        1.421413741,
        -1.453152027,
        1.061405429,
    ];
    let sign = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + P * x);
    // Horner's method: build `a₁ + a₂·t + … + a₅·t⁴` inside-out, then one
    // more multiply by `t` to get the `a₁·t + … + a₅·t⁵` we actually want.
    let poly = COEFFS.iter().rev().fold(0.0, |acc, &c| acc * t + c) * t;
    sign * (1.0 - poly * (-x * x).exp())
}

/// Probability that final usage hits or exceeds the limit (`C(1) ≥ 1.0`),
/// given fraction-of-limit-used `u` and fraction-of-window-elapsed `t`.
///
/// See the section header above [`WindowPriors`] for the model. In short:
/// posterior on the rate via Bayes, predictive on final usage is Gaussian,
/// answer is its upper tail at 1.0.
fn p_over(u: f64, t: f64, p: &WindowPriors) -> f64 {
    // End of window: deterministic — we either crossed or we didn't.
    if t >= 1.0 {
        return if u >= 1.0 { 1.0 } else { 0.0 };
    }
    let (mean, var) = if t <= 0.0 {
        // No data yet. Predictive collapses to the prior on `λ` plus one
        // window's worth of process noise on top.
        (p.m0, p.s0 * p.s0 + p.sigma * p.sigma)
    } else {
        // --- Bayes update on `λ` from one observation `C(t) = u` ---
        //
        // Both precisions live on the rate `λ`. The data precision is `t/σ²`
        // because observing `C(t) = u` is equivalent to observing the rate
        // `u/t` with variance `σ²/t`. The posterior is Gaussian with:
        //
        //     post_prec = prior_prec + data_prec
        //     post_mean = (prior_prec · m₀ + data_prec · (u/t)) / post_prec
        //
        // The `data_prec · (u/t)` term is rewritten as `u / σ²` so both
        // terms share a denominator and the multiplication is cheaper.
        let sigma2 = p.sigma * p.sigma;
        let prior_prec = 1.0 / (p.s0 * p.s0);
        let data_prec = t / sigma2;
        let post_var = 1.0 / (prior_prec + data_prec);
        let post_mean = post_var * (p.m0 * prior_prec + u / sigma2);

        // --- Predictive for `C(1) = u + (rate × time remaining) + noise` ---
        //
        // Mean: what we've used, plus the posterior rate projected over the
        // remaining `(1 - t)` of the window.
        //
        // Variance has two pieces: uncertainty in the rate estimate
        // propagated over `(1 - t)` (squared because it scales the deterministic
        // drift term), plus accumulated process noise `σ² · (1 - t)` over the
        // remaining interval (linear because Brownian variance grows with time).
        let mean = u + post_mean * (1.0 - t);
        let var = post_var * (1.0 - t).powi(2) + sigma2 * (1.0 - t);
        (mean, var)
    };
    // Upper tail of the Gaussian predictive at 1.0: `P(C(1) ≥ 1)`.
    standard_normal_cdf((mean - 1.0) / var.sqrt())
}

/// 12-hour vs 24-hour clock preference. Carried as a parameter so the
/// formatters stay pure and unit-testable; the env-driven detection lives in
/// [`detect_clock_format`] at the call-site boundary.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ClockFormat {
    /// `3pm`, `5:45pm` — US/Philippine convention. `:00` minutes elided.
    H12,
    /// `15:00`, `15:45` — ISO/European convention. Minutes always shown,
    /// hour zero-padded.
    H24,
}

/// Classify a `LC_*`/`LANG` value into 12h or 24h. Pure so it can be tested
/// without env mutation; strips any `.encoding` or `@modifier` suffix.
///
/// 12h: the small set of locales where 12h is the everyday written form —
/// US English (`en_US`), Philippine English (`en_PH`), Canadian English
/// (`en_CA`). Everything else, including the POSIX `C` and `POSIX` locales
/// and the empty string (unset), is 24h.
fn classify_locale(locale: &str) -> ClockFormat {
    // `en_US.UTF-8@posix` → `en_US`; `C` → `C`; `` → ``.
    let lang = locale.split(['.', '@']).next().unwrap_or("");
    if matches!(lang, "en_US" | "en_PH" | "en_CA") {
        ClockFormat::H12
    } else {
        ClockFormat::H24
    }
}

/// Detect the user's clock preference from environment, in POSIX precedence:
/// `LC_ALL` overrides `LC_TIME` overrides `LANG`. Reading env directly
/// (instead of going through `sys_locale` or `CFLocale`) honors the shell
/// config the user actually set — for a CLI, the env IS the configuration.
fn detect_clock_format() -> ClockFormat {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_TIME"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    classify_locale(&locale)
}

/// Format a `DateTime` as a clock time only.
///
/// 12h: `3pm`, `5:45pm` (`:00` elided — chrono can't do that conditionally in
/// one format string). 24h: `15:00`, `09:45` (always show minutes,
/// zero-padded hour).
fn format_clock<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>, fmt: ClockFormat) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Timelike as _;
    match fmt {
        ClockFormat::H12 if dt.minute() == 0 => dt.format("%-I%P").to_string(),
        ClockFormat::H12 => dt.format("%-I:%M%P").to_string(),
        ClockFormat::H24 => dt.format("%H:%M").to_string(),
    }
}

/// Format the rate-limit window's start and end as a short label that
/// goes inside the parenthetical: `10am–3pm` (12h) / `10:00–15:00` (24h) for
/// the 5-hour window, `Mon–Mon 3pm` / `Mon–Mon 15:00` for the 7-day window.
///
/// The 5h variant uses clock times on both endpoints. The longer window
/// uses the weekday on both ends (they're equal for a true 7-day rolling
/// window) and the clock time only on the end, which is the reset.
///
/// Weekday is English (`%a`) regardless of locale — chrono's localized
/// weekday names require the `unstable-locales` feature; clock format is the
/// part of the follow-up that mattered.
///
/// Generic over `TimeZone` so unit tests can pass `&chrono::Utc` directly
/// without mutating the process-global `TZ`; production passes
/// `&chrono::Local`.
fn format_window_bounds<Tz: chrono::TimeZone>(
    resets_at: i64,
    window_secs: i64,
    tz: &Tz,
    fmt: ClockFormat,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let to_tz = |unix| {
        chrono::DateTime::from_timestamp(unix, 0)
            .map(|t| t.with_timezone(tz))
            .unwrap_or_else(|| chrono::Utc::now().with_timezone(tz))
    };
    let start = to_tz(resets_at - window_secs);
    let end = to_tz(resets_at);
    // En-dash (U+2013) is the typographically correct separator for a
    // numeric range and stays visually distinct from the ASCII hyphens
    // used elsewhere in the statusline (e.g. `-3` for line deletions).
    if window_secs <= 12 * 3600 {
        format!("{}–{}", format_clock(&start, fmt), format_clock(&end, fmt))
    } else {
        format!(
            "{}–{} {}",
            start.format("%a"),
            end.format("%a"),
            format_clock(&end, fmt)
        )
    }
}

/// Pick the window most likely to be hit; return `None` to hide the segment.
///
/// Both windows can simultaneously be over the show threshold. We surface
/// only the worse-projected one because a single-segment statusline is
/// already crowded — the user gets the binding constraint, not a list.
///
/// Split out from rendering so the selection logic stays testable without
/// dragging in `chrono::Local`.
fn select_binding_window(
    readings: &[RateLimitReading],
    now_unix: i64,
) -> Option<&RateLimitReading> {
    readings
        .iter()
        .filter_map(|r| {
            let u = (r.used_percentage / 100.0).clamp(0.0, 1.0);
            let elapsed = (now_unix - (r.resets_at - r.window_secs)) as f64 / r.window_secs as f64;
            let t = elapsed.clamp(0.0, 1.0);
            let p = p_over(u, t, r.priors);
            log::debug!(
                "rate-limit {} window: used={:.1}% elapsed={:.1}% pace={:.2}× P(over)={p:.3} (show threshold {RATE_LIMIT_P_THRESHOLD})",
                r.name,
                u * 100.0,
                t * 100.0,
                u / t.max(0.001),
            );
            (p >= RATE_LIMIT_P_THRESHOLD).then_some((p, r))
        })
        .max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, r)| r)
}

/// Build the rate-limit segment, or `None` to hide.
///
/// Shape: `<pace>×(<window_bounds>)` — e.g. `1.4×(10am–3pm)` or
/// `1.9×(Mon–Mon 3pm)`. `pace` is the **naive `u/t` ratio**: what the
/// user has actually consumed per unit elapsed window. `1.0×` is on pace
/// to exactly fill the window; `>1.0×` is over-pace. The Bayesian
/// posterior `m₁` is only used by [`p_over`] to decide whether to show —
/// for the *displayed* number, the raw measurement is more honest, more
/// transparent, and tracks bursts naturally.
///
/// Above [`RATE_LIMIT_NEAR_CAP_PCT`] used, the shape becomes
/// `<used>%(<window_bounds>)` — e.g. `93%(10am–3pm)` — showing how close
/// the limit is rather than how fast it's being approached.
fn format_rate_limit_segment(readings: &[RateLimitReading], now_unix: i64) -> Option<String> {
    let r = select_binding_window(readings, now_unix)?;
    let u = r.used_percentage / 100.0;
    // `t` is the elapsed fraction of the window. Floor avoids division by
    // zero on absurd inputs; in practice `select_binding_window` only
    // returns a reading whose `P(over) ≥ 0.5`, which requires `t > 0`.
    let elapsed = (now_unix - (r.resets_at - r.window_secs)) as f64 / r.window_secs as f64;
    let t = elapsed.clamp(0.001, 1.0);
    let reading = if r.used_percentage > RATE_LIMIT_NEAR_CAP_PCT {
        format!("{:.0}%", r.used_percentage)
    } else {
        format!("{:.1}×", u / t)
    };
    let bounds = format_window_bounds(
        r.resets_at,
        r.window_secs,
        &chrono::Local,
        detect_clock_format(),
    );
    // The whole segment is wrapped in yellow because its appearance *is*
    // the warning. Unlike informational segments where color picks out one
    // sub-glyph (`@+1` green, `?` cyan), here the entire string is the
    // "you should look at this" signal.
    Some(color_print::cformat!("<yellow>{reading}({bounds})</>"))
}

/// Format context usage as a moon phase gauge.
///
/// Uses moon phase emoji to show fill level (waning - gets darker as context fills).
/// Thresholds use exponential halving where each range is half the previous.
/// Formula: 5 buckets with ratio 16:8:4:2:1, normalized to 100% (sum = 31).
/// - 🌕 (0-51%) - full moon (plenty of room) - 16/31 ≈ 52%
/// - 🌔 (52-77%) - waning gibbous - 8/31 ≈ 26%
/// - 🌓 (78-90%) - last quarter - 4/31 ≈ 13%
/// - 🌒 (91-97%) - waning crescent - 2/31 ≈ 7%
/// - 🌑 (98-100%) - new moon (nearly full, warning) - 1/31 ≈ 3%
fn format_context_gauge(percentage: f64) -> String {
    // Clamp to valid range to handle edge cases (negative or >100%)
    let clamped = percentage.clamp(0.0, 100.0);
    let symbol = match clamped as u32 {
        0..=51 => '🌕',
        52..=77 => '🌔',
        78..=90 => '🌓',
        91..=97 => '🌒',
        _ => '🌑',
    };
    // Display the original percentage (not clamped) for transparency.
    // A space separates the moon emoji from the percent: unlike the ASCII
    // prefix-char segments (`@+1`, `↓1`, `?^|`) that run flush, emoji glyphs
    // are double-width and bleed into the next cell on most terminals, so they
    // need a trailing space to keep from colliding with the digits.
    format!("{symbol} {:.0}%", percentage)
}

/// Run the statusline command.
///
/// Output uses `println!` for raw stdout (bypasses anstream color detection).
/// Shell prompts (PS1) and Claude Code always expect ANSI codes.
pub fn run(format: OutputFormat) -> Result<()> {
    // Statusline runs on every prompt redraw — deprecation warnings on stderr
    // would appear above each prompt.
    worktrunk::config::suppress_warnings();

    // JSON format: output current worktree as JSON
    if matches!(format, OutputFormat::Json) {
        return run_json();
    }

    let claude_code = matches!(format, OutputFormat::ClaudeCode);

    // Get context - either from stdin (claude-code mode) or current directory
    let (cwd, model_name, context_used_percentage, rate_limits) = if claude_code {
        let ctx = ClaudeCodeContext::from_stdin();
        let current_dir = ctx
            .as_ref()
            .map(|c| c.current_dir.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_default().display().to_string());
        let model = ctx.as_ref().and_then(|c| c.model_name.clone());
        let context_pct = ctx.as_ref().and_then(|c| c.context_used_percentage);
        let limits = ctx.map(|c| c.rate_limits).unwrap_or_default();
        (
            Path::new(&current_dir).to_path_buf(),
            model,
            context_pct,
            limits,
        )
    } else {
        (
            env::current_dir().context("Failed to get current directory")?,
            None,
            None,
            Vec::new(),
        )
    };

    // Build segments with priorities
    let mut segments: Vec<StatuslineSegment> = Vec::new();

    // Directory (claude-code mode only) - priority 0
    let dir_str = if claude_code {
        let formatted = format_directory_fish_style(&cwd);
        // Only push non-empty directory segments (empty can happen if cwd is ".")
        if !formatted.is_empty() {
            segments.push(StatuslineSegment::new(
                formatted.clone(),
                PRIORITY_DIRECTORY,
            ));
        }
        Some(formatted)
    } else {
        None
    };

    // Git status segments (skip links in claude-code mode - OSC 8 not supported)
    if let Ok(repo) = Repository::current()
        && repo.worktree_at(&cwd).git_dir().is_ok()
    {
        let git_segments = git_status_segments(&repo, &cwd, !claude_code)?;

        // In claude-code mode, skip branch segment if directory matches worktrunk template
        let git_segments = if let Some(ref dir) = dir_str {
            filter_redundant_branch(git_segments, dir)
        } else {
            git_segments
        };

        segments.extend(git_segments);
    }

    // Model name (claude-code mode only) - priority 1 (same as Branch).
    // The two-space inter-segment separator handles spacing on its own; no
    // decorative prefix needed.
    if let Some(model) = model_name {
        segments.push(StatuslineSegment::new(model, PRIORITY_MODEL));
    }

    // Context gauge (claude-code mode only) - priority 2 (placed after model)
    if let Some(pct) = context_used_percentage {
        segments.push(StatuslineSegment::new(
            format_context_gauge(pct),
            PRIORITY_CONTEXT,
        ));
    }

    // Rate-limit segment (claude-code mode only) - priority 3, shown only
    // when the binding window has a real chance of going over.
    // `epoch_now()` honours `WORKTRUNK_TEST_EPOCH` so snapshot tests can pin
    // "now" alongside `TZ=UTC` for deterministic output.
    if !rate_limits.is_empty()
        && let Some(content) =
            format_rate_limit_segment(&rate_limits, worktrunk::utils::epoch_now() as i64)
    {
        segments.push(StatuslineSegment::new(content, PRIORITY_RATE_LIMITS));
    }

    if segments.is_empty() {
        return Ok(());
    }

    // Fit segments to terminal width using priority-based dropping; with no
    // detectable width (even via the parent-TTY walk), render everything
    let max_width = terminal_width_for_statusline().unwrap_or(usize::MAX);
    // Reserve 1 char for leading space (ellipsis handled by truncate_visible fallback)
    let content_budget = max_width.saturating_sub(1);
    let fitted_segments = StatuslineSegment::fit_to_width(segments, content_budget);

    // Join and apply final truncation as fallback
    let output = StatuslineSegment::join(&fitted_segments);

    let reset = anstyle::Reset;
    let output = fix_dim_after_color_reset(&output);
    let output = truncate_visible(&format!("{reset} {output}"), max_width);

    println!("{}", output);

    Ok(())
}

/// Run statusline with JSON output format.
///
/// Outputs the current worktree as JSON, using the same structure as `wt list --format=json`.
fn run_json() -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current directory")?;

    let repo = Repository::current().context("Not in a git repository")?;

    // Verify we're in a worktree
    if repo.worktree_at(&cwd).git_dir().is_err() {
        // Not in a worktree - return empty array (consistent with wt list)
        println!("[]");
        return Ok(());
    }

    // Get current worktree info
    // Use git rev-parse --show-toplevel (via current_worktree().root()) to correctly identify
    // the worktree containing cwd, rather than prefix matching which fails for nested worktrees.
    let worktrees = repo.list_worktrees()?;
    let worktree_root = repo.current_worktree().root()?;
    let current_worktree = worktrees.iter().find(|wt| {
        canonicalize(&wt.path)
            .map(|p| p == worktree_root)
            .unwrap_or(false)
    });

    let Some(wt) = current_worktree else {
        println!("[]");
        return Ok(());
    };

    // Determine if this is the primary worktree
    let is_home = repo
        .primary_worktree()
        .ok()
        .flatten()
        .is_some_and(|p| wt.path == p);

    // Build item with identity fields
    let mut item = list::build_worktree_item(wt, is_home, true, false);

    // Load URL template from project config (if configured)
    let url_template = repo.url_template();

    // Build collect options with URL template (compute everything for complete data)
    let options = CollectOptions {
        url_template,
        // Match `wt list --full`: include untracked files in the working
        // diff (`HEAD±`) so the segment counts the same lines `wt step
        // diff` would show. Statusline already runs the full task set;
        // this keeps the diff number consistent with the rest of the
        // statusline data.
        include_untracked_in_working_diff: true,
        ..Default::default()
    };

    // Populate computed fields (parallel git operations)
    list::populate_item(&repo, &mut item, options)?;

    // Convert to JSON format — single-branch lookup (not all_vars_entries)
    let mut all_vars = HashMap::new();
    if let Some(branch) = &item.branch {
        let entries = repo.vars_entries(branch);
        if !entries.is_empty() {
            all_vars.insert(branch.clone(), entries);
        }
    }
    // No custom columns: the statusline path never expands `[list.custom-columns]`
    // (prompt hot path; its compact format has no column grid).
    let json_item = json_output::JsonItem::from_list_item(
        &item,
        &mut all_vars,
        repo.repo_web_url().as_deref(),
        &[],
    );

    // Output as JSON array (consistent with wt list --format=json)
    let output = serde_json::to_string_pretty(&[json_item])?;
    println!("{output}");

    Ok(())
}

/// Filter out branch segment if directory already shows it via worktrunk template.
fn filter_redundant_branch(segments: Vec<StatuslineSegment>, dir: &str) -> Vec<StatuslineSegment> {
    use super::list::columns::ColumnKind;

    // Find the branch segment by its column kind (not priority, which could be shared)
    if let Some(branch_seg) = segments.iter().find(|s| s.kind == Some(ColumnKind::Branch)) {
        // Strip ANSI codes in case branch becomes styled in future
        let raw_branch = branch_seg.content.ansi_strip();
        // Normalize branch name for comparison (slashes become dashes in paths)
        let normalized_branch = worktrunk::config::sanitize_branch_name(&raw_branch);
        let pattern = format!(".{normalized_branch}");

        if dir.ends_with(&pattern) {
            // Directory already shows branch via worktrunk template, skip branch segment
            return segments
                .into_iter()
                .filter(|s| s.kind != Some(ColumnKind::Branch))
                .collect();
        }
    }

    segments
}

/// Get git status as prioritized segments for the current worktree.
///
/// When `include_links` is true, CI status includes clickable OSC 8 hyperlinks.
fn git_status_segments(
    repo: &Repository,
    cwd: &Path,
    include_links: bool,
) -> Result<Vec<StatuslineSegment>> {
    use super::list::columns::ColumnKind;

    // Get current worktree info
    // Use git rev-parse --show-toplevel (via worktree_at().root()) to correctly identify
    // the worktree containing cwd, rather than prefix matching which fails for nested worktrees.
    let worktrees = repo.list_worktrees()?;
    let worktree_root = repo.worktree_at(cwd).root()?;
    let current_worktree = worktrees.iter().find(|wt| {
        canonicalize(&wt.path)
            .map(|p| p == worktree_root)
            .unwrap_or(false)
    });

    let Some(wt) = current_worktree else {
        // Not in a worktree - just show branch name as a segment
        if let Ok(Some(branch)) = repo.current_worktree().branch() {
            return Ok(vec![StatuslineSegment::from_column(
                branch.to_string(),
                ColumnKind::Branch,
            )]);
        }
        return Ok(vec![]);
    };

    // If we can't determine the default branch, just show current branch
    if repo.default_branch().is_none() {
        return Ok(vec![StatuslineSegment::from_column(
            wt.branch.as_deref().unwrap_or("HEAD").to_string(),
            ColumnKind::Branch,
        )]);
    }

    // Determine if this is the primary worktree
    // - Normal repos: the main worktree (repo root)
    // - Bare repos: the default branch's worktree
    let is_home = repo
        .primary_worktree()
        .ok()
        .flatten()
        .is_some_and(|p| wt.path == p);

    // Build item with identity fields
    let mut item = list::build_worktree_item(wt, is_home, true, false);

    // Load URL template from project config (if configured)
    let url_template = repo.url_template();

    // Build collect options with URL template
    let options = CollectOptions {
        url_template,
        // Match `wt list --full`: include untracked files in the working
        // diff (`HEAD±`) so the segment counts the same lines `wt step
        // diff` would show. Statusline already runs the full task set;
        // this keeps the diff number consistent with the rest of the
        // statusline data.
        include_untracked_in_working_diff: true,
        ..Default::default()
    };

    // Populate computed fields (parallel git operations)
    // Compute everything (same as --full) for complete status symbols
    list::populate_item(repo, &mut item, options)?;

    // Get prioritized segments
    let segments = item.format_statusline_segments(include_links);

    if segments.is_empty() {
        // Fallback: just show branch name
        Ok(vec![StatuslineSegment::from_column(
            wt.branch.as_deref().unwrap_or("HEAD").to_string(),
            ColumnKind::Branch,
        )])
    } else {
        Ok(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_directory_fish_style() {
        // Test absolute paths (Unix-style paths only meaningful on Unix)
        #[cfg(unix)]
        {
            assert_eq!(
                format_directory_fish_style(Path::new("/tmp/test")),
                "/t/test"
            );
            assert_eq!(format_directory_fish_style(Path::new("/")), "/");
            assert_eq!(
                format_directory_fish_style(Path::new("/var/log/app")),
                "/v/l/app"
            );
        }

        // Test with actual HOME (if set)
        if let Ok(home) = env::var("HOME") {
            // Basic home substitution
            let test_path = format!("{home}/workspace/project");
            let result = format_directory_fish_style(Path::new(&test_path));
            assert!(result.starts_with("~/"), "Expected ~ prefix, got: {result}");
            assert!(
                result.ends_with("/project"),
                "Expected /project suffix, got: {result}"
            );

            // Exact HOME path should become just ~
            assert_eq!(format_directory_fish_style(Path::new(&home)), "~");

            // Path that shares HOME as string prefix but not as path component
            // e.g., /home/user vs /home/usered/nested
            let path_outside_home = format!("{home}ed/nested");
            let result = format_directory_fish_style(Path::new(&path_outside_home));
            assert!(
                !result.starts_with("~"),
                "Path sharing HOME string prefix should not use ~: {result}"
            );
        }
    }

    #[test]
    fn test_claude_code_context_parse_full() {
        // Full Claude Code context JSON (as documented)
        let json = r#"{
            "hook_event_name": "Status",
            "session_id": "abc123",
            "cwd": "/current/working/directory",
            "model": {
                "id": "claude-opus-4-1",
                "display_name": "Opus"
            },
            "workspace": {
                "current_dir": "/home/user/project",
                "project_dir": "/home/user/project"
            },
            "version": "1.0.80"
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/home/user/project");
        assert_eq!(ctx.model_name, Some("Opus".to_string()));
    }

    #[test]
    fn test_claude_code_context_parse_minimal() {
        // Minimal JSON with just the fields we need
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Haiku"}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, Some("Haiku".to_string()));
    }

    #[test]
    fn test_claude_code_context_parse_missing_model() {
        // Model is optional
        let json = r#"{"workspace": {"current_dir": "/tmp/test"}}"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, None);
    }

    #[test]
    fn test_claude_code_context_parse_missing_workspace() {
        // Missing current_dir makes the JSON invalid - returns None
        let json = r#"{"model": {"display_name": "Sonnet"}}"#;

        assert!(
            ClaudeCodeContext::parse(json).is_none(),
            "Missing current_dir should return None"
        );
    }

    #[test]
    fn test_claude_code_context_parse_empty() {
        assert!(ClaudeCodeContext::parse("").is_none());
    }

    #[test]
    fn test_claude_code_context_parse_invalid_json() {
        assert!(ClaudeCodeContext::parse("not json").is_none());
        assert!(ClaudeCodeContext::parse("{invalid}").is_none());
    }

    #[test]
    fn test_branch_deduplication_with_slashes() {
        // Simulate the actual scenario:
        // - Directory: ~/w/insta.claude-fix-snapshot-merge-conflicts-xyz
        // - Branch: claude/fix-snapshot-merge-conflicts-xyz
        let dir = "~/w/insta.claude-fix-snapshot-merge-conflicts-xyz";
        let branch = "claude/fix-snapshot-merge-conflicts-xyz";

        let normalized_branch = worktrunk::config::sanitize_branch_name(branch);
        let pattern = format!(".{normalized_branch}");

        assert!(
            dir.ends_with(&pattern),
            "Directory '{}' should end with pattern '{}' (normalized from branch '{}')",
            dir,
            pattern,
            branch
        );
    }

    #[test]
    fn test_statusline_truncation() {
        use color_print::cformat;

        // Simulate a long statusline with styled content
        let long_line =
            cformat!("main  <cyan>?</><dim>^</>  http://very-long-branch-name.localhost:3000");

        // Truncate to 30 visible characters
        let truncated = truncate_visible(&long_line, 30);

        // Should end with ellipsis and be shorter
        assert!(
            truncated.contains('…'),
            "Truncated line should contain ellipsis: {truncated}"
        );

        // Visible width should be <= 30
        let visible: String = truncated
            .chars()
            .filter(|c| !c.is_ascii_control())
            .collect();
        // Simple check: the truncated output should be shorter than original
        let original_visible: String = long_line
            .chars()
            .filter(|c| !c.is_ascii_control())
            .collect();
        assert!(
            visible.len() < original_visible.len(),
            "Truncated should be shorter: {} vs {}",
            visible.len(),
            original_visible.len()
        );
    }

    #[test]
    fn test_context_gauge_formatting() {
        // Test boundary values for each moon phase symbol (waning - darker as context fills)
        // Thresholds use exponential halving: ratio 16:8:4:2:1, normalized to 100%
        assert_eq!(format_context_gauge(0.0), "🌕 0%");
        assert_eq!(format_context_gauge(51.0), "🌕 51%");
        assert_eq!(format_context_gauge(52.0), "🌔 52%");
        assert_eq!(format_context_gauge(77.0), "🌔 77%");
        assert_eq!(format_context_gauge(78.0), "🌓 78%");
        assert_eq!(format_context_gauge(90.0), "🌓 90%");
        assert_eq!(format_context_gauge(91.0), "🌒 91%");
        assert_eq!(format_context_gauge(97.0), "🌒 97%");
        assert_eq!(format_context_gauge(98.0), "🌑 98%");
        assert_eq!(format_context_gauge(100.0), "🌑 100%");
    }

    #[test]
    fn test_context_gauge_fractional_percentages() {
        // Fractional values are rounded (per {:.0} format specifier)
        // Rust uses banker's rounding (round half to even)
        assert_eq!(format_context_gauge(42.7), "🌕 43%"); // 43% is in 0-51% range
        assert_eq!(format_context_gauge(0.4), "🌕 0%");
        assert_eq!(format_context_gauge(0.5), "🌕 0%"); // banker's rounding: 0.5 rounds to even (0)
        assert_eq!(format_context_gauge(1.5), "🌕 2%"); // banker's rounding: 1.5 rounds to even (2)
        assert_eq!(format_context_gauge(99.9), "🌑 100%");
    }

    #[test]
    fn test_context_gauge_edge_cases() {
        // Negative values: symbol clamps to bright (low usage), but display shows original value
        assert_eq!(format_context_gauge(-5.0), "🌕 -5%");
        assert_eq!(format_context_gauge(-0.1), "🌕 -0%"); // rounds to -0%

        // Values over 100%: symbol clamps to dark (high usage), but display shows original value
        assert_eq!(format_context_gauge(105.0), "🌑 105%");
        assert_eq!(format_context_gauge(150.0), "🌑 150%");
    }

    #[test]
    fn test_claude_code_context_parse_with_context_window() {
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Opus"},
            "context_window": {"used_percentage": 42.5}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.current_dir, "/tmp/test");
        assert_eq!(ctx.model_name, Some("Opus".to_string()));
        assert_eq!(ctx.context_used_percentage, Some(42.5));
    }

    #[test]
    fn test_claude_code_context_parse_missing_context_window() {
        // context_window is optional
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "model": {"display_name": "Opus"}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.context_used_percentage, None);
    }

    #[test]
    fn test_claude_code_context_parse_context_window_missing_percentage() {
        // context_window can exist without used_percentage
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "context_window": {}
        }"#;

        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.context_used_percentage, None);
    }

    // --- Rate-limit pace prediction ---

    #[test]
    fn test_erf_matches_reference_values() {
        // Reference values from Abramowitz & Stegun Table 7.1; A&S 7.1.26
        // claims |error| < 1.5e-7.
        let cases = [
            (0.0, 0.0),
            (0.5, 0.520_499_877_8),
            (1.0, 0.842_700_792_9),
            (2.0, 0.995_322_265_0),
            (-1.0, -0.842_700_792_9),
        ];
        for (x, expected) in cases {
            assert!(
                (erf(x) - expected).abs() < 1e-6,
                "erf({x}): expected {expected}, got {}",
                erf(x)
            );
        }
        // Tail saturates.
        assert!((erf(5.0) - 1.0).abs() < 1e-7);
        assert!((erf(-5.0) + 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_standard_normal_cdf_known_points() {
        let cases = [
            (0.0, 0.5),
            (1.0, 0.841_344_746),
            (-1.0, 0.158_655_254),
            (1.96, 0.975_002_105),
            (-1.96, 0.024_997_895),
        ];
        for (z, expected) in cases {
            assert!(
                (standard_normal_cdf(z) - expected).abs() < 1e-5,
                "phi({z}): expected {expected}, got {}",
                standard_normal_cdf(z)
            );
        }
    }

    #[test]
    fn test_p_over_boundary_cases() {
        // t = 1: deterministic.
        assert_eq!(p_over(0.5, 1.0, &FIVE_HOUR_PRIORS), 0.0);
        assert_eq!(p_over(1.0, 1.0, &FIVE_HOUR_PRIORS), 1.0);
        assert_eq!(p_over(1.5, 1.0, &FIVE_HOUR_PRIORS), 1.0);

        // t = 0: predictive is the prior, so u doesn't matter.
        let p0 = p_over(0.0, 0.0, &FIVE_HOUR_PRIORS);
        let p1 = p_over(0.9, 0.0, &FIVE_HOUR_PRIORS);
        assert!((p0 - p1).abs() < 1e-9);
        // m0 < 1 means the prior expects to come in under the cap.
        assert!(p0 < 0.5);
    }

    #[test]
    fn test_p_over_matches_prototype_values() {
        // Values computed by the Python prototype (.tmp/rate_limit_pace.py)
        // with the same priors. Tolerance covers the A&S erf error and
        // round-off in either implementation.
        let cases: &[(f64, f64, &WindowPriors, f64, &str)] = &[
            (0.05, 0.03, &FIVE_HOUR_PRIORS, 0.415, "5%@3% on 5h"),
            (0.30, 0.20, &FIVE_HOUR_PRIORS, 0.59, "30%@20% on 5h"),
            (0.50, 0.40, &FIVE_HOUR_PRIORS, 0.61, "50%@40% on 5h"),
            (0.80, 0.60, &FIVE_HOUR_PRIORS, 0.82, "80%@60% on 5h"),
            (0.50, 0.30, &SEVEN_DAY_PRIORS, 0.83, "50%@30% on 7d"),
            (0.30, 0.20, &SEVEN_DAY_PRIORS, 0.63, "30%@20% on 7d"),
        ];
        for &(u, t, p, expected, label) in cases {
            let got = p_over(u, t, p);
            assert!(
                (got - expected).abs() < 0.02,
                "{label}: expected ~{expected:.3}, got {got:.3}"
            );
        }
    }

    #[test]
    fn test_p_over_monotonic_in_u() {
        // Holding t and priors fixed, P(over) is non-decreasing in u.
        let mut prev = 0.0;
        for u_pct in 0..=100 {
            let u = u_pct as f64 / 100.0;
            let p = p_over(u, 0.5, &FIVE_HOUR_PRIORS);
            assert!(
                p >= prev - 1e-9,
                "non-monotone at u={u}: prev={prev}, p={p}"
            );
            prev = p;
        }
    }

    #[test]
    fn test_format_clock_renders_12h_and_elides_zero_minutes() {
        use chrono::TimeZone;
        let cases: &[(u32, u32, &str)] = &[
            (15, 0, "3pm"),
            (17, 45, "5:45pm"),
            (0, 0, "12am"),
            (12, 0, "12pm"),
            (1, 5, "1:05am"),
            (23, 59, "11:59pm"),
        ];
        for &(h, m, expected) in cases {
            let d = chrono::Utc.with_ymd_and_hms(2026, 5, 23, h, m, 0).unwrap();
            assert_eq!(format_clock(&d, ClockFormat::H12), expected, "h={h} m={m}");
        }
    }

    #[test]
    fn test_format_clock_renders_24h_always_with_minutes_and_zero_pad() {
        use chrono::TimeZone;
        // 24h never elides minutes (`15:00`, not `15`) and zero-pads the hour
        // (`09:00`, not `9:00`) so the digit count stays constant — both are
        // the everyday written form in 24h locales.
        let cases: &[(u32, u32, &str)] = &[
            (15, 0, "15:00"),
            (17, 45, "17:45"),
            (0, 0, "00:00"),
            (9, 5, "09:05"),
            (23, 59, "23:59"),
        ];
        for &(h, m, expected) in cases {
            let d = chrono::Utc.with_ymd_and_hms(2026, 5, 23, h, m, 0).unwrap();
            assert_eq!(format_clock(&d, ClockFormat::H24), expected, "h={h} m={m}");
        }
    }

    #[test]
    fn test_classify_locale_picks_12h_for_us_canada_philippines_only() {
        // The small set where 12h is the everyday written form.
        for loc in [
            "en_US",
            "en_US.UTF-8",
            "en_PH.UTF-8",
            "en_CA.UTF-8",
            "en_US.UTF-8@posix",
        ] {
            assert_eq!(classify_locale(loc), ClockFormat::H12, "{loc}");
        }
        // Everything else — other English variants, other languages, POSIX
        // C, and unset — falls through to 24h.
        for loc in [
            "en_GB",
            "en_GB.UTF-8",
            "en_AU.UTF-8",
            "fr_FR.UTF-8",
            "de_DE.UTF-8",
            "ja_JP.UTF-8",
            "C",
            "POSIX",
            "",
        ] {
            assert_eq!(classify_locale(loc), ClockFormat::H24, "{loc}");
        }
    }

    #[test]
    fn test_format_window_bounds_five_hour_is_clock_only() {
        use chrono::TimeZone;
        // resets_at = 2026-05-23 15:00 UTC, window = 5h → start = 10:00 UTC.
        let resets_at = chrono::Utc
            .with_ymd_and_hms(2026, 5, 23, 15, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(
            format_window_bounds(resets_at, FIVE_HOUR_SECS, &chrono::Utc, ClockFormat::H12),
            "10am\u{2013}3pm"
        );
        assert_eq!(
            format_window_bounds(resets_at, FIVE_HOUR_SECS, &chrono::Utc, ClockFormat::H24),
            "10:00\u{2013}15:00"
        );
    }

    #[test]
    fn test_format_window_bounds_seven_day_is_weekday_with_end_time() {
        use chrono::TimeZone;
        // 2026-01-05 is a Monday; resets Mon 15:00 UTC, window 7d → start
        // prev Mon 15:00 UTC. Both endpoints are Monday.
        let resets_at = chrono::Utc
            .with_ymd_and_hms(2026, 1, 5, 15, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(
            format_window_bounds(resets_at, SEVEN_DAY_SECS, &chrono::Utc, ClockFormat::H12),
            "Mon\u{2013}Mon 3pm"
        );
        assert_eq!(
            format_window_bounds(resets_at, SEVEN_DAY_SECS, &chrono::Utc, ClockFormat::H24),
            "Mon\u{2013}Mon 15:00"
        );
    }

    fn make_reading(
        used_pct: f64,
        t_elapsed: f64,
        priors: &'static WindowPriors,
        now: i64,
        window_secs: i64,
    ) -> RateLimitReading {
        let resets_at = now + ((1.0 - t_elapsed) * window_secs as f64).round() as i64;
        RateLimitReading {
            name: if window_secs == FIVE_HOUR_SECS {
                "five_hour"
            } else {
                "seven_day"
            },
            used_percentage: used_pct,
            resets_at,
            window_secs,
            priors,
        }
    }

    #[test]
    fn test_select_binding_window_empty() {
        assert!(select_binding_window(&[], 1_700_000_000).is_none());
    }

    #[test]
    fn test_select_binding_window_below_threshold_hides() {
        let now = 1_700_000_000_i64;
        // 5%@3% on the 5h window: P ~ 42%, below the 50% trigger.
        let r = make_reading(5.0, 0.03, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        assert!(select_binding_window(&[r], now).is_none());
    }

    #[test]
    fn test_select_binding_window_visible_single() {
        let now = 1_700_000_000_i64;
        // 80%@60% on 5h: P ~ 82%, above threshold.
        let r = make_reading(80.0, 0.60, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        let sel = select_binding_window(std::slice::from_ref(&r), now);
        assert!(sel.is_some());
        assert_eq!(sel.unwrap().used_percentage, 80.0);
    }

    #[test]
    fn test_select_binding_window_logs_at_debug() {
        // The per-window debug line's format args are lazily skipped at
        // default verbosity; enabling Debug evaluates them. The
        // not-yet-started window (negative elapsed, clamped to 0) is the
        // case the `t.max(0.001)` division guard exists for.
        log::set_max_level(log::LevelFilter::Debug);
        let now = 1_700_000_000_i64;
        let readings = [
            make_reading(80.0, 0.60, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS),
            make_reading(5.0, -0.10, &SEVEN_DAY_PRIORS, now, SEVEN_DAY_SECS),
        ];
        let sel = select_binding_window(&readings, now);
        log::set_max_level(log::LevelFilter::Off);
        assert_eq!(sel.unwrap().used_percentage, 80.0);
    }

    #[test]
    fn test_select_binding_window_picks_worse_of_two() {
        let now = 1_700_000_000_i64;
        let readings = [
            make_reading(50.0, 0.30, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS),
            make_reading(80.0, 0.40, &SEVEN_DAY_PRIORS, now, SEVEN_DAY_SECS),
        ];
        let sel = select_binding_window(&readings, now);
        assert!(sel.is_some());
        // 7d is worse-projected — wins.
        assert_eq!(sel.unwrap().used_percentage, 80.0);
    }

    #[test]
    fn test_select_binding_window_filters_below_threshold() {
        let now = 1_700_000_000_i64;
        let readings = [
            make_reading(5.0, 0.03, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS),
            make_reading(50.0, 0.30, &SEVEN_DAY_PRIORS, now, SEVEN_DAY_SECS),
        ];
        let sel = select_binding_window(&readings, now);
        assert!(sel.is_some());
        assert_eq!(sel.unwrap().used_percentage, 50.0);
    }

    #[test]
    fn test_format_rate_limit_segment_format_string() {
        let now = 1_700_000_000_i64;
        // u=0.8 used over t=0.6 of the window ⇒ pace = u/t ≈ 1.33 → "1.3×".
        let r = make_reading(80.0, 0.60, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        let out =
            format_rate_limit_segment(std::slice::from_ref(&r), now).expect("should be visible");
        // Strip ANSI before asserting on the visible characters — the segment
        // is wrapped in `<yellow>…</>`.
        let visible = out.ansi_strip();
        assert!(
            visible.starts_with("1.3×(") && visible.ends_with(')'),
            "unexpected format: {visible:?}"
        );
    }

    #[test]
    fn test_format_rate_limit_segment_near_cap_shows_used_pct() {
        let now = 1_700_000_000_i64;
        // Above 90% used the displayed number switches from pace to used %:
        // remaining headroom, not speed, is the actionable quantity there.
        let r = make_reading(95.0, 0.60, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        let out =
            format_rate_limit_segment(std::slice::from_ref(&r), now).expect("should be visible");
        let visible = out.ansi_strip();
        assert!(
            visible.starts_with("95%(") && visible.ends_with(')'),
            "unexpected format: {visible:?}"
        );
        // Exactly 90% stays on the pace form — the switch is strictly above.
        let r = make_reading(90.0, 0.60, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        let out =
            format_rate_limit_segment(std::slice::from_ref(&r), now).expect("should be visible");
        let visible = out.ansi_strip();
        assert!(
            visible.starts_with("1.5×("),
            "unexpected format: {visible:?}"
        );
    }

    #[test]
    fn test_format_rate_limit_segment_hidden() {
        let now = 1_700_000_000_i64;
        let r = make_reading(5.0, 0.03, &FIVE_HOUR_PRIORS, now, FIVE_HOUR_SECS);
        assert!(format_rate_limit_segment(&[r], now).is_none());
    }

    #[test]
    fn test_parse_rate_limits_both_windows() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "rate_limits": {
                    "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
                    "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}
                }
            }"#,
        )
        .unwrap();
        let limits = parse_rate_limits(&v);
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].used_percentage, 23.5);
        assert_eq!(limits[0].resets_at, 1738425600);
        assert_eq!(limits[0].window_secs, FIVE_HOUR_SECS);
        assert_eq!(limits[1].used_percentage, 41.2);
        assert_eq!(limits[1].resets_at, 1738857600);
        assert_eq!(limits[1].window_secs, SEVEN_DAY_SECS);
    }

    #[test]
    fn test_parse_rate_limits_one_window_present() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "rate_limits": {
                    "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600}
                }
            }"#,
        )
        .unwrap();
        let limits = parse_rate_limits(&v);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].window_secs, FIVE_HOUR_SECS);
    }

    #[test]
    fn test_parse_rate_limits_absent() {
        let v: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
        assert!(parse_rate_limits(&v).is_empty());
    }

    #[test]
    fn test_parse_rate_limits_partial_window_dropped() {
        // Each window needs both used_percentage and resets_at; otherwise
        // it's treated as absent rather than half-parsed.
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "rate_limits": {
                    "five_hour": {"used_percentage": 23.5},
                    "seven_day": {"resets_at": 1738857600}
                }
            }"#,
        )
        .unwrap();
        assert!(parse_rate_limits(&v).is_empty());
    }

    #[test]
    fn test_claude_code_context_parse_with_rate_limits() {
        let json = r#"{
            "workspace": {"current_dir": "/tmp/test"},
            "rate_limits": {
                "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
                "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}
            }
        }"#;
        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert_eq!(ctx.rate_limits.len(), 2);
    }

    #[test]
    fn test_claude_code_context_parse_no_rate_limits() {
        let json = r#"{"workspace": {"current_dir": "/tmp/test"}}"#;
        let ctx = ClaudeCodeContext::parse(json).expect("should parse");
        assert!(ctx.rate_limits.is_empty());
    }
}
