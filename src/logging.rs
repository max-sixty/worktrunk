//! Tracing-subscriber setup for the `wt` binary.
//!
//! Three layered subscribers cooperate to give each verbosity level the
//! routing it needs. Filtering is structural (per-layer `Filter`), not done
//! after-the-fact in a format closure:
//!
//! | layer       | filter                                            | format            |
//! | ----------- | ------------------------------------------------- | ----------------- |
//! | stderr      | off at `-vv`; else `$RUST_LOG` or `-v` baseline   | styled with ANSI  |
//! | `trace.log` | `-vv` only, excludes `SUBPROCESS_FULL_TARGET`     | plain text        |
//! | `output.log`| `-vv` only, includes only `SUBPROCESS_FULL_TARGET`| raw (no prefix)   |
//!
//! The `log` crate calls (used throughout the codebase) are bridged into
//! `tracing` by [`tracing_log::LogTracer::init`] — every layer above sees
//! both native `tracing::*` events and forwarded `log::*` records.

use std::fmt::{self, Write as _};

use color_print::cformat;
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, FilterExt, LevelFilter, Targets, filter_fn};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use worktrunk::shell_exec::SUBPROCESS_FULL_TARGET;
use worktrunk::styling::{eprintln, info_message};

use crate::log_files::{self, OutputMakeWriter, TraceMakeWriter};
use crate::output;

/// Single-character thread label (e.g. `a`, `b`, …, `A`, …) used to group
/// concurrent records by thread in stderr / trace.log output.
fn thread_label() -> char {
    let thread_id = format!("{:?}", std::thread::current().id());
    thread_id
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(")"))
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| {
            if n == 0 {
                '0'
            } else if n <= 26 {
                char::from(b'a' + (n - 1) as u8)
            } else if n <= 52 {
                char::from(b'A' + (n - 27) as u8)
            } else {
                '?'
            }
        })
        .unwrap_or('?')
}

/// Pull the rendered message out of a `tracing` event.
///
/// Native `tracing::debug!("…")` and `log::*`-bridged calls both put their
/// rendered text in the `message` field. Other fields are ignored — every
/// caller in worktrunk emits the message inline.
fn event_message(event: &Event<'_>) -> String {
    struct V(String);
    impl tracing::field::Visit for V {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                let _ = write!(&mut self.0, "{value:?}");
            }
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "message" {
                self.0.push_str(value);
            }
        }
    }
    let mut v = V(String::new());
    event.record(&mut v);
    v.0
}

/// Stderr formatter: replicates the legacy env_logger styling pre-migration.
///
/// `$ cmd [worktree]` headers bold the command. `  ! …` continuation lines
/// (subprocess stderr) are reddened. Everything else gets the thread-label
/// prefix.
struct StderrFormat;

impl<S, N> FormatEvent<S, N> for StderrFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let thread_num = thread_label();
        let msg = event_message(event);
        if let Some(rest) = msg.strip_prefix("$ ") {
            // Standalone tools (gh, glab) emit no `[ctx]` suffix.
            let (command, worktree) = match rest.find(" [") {
                Some(pos) => (&rest[..pos], &rest[pos..]),
                None => (rest, ""),
            };
            writeln!(
                writer,
                "{}",
                cformat!("<dim>[{thread_num}]</> $ <bold>{command}</>{worktree}")
            )
        } else if msg.starts_with("  ! ") {
            writeln!(
                writer,
                "{}",
                cformat!("<dim>[{thread_num}]</> <red>{msg}</>")
            )
        } else {
            writeln!(writer, "{}", cformat!("<dim>[{thread_num}]</> {msg}"))
        }
    }
}

/// `trace.log` formatter: plain `[<thread>] <message>`, no ANSI, one line
/// per event. Matches the on-disk layout pre-migration.
struct TraceFileFormat;

impl<S, N> FormatEvent<S, N> for TraceFileFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let thread_num = thread_label();
        let msg = event_message(event);
        writeln!(writer, "[{thread_num}] {msg}")
    }
}

/// `output.log` formatter: the message verbatim. Subprocess bodies are
/// already prefixed (`  …` / `  ! …`) by `shell_exec::format_stream_full`.
struct OutputFileFormat;

impl<S, N> FormatEvent<S, N> for OutputFileFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        writeln!(writer, "{}", event_message(event))
    }
}

/// Install the tracing subscriber and bridge `log::*` calls.
///
/// Stage-by-stage:
///
/// 1. Set verbosity for downstream styling code (unchanged).
/// 2. Open the file sinks before the subscriber registers, so the
///    `Repository::current()` rev-parse fired by `try_create` doesn't emit
///    records into a half-built pipeline. Pre-tracing-init `log::*` calls
///    are dropped by the default no-op `log` logger, which is the right
///    behavior — there's nothing meaningful to attribute the call to before
///    the subscriber exists.
/// 3. Build three layered subscribers, each gated by both a verbosity check
///    and the relevant `LogSink::is_active()` so a failed file open turns
///    its layer into a no-op rather than silently dropping records.
/// 4. Bridge `log::*` into tracing via `LogTracer`. Idempotent on
///    re-invocation (init is `.ok()`-discarded).
/// 5. Announce the file destinations on stderr at `-vv`.
pub(crate) fn init(verbose_level: u8) {
    output::set_verbosity(verbose_level);

    if verbose_level >= 2 {
        log_files::init();
    }

    // Layers wrap a base `fmt::Layer` with a `Filter`. `Option<Layer>`
    // is itself a `Layer` (no-op when `None`), so verbosity gates compose
    // naturally with subscriber `.with(...)` calls.
    let stderr_layer = build_stderr_layer(verbose_level);
    let trace_layer = build_trace_layer(verbose_level);
    let output_layer = build_output_layer(verbose_level);

    let registered = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(trace_layer)
        .with(output_layer)
        .try_init()
        .is_ok();

    if registered {
        // Forward `log::*` macros into `tracing`. Must come after subscriber
        // init: `LogTracer::enabled` consults the tracing dispatcher.
        //
        // The builder's `with_max_level` caps `log::max_level()` — the static
        // gate `log_enabled!` checks before format args are evaluated. Match
        // env_logger's old behavior: take the higher of the verbosity-derived
        // level and `RUST_LOG`'s level. Without this, the default
        // `LevelFilter::max()` would always pass the static check, forcing
        // every `log::debug!(…)` site to evaluate its format args — exposing
        // arithmetic that's safe today only because the macro short-circuits
        // (e.g. `now_secs - cached.checked_at` in `list/ci_status` is fine
        // under monotonic-ish clocks but panics when args are evaluated
        // against a clock-skewed fixture).
        let from_verbose = match verbose_level {
            0 => log::LevelFilter::Warn,
            1 => log::LevelFilter::Info,
            _ => log::LevelFilter::Debug,
        };
        let log_max = from_verbose.max(rust_log_level());
        let _ = tracing_log::LogTracer::builder()
            .with_max_level(log_max)
            .init();
    }

    if verbose_level >= 2 {
        announce_trace_destination();
    }
}

/// Highest level mentioned in `$RUST_LOG`, or `Off` if absent / unparsable.
///
/// `RUST_LOG=info,worktrunk=debug` returns `Debug` (the most permissive
/// directive wins). The `EnvFilter` on the stderr layer still does the
/// per-target matching; this helper just lifts `log::max_level` high enough
/// that `log::*` macros don't short-circuit before reaching the dispatcher.
fn rust_log_level() -> log::LevelFilter {
    let Ok(raw) = std::env::var("RUST_LOG") else {
        return log::LevelFilter::Off;
    };
    raw.split(',')
        .filter_map(|directive| {
            // Each directive is either `level` or `target=level` (the level
            // is the rightmost `=`-separated token). Unknown tokens are
            // treated as `Off` so they don't accidentally lift the ceiling.
            let level_token = directive.rsplit('=').next().unwrap_or(directive).trim();
            level_token.parse::<log::LevelFilter>().ok()
        })
        .max()
        .unwrap_or(log::LevelFilter::Off)
}

/// Stderr layer: off at `-vv` (the file layers take over). At `-v`, pin
/// Info regardless of `RUST_LOG`. At `-v 0`, honor `RUST_LOG` (default
/// `off`). Excludes `SUBPROCESS_FULL_TARGET` at all levels — raw bodies
/// must never reach the terminal.
fn build_stderr_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level >= 2 {
        return None;
    }
    let env_filter = if verbose_level >= 1 {
        EnvFilter::new("info")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off"))
    };
    let exclude_full = filter_fn(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .event_format(StderrFormat)
        .with_filter(env_filter.and(exclude_full));
    Some(layer)
}

/// `trace.log` layer: only when `-vv` opened the file. Captures everything
/// at Debug+ except `SUBPROCESS_FULL_TARGET` (raw bodies go to `output.log`).
fn build_trace_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::TRACE.is_active() {
        return None;
    }
    let level = Targets::new().with_default(LevelFilter::DEBUG);
    let exclude_full = filter_fn(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(TraceMakeWriter)
        .with_ansi(false)
        .event_format(TraceFileFormat)
        .with_filter(level.and(exclude_full));
    Some(layer)
}

/// `output.log` layer: only `SUBPROCESS_FULL_TARGET` records, raw passthrough.
fn build_output_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::OUTPUT.is_active() {
        return None;
    }
    let only_full = filter_fn(|meta| meta.target() == SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(OutputMakeWriter)
        .with_ansi(false)
        .event_format(OutputFileFormat)
        .with_filter(only_full);
    Some(layer)
}

/// Print a one-line stderr pointer at `-vv` so users know where the noisy
/// log pipeline output went. Silent if `trace.log` couldn't be opened
/// (outside a git repo, permission error) — there's nothing meaningful to
/// point at.
fn announce_trace_destination() {
    // TRACE and OUTPUT open independently — `LogSink::init` succeeds per
    // file. The (Some, None) case (trace.log open, output.log failed) is
    // rare but real (path-type mismatch, fs quota); the reverse is
    // possible too but `output.log` alone has no `$ cmd` context, so we
    // stay silent there.
    let Some(trace_path) = log_files::TRACE.path() else {
        return;
    };
    let trace_display = worktrunk::path::format_path_for_display(&trace_path);
    let msg = match log_files::OUTPUT.path() {
        Some(output_path) => {
            let output_display = worktrunk::path::format_path_for_display(&output_path);
            cformat!(
                "Tracing to <underline>{trace_display}</> (raw subprocess output @ <underline>{output_display}</>)"
            )
        }
        None => cformat!("Tracing to <underline>{trace_display}</> (output.log unavailable)"),
    };
    eprintln!("{}", info_message(msg));
}
