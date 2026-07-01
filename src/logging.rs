//! Tracing-subscriber setup for the `wt` binary.
//!
//! Four layered subscribers cooperate to give each verbosity level the
//! routing it needs. Filtering is structural (per-layer `Filter`), not done
//! after-the-fact in a format closure:
//!
//! | layer            | filter                                              | format            |
//! | ---------------- | --------------------------------------------------- | ----------------- |
//! | stderr           | `$RUST_LOG` or flag baseline (`Off`/`Info`/`Info`)  | human, ANSI-styled |
//! | `trace.log`      | `-vv` only, excludes `SUBPROCESS_FULL_TARGET`       | human, plain text |
//! | `trace.jsonl`    | `-vv` only, excludes both subprocess targets        | one JSON object per event (machine) |
//! | `subprocess.log` | `-vv` only, includes only `SUBPROCESS_FULL_TARGET`  | raw bodies + `$ cmd … seq=N` headers |
//!
//! The two human routes (stderr, `trace.log`) render `[wt-trace]` records as
//! readable lines (`✓ git status [wt]  12.3ms`); `trace.jsonl` is the sole
//! machine sink, carrying the full structured fields that `src/trace/parse.rs`
//! and `wt-perf` consume. So the human and machine formats are decoupled — the
//! human lines never carry `key=value` clutter, and a parser never reads them.
//!
//! At `-vv` the stderr layer keeps its Info baseline — `-vv` is a strict
//! superset of `-v`, with Debug-level records (the noisy ones, including
//! the bounded subprocess preview) routed to the file layers only.
//!
//! Each layer's target filter carries an explicit TRACE `max_level_hint`
//! (see [`target_filter`]) so the `EnvFilter`'s level bound survives the
//! `Filter::and` and reaches the global `LevelFilter`. That keeps the native
//! `tracing::*` macros cheap: below the active verbosity a `debug!` is gated
//! out before it builds its fields, exactly as `log::debug!` short-circuits
//! on `log::max_level`. Without the hint, `And`'s `cmp::min` (where a `None`
//! hint sorts below any `Some`) would erase the bound, pin the global level
//! at TRACE, and force every `debug!` to format its args even at `-v0`.
//!
//! The `log` crate calls (used throughout the codebase) are bridged into
//! `tracing` by [`tracing_log::LogTracer::init`] — every layer above sees
//! both native `tracing::*` events and forwarded `log::*` records.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::time::Duration;

use color_print::cformat;
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, FilterExt, FilterFn, LevelFilter, filter_fn};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use worktrunk::shell_exec::{SUBPROCESS_BOUNDED_TARGET, SUBPROCESS_FULL_TARGET};
use worktrunk::styling::{eprintln, info_message};
use worktrunk::trace::{WT_TRACE_TARGET, now_us, thread_id};
use worktrunk::utils::escape_controls;

use crate::log_files::{self, SubprocessMakeWriter, TraceJsonlMakeWriter, TraceMakeWriter};
use crate::output;

/// Single-character thread label (e.g. `a`, `b`, …, `A`, …) used to group
/// concurrent records by thread in stderr / trace.log output.
fn thread_label() -> char {
    let thread_id = format!("{:?}", std::thread::current().id());
    let parsed = thread_id
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(")"))
        .and_then(|s| s.parse::<usize>().ok());
    label_for_thread_index(parsed)
}

/// Pure helper: map a parsed `ThreadId` number to a single-char label.
///
/// `n == 0` → `'0'`; `1..=26` → `'a'..='z'`; `27..=52` → `'A'..='Z'`;
/// everything else (including a `None` from a `ThreadId` whose `Debug`
/// shape we don't recognize) → `'?'`. Tested via the branch coverage
/// below — `thread_label` itself never sees `n == 0` or `n > 52` in
/// practice, so its `unwrap_or` chain stays exercised only through
/// `label_for_thread_index`.
fn label_for_thread_index(n: Option<usize>) -> char {
    let Some(n) = n else { return '?' };
    if n == 0 {
        '0'
    } else if n <= 26 {
        char::from(b'a' + (n - 1) as u8)
    } else if n <= 52 {
        char::from(b'A' + (n - 27) as u8)
    } else {
        '?'
    }
}

/// Pull the rendered message out of a `tracing` event.
///
/// Native `tracing::debug!("…")` and `log::*`-bridged calls both put their
/// rendered text in the `message` field, recorded as `&dyn Debug` (an
/// `Arguments` instance). Other fields are ignored — every caller in
/// worktrunk emits the message inline.
fn event_message(event: &Event<'_>) -> String {
    struct V(String);
    impl tracing::field::Visit for V {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                let _ = write!(&mut self.0, "{value:?}");
            }
        }
    }
    let mut v = V(String::new());
    event.record(&mut v);
    v.0
}

/// Pure helper: render a single log message for stderr with the thread
/// label and the styling rules `StderrFormat` applies. Factored out so the
/// branches (`$ cmd [ctx]`, `✓ cmd [ctx]  dur`, `  ! err`, plain) can be
/// unit-tested without standing up a `tracing` subscriber.
fn style_stderr_line(thread_num: char, msg: &str) -> String {
    // Command framing lines share one shape: a leading glyph, then the
    // command, then an optional `[context]` and trailing detail. `$` opens a
    // command; `✓`/`✗` report its completion (the humanized wt-trace records).
    // Bolding the command on all three makes a start line and its finish line
    // read as a pair.
    let framed = ['$', '✓', '✗'].into_iter().find_map(|glyph| {
        msg.strip_prefix(glyph)?
            .strip_prefix(' ')
            .map(|r| (glyph, r))
    });
    if let Some((glyph, rest)) = framed {
        // The command ends at the earliest separator: ` [` before a context,
        // or `  ` before a duration (standalone tools like gh emit neither).
        let boundary = [rest.find(" ["), rest.find("  ")]
            .into_iter()
            .flatten()
            .min();
        let (command, tail) = match boundary {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, ""),
        };
        cformat!("<dim>[{thread_num}]</> {glyph} <bold>{command}</>{tail}")
    } else if msg.starts_with("  ! ") {
        cformat!("<dim>[{thread_num}]</> <red>{msg}</>")
    } else {
        cformat!("<dim>[{thread_num}]</> {msg}")
    }
}

/// Stderr formatter for the human routes.
///
/// `$ cmd [worktree]` start lines and `✓`/`✗ cmd …` finish lines bold the
/// command (so a command's start and finish read as a pair). `  ! …`
/// continuation lines (subprocess stderr) are reddened. Everything else gets
/// the thread-label prefix.
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
        let msg = render_event_message(event);
        let line = style_stderr_line(thread_label(), &msg);
        writeln!(writer, "{line}")
    }
}

/// Render an event to its single-line human text payload — the humanized
/// `[wt-trace]` line ([`format_wt_trace`]) for events under
/// [`WT_TRACE_TARGET`], the raw `message` field for everything else. Shared
/// between the stderr and `trace.log` formatters so `[wt-trace]` records read
/// the same in both routes (`-vv` writes to the file; `RUST_LOG=debug -v`
/// surfaces them on stderr).
///
/// Control bytes are escaped here ([`escape_controls`]) — this is the single
/// chokepoint feeding both human-facing routes, so raw NUL/ESC from subprocess
/// output (e.g. the bounded preview of `git … -z`, or a `cmd`/`err` field
/// carrying captured bytes) can't ride into the terminal or `trace.log`, and
/// thus can't break the gist upload of the `diagnostic.md` that inlines
/// `trace.log`. `subprocess.log` keeps raw bytes verbatim: it renders via
/// [`event_message`] directly, not this helper.
fn render_event_message(event: &Event<'_>) -> String {
    let rendered = if event.metadata().target() == WT_TRACE_TARGET {
        let mut fields = WtTraceFields::default();
        event.record(&mut fields);
        format_wt_trace(&fields)
    } else {
        event_message(event)
    };
    // Reuse the owned `rendered` on the clean path — `escape_controls` borrows
    // when nothing needs escaping, so `into_owned()` would otherwise re-clone an
    // already-owned String on every log line. Only control-bearing lines allocate.
    match escape_controls(&rendered) {
        Cow::Borrowed(_) => rendered,
        Cow::Owned(escaped) => escaped,
    }
}

/// `trace.log` formatter: plain `[<thread>] <message>`, no ANSI, one line
/// per event — the human-readable trace artifact.
///
/// Events under [`WT_TRACE_TARGET`] are rendered as humanized lines by
/// [`format_wt_trace`]; everything else falls through to the message-prefix
/// shape. This is the only place the human trace line lives — emit sites in
/// `trace::emit` carry structured fields, not pre-formatted strings, and the
/// machine grammar lives in `trace.jsonl`.
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
        let msg = render_event_message(event);
        writeln!(writer, "[{thread_num}] {msg}")
    }
}

/// Captured fields from a single `WT_TRACE_TARGET` event, for the human
/// `trace.log` / stderr render. The visitor reads each field by name and
/// stores its value typed; the layer renderer then composes the humanized
/// line.
///
/// Only the fields the human line shows are captured — `ts`/`tid`/`seq` are
/// machine-only (the `[thread]` prefix already names the thread) and stay in
/// `trace.jsonl`, which serializes every field via its own generic visitor.
/// Unknown fields are dropped; the wt-trace grammar is closed (every key has
/// a fixed meaning).
#[derive(Default)]
struct WtTraceFields {
    kind: Option<String>,
    dur_us: Option<u64>,
    ok: Option<bool>,
    context: Option<String>,
    cmd: Option<String>,
    err: Option<String>,
    event: Option<String>,
    span: Option<String>,
}

impl tracing::field::Visit for WtTraceFields {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "dur_us" {
            self.dur_us = Some(value);
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if field.name() == "ok" {
            self.ok = Some(value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.record_string(field.name(), value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        // `tracing::debug!(field = %expr)` routes Display-formatted values
        // through here via a `DisplayValue` wrapper whose `Debug` impl calls
        // `Display` (bare text, no `"…"` quoting). Capture the rendered
        // string verbatim — the wire grammar adds its own quotes when it
        // composes the line.
        let mut buf = String::new();
        let _ = write!(&mut buf, "{value:?}");
        self.record_string(field.name(), &buf);
    }
}

impl WtTraceFields {
    fn record_string(&mut self, name: &str, value: &str) {
        match name {
            "kind" => self.kind = Some(value.to_owned()),
            "context" => self.context = Some(value.to_owned()),
            "cmd" => self.cmd = Some(value.to_owned()),
            "err" => self.err = Some(value.to_owned()),
            "event" => self.event = Some(value.to_owned()),
            "span" => self.span = Some(value.to_owned()),
            _ => {}
        }
    }
}

/// Render a `[wt-trace]` event as a human line for `trace.log` / stderr, with
/// one rendering of `cmd [context]` across every line type so a command's
/// start echo (`$ …`) and finish record (`✓ …`/`✗ …`) read as a pair:
///
/// ```text
/// ✓ git status [worktree]  12.3ms          cmd_completed, ok=true
/// ✗ git merge-base [main]  1.1s            cmd_completed, ok=false
/// ✗ git rev-list [.]  100ms  fatal: …      cmd_errored
/// · Showed skeleton                        instant (milestone)
/// ◷ build_hook_context  8.2ms              span (in-process)
/// ```
///
/// The leading glyph names the line type at a glance; durations render via
/// `Duration`'s compact `Debug` (`999µs`/`12.3ms`/`1.5s`). Machine fields
/// (`ts`/`tid`/`seq`) live only in `trace.jsonl` (see [`event_json`]). A
/// malformed or future `kind` renders a best-effort `· <kind>` line rather
/// than vanishing.
fn format_wt_trace(f: &WtTraceFields) -> String {
    let dur = |dur_us: Option<u64>| format!("{:?}", Duration::from_micros(dur_us.unwrap_or(0)));
    let with_context = |cmd: &str| match &f.context {
        Some(ctx) => format!("{cmd} [{ctx}]"),
        None => cmd.to_string(),
    };
    let cmd = || with_context(f.cmd.as_deref().unwrap_or(""));

    match f.kind.as_deref() {
        Some("cmd_completed") => {
            let glyph = if f.ok.unwrap_or(false) { '✓' } else { '✗' };
            format!("{glyph} {}  {}", cmd(), dur(f.dur_us))
        }
        Some("cmd_errored") => {
            format!(
                "✗ {}  {}  {}",
                cmd(),
                dur(f.dur_us),
                f.err.as_deref().unwrap_or("")
            )
        }
        Some("instant") => format!("· {}", f.event.as_deref().unwrap_or("")),
        Some("span") => format!("◷ {}  {}", f.span.as_deref().unwrap_or(""), dur(f.dur_us)),
        other => format!("· {}", other.unwrap_or("<unknown>")),
    }
}

/// Generic field collector for `trace.jsonl`: serializes whatever typed fields
/// an event carries into a JSON object, regardless of which fields they are.
///
/// This is the structured counterpart of [`event_message`] (which keeps only
/// the `message` field). A `[wt-trace]` event arrives with its full typed
/// grammar (`kind`, `ts`, `seq`, `cmd`, `dur_us`, `ok`, ...) and serializes
/// rich; a bridged `log::*` / bare `tracing::*` call arrives with just
/// `message` and serializes as `{"message":...}`. Nothing here is
/// `[wt-trace]`-specific, so a freshly-structured call site
/// (`tracing::warn!(branch = %b, "...")`) gains a queryable `branch` field with
/// no change to this layer.
///
/// serde_json owns all string escaping, so control bytes and embedded quotes in
/// any field are encoded losslessly -- the JSON path needs no `escape_controls`.
#[derive(Default)]
struct JsonFieldVisitor {
    map: serde_json::Map<String, serde_json::Value>,
}

impl tracing::field::Visit for JsonFieldVisitor {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.map.insert(field.name().to_owned(), value.into());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.map.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.map.insert(field.name().to_owned(), value.into());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.map.insert(field.name().to_owned(), value.into());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        // `message` (an `Arguments`) and `%`/`?`-formatted values arrive here;
        // store their rendered form as a JSON string.
        self.map
            .insert(field.name().to_owned(), format!("{value:?}").into());
    }
}

/// Render a single event as one line of `trace.jsonl`.
///
/// Collects the event's typed fields via [`JsonFieldVisitor`], then fills in
/// `ts`/`tid` from the runtime *only if the event did not carry them* (a
/// `[wt-trace]` record sets its own, captured at the precise event moment; a
/// bridged `log::*` line carries neither, so it gets the emission-time values),
/// and stamps the metadata `level`. Field order is serde_json's default
/// (sorted) -- irrelevant to a machine reader.
fn event_json(event: &Event<'_>) -> String {
    let mut visitor = JsonFieldVisitor::default();
    event.record(&mut visitor);
    let map = &mut visitor.map;
    map.entry("ts".to_owned())
        .or_insert_with(|| now_us().into());
    map.entry("tid".to_owned())
        .or_insert_with(|| thread_id().into());
    map.insert(
        "level".to_owned(),
        event
            .metadata()
            .level()
            .as_str()
            .to_ascii_lowercase()
            .into(),
    );
    serde_json::Value::Object(visitor.map).to_string()
}

/// `trace.jsonl` formatter: one JSON object per event, fields serialized
/// generically by [`event_json`].
///
/// The layer admits every event except the two subprocess-output targets (see
/// [`build_trace_jsonl_layer`]), so both structured `[wt-trace]` records and
/// free-form `log::*` messages arrive here. The former serialize rich (their
/// full typed grammar); the latter as `{"message":...}` until their call site
/// is given fields.
struct TraceJsonlFormat;

impl<S, N> FormatEvent<S, N> for TraceJsonlFormat
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
        writeln!(writer, "{}", event_json(event))
    }
}

/// `subprocess.log` formatter: the message verbatim. Body lines are already
/// prefixed (`  …` / `  ! …`) by `shell_exec::format_stream_full`, and each
/// command's block is introduced by a `$ cmd … seq=N` header line emitted by
/// `shell_exec::log_output` — both arrive pre-rendered, so this writer adds
/// nothing.
struct SubprocessFileFormat;

impl<S, N> FormatEvent<S, N> for SubprocessFileFormat
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
/// 3. Build four layered subscribers, each gated by both a verbosity check
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
    let trace_jsonl_layer = build_trace_jsonl_layer(verbose_level);
    let subprocess_layer = build_subprocess_layer(verbose_level);

    // `try_init` fails only if a subscriber is already installed (the
    // single-call-per-process contract). `wt`'s `main` runs `logging::init`
    // exactly once, so the error is just defensive — discard it. The
    // `LogTracer::init` below has the same shape for the same reason.
    let _ = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(trace_layer)
        .with(trace_jsonl_layer)
        .with(subprocess_layer)
        .try_init();

    // Forward `log::*` macros into `tracing`. Must come after subscriber
    // init: `LogTracer::enabled` consults the tracing dispatcher.
    //
    // The builder's `with_max_level` caps `log::max_level()` — the static
    // gate `log_enabled!` checks before a `log::*` record evaluates its
    // format args. Worktrunk's own emit sites are native `tracing::*`, gated
    // by the global level hint the layer filters now expose (see
    // `target_filter`), so this cap is specifically for the two `log`-API
    // consumers that hint can't reach: the `log::*` records forwarded from
    // dependencies, and the `log::log_enabled!` deep-logging guard in
    // `shell_exec::log_output`.
    //
    // Mirror the env-wins-when-set semantics the layer filters use (PR #2901):
    // if `RUST_LOG` is set, its level wins; otherwise the verbosity flag
    // baseline applies. Without an explicit cap, the default
    // `LevelFilter::max()` would always pass the static check, forcing
    // every dependency `log::debug!(…)` to format its args even when no
    // sink is active.
    let _ = tracing_log::LogTracer::builder()
        .with_max_level(effective_log_max_level(verbose_level, rust_log_level()))
        .init();

    if verbose_level >= 2 {
        announce_trace_destination();
    }
}

/// Effective ceiling for `log::max_level` given the verbosity flag and the
/// parsed `RUST_LOG` value. Env wins when set; otherwise the verbosity
/// baseline (`0` → Warn, `1` → Info, `2+` → Debug) applies. Factored out
/// so the merge logic can be tested without driving the process env.
fn effective_log_max_level(
    verbose_level: u8,
    from_env: Option<log::LevelFilter>,
) -> log::LevelFilter {
    let baseline = match verbose_level {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        _ => log::LevelFilter::Debug,
    };
    from_env.unwrap_or(baseline)
}

/// Highest level mentioned in `$RUST_LOG`, or `None` if unset / unparsable.
///
/// `RUST_LOG=info,worktrunk=debug` returns `Some(Debug)` (the most permissive
/// directive wins). The `EnvFilter` on the stderr / trace layers still does
/// the per-target matching; this helper just lifts `log::max_level` high
/// enough that `log::*` macros don't short-circuit before reaching the
/// dispatcher.
fn rust_log_level() -> Option<log::LevelFilter> {
    let raw = std::env::var("RUST_LOG").ok()?;
    raw.split(',')
        .filter_map(|directive| {
            // Each directive is either `level` or `target=level` (the level
            // is the rightmost `=`-separated token). Unknown tokens parse
            // as `None` and don't contribute to the ceiling.
            let level_token = directive.rsplit('=').next().unwrap_or(directive).trim();
            level_token.parse::<log::LevelFilter>().ok()
        })
        .max()
}

/// Environment variable mirroring the `-v`/`-vv` flags as a level
/// (`0`/`1`/`2`) — the env-var equivalent of the flag. Unlike the flag it is
/// read everywhere, including shell completion, which exits before `main`
/// parses the CLI; that is the only way to drive completion's logging (and, at
/// level 2, the `-vv` trace files). Combined with the flag via `max`: the env
/// sets a baseline the flag can raise but never lower.
pub(crate) const VERBOSE_ENV: &str = "WORKTRUNK_VERBOSE";

/// Read [`VERBOSE_ENV`] from the process environment into a verbosity count.
/// See [`parse_verbose_level`] for the grammar.
pub(crate) fn env_verbose_level() -> u8 {
    parse_verbose_level(std::env::var(VERBOSE_ENV).ok().as_deref())
}

/// Pure parse of a [`VERBOSE_ENV`] value into a `0`/`1`/`2…` count. Unset,
/// empty, or unparsable values yield `0`; the parse is lossy (never errors) so
/// a stray value can't break a command — least of all completion, where it
/// would corrupt the candidate list. Extracted as a pure function so it can be
/// unit-tested without mutating the process env (which races parallel tests).
fn parse_verbose_level(raw: Option<&str>) -> u8 {
    raw.and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(0)
}

/// Wrap a target predicate as a `Filter` that bounds by callsite *target*,
/// never by level — and says so via an explicit TRACE `max_level_hint`.
///
/// A bare [`filter_fn`] reports no hint (`None`). [`FilterExt::and`] returns
/// `None` if *either* side lacks one (`cmp::min`, and a `None` hint sorts
/// below any `Some`), so ANDing a hintless target filter onto an `EnvFilter`
/// erases its level bound. That `None` then propagates through the layered
/// subscriber's `pick_level_hint` up to the global `LevelFilter::current()`,
/// pinning it at TRACE — at which point the `tracing::*` macros stop
/// short-circuiting and build their fields at every verbosity, even `-v0`
/// where nothing records them. Tagging the target filter with TRACE (honest:
/// it really does pass all levels) lets the `EnvFilter`'s real bound survive.
fn target_filter<F>(predicate: F) -> FilterFn<F>
where
    F: Fn(&tracing::Metadata<'_>) -> bool,
{
    filter_fn(predicate).with_max_level_hint(LevelFilter::TRACE)
}

/// Stderr layer: the flag sets a baseline (`Off` / `Info` / `Info`) and
/// `RUST_LOG`, when set, overrides via the standard directive grammar —
/// matching the env-wins-when-set convention (see PR #2901). At `-vv`
/// stderr keeps the Info baseline so `-vv` is a strict superset of `-v`;
/// Debug-level records (the noisy ones) route to the file layers only.
/// Excludes `SUBPROCESS_FULL_TARGET` at all levels — raw bodies must
/// never reach the terminal.
fn build_stderr_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let baseline = match verbose_level {
        0 => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    };
    let env_filter = EnvFilter::builder()
        .with_default_directive(baseline.into())
        .from_env_lossy();
    let exclude_full = target_filter(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .event_format(StderrFormat)
        .with_filter(env_filter.and(exclude_full));
    Some(layer)
}

/// `trace.log` layer: only when `-vv` opened the file. Captures everything
/// at the Debug baseline (`RUST_LOG`, when set, overrides — e.g.
/// `RUST_LOG=trace wt -vv` lifts the file to Trace) except
/// `SUBPROCESS_FULL_TARGET` (raw bodies go to `subprocess.log`).
fn build_trace_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::TRACE.is_active() {
        return None;
    }
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env_lossy();
    let exclude_full = target_filter(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(TraceMakeWriter)
        .with_ansi(false)
        .event_format(TraceFileFormat)
        .with_filter(env_filter.and(exclude_full));
    Some(layer)
}

/// `trace.jsonl` layer: every event *except* the two subprocess-output targets,
/// rendered one JSON object per line at the Debug baseline (`RUST_LOG`
/// overrides, matching `trace.log`). Active only when `-vv` opened the file.
///
/// The exclusions are the raw bodies (`SUBPROCESS_FULL_TARGET`, which belong in
/// `subprocess.log`) and the bounded preview (`SUBPROCESS_BOUNDED_TARGET`, the
/// capped gist of that same output) — `trace.jsonl` is the *event* stream, so
/// command output stays out and is reached via `seq` into `subprocess.log`.
/// Everything else flows in: `[wt-trace]` records serialize rich, and free-form
/// `log::*` lines serialize as `{"message":...}` until structured.
fn build_trace_jsonl_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::TRACE_JSONL.is_active() {
        return None;
    }
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env_lossy();
    let exclude_output = target_filter(|meta| {
        meta.target() != SUBPROCESS_FULL_TARGET && meta.target() != SUBPROCESS_BOUNDED_TARGET
    });
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(TraceJsonlMakeWriter)
        .with_ansi(false)
        .event_format(TraceJsonlFormat)
        .with_filter(env_filter.and(exclude_output));
    Some(layer)
}

/// `subprocess.log` layer: only `SUBPROCESS_FULL_TARGET` records (raw bodies
/// and their `$ cmd … seq=N` headers), written through verbatim.
fn build_subprocess_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::SUBPROCESS.is_active() {
        return None;
    }
    let only_full = target_filter(|meta| meta.target() == SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(SubprocessMakeWriter)
        .with_ansi(false)
        .event_format(SubprocessFileFormat)
        .with_filter(only_full);
    Some(layer)
}

/// Print a one-line stderr pointer at `-vv` to the log directory, so users
/// know where the noisy log pipeline output went. Silent if `trace.log`
/// couldn't be opened (outside a git repo, permission error) — there's nothing
/// meaningful to point at.
///
/// Only the directory is named here, not the individual files: at init the
/// streamed sinks (`trace.log`, `trace.jsonl`, `subprocess.log`) exist but the
/// derived reports don't (`diagnostic.md` is written at exit), so the directory
/// is the one stable anchor. The end-of-run announcement (`write_if_verbose`)
/// owns the per-file story — it points at `diagnostic.md`, the single
/// human-facing doc.
fn announce_trace_destination() {
    let Some(trace_path) = log_files::TRACE.path() else {
        return;
    };
    // trace.log is always at `<git>/wt/logs/trace.log` (see `log_files::try_create`),
    // so the parent is structurally guaranteed.
    let dir = trace_path.parent().expect("trace.log path has a parent");
    // `format_path_for_display` emits forward slashes on every platform; the
    // trailing slash marks the path as a directory.
    let dir_display = worktrunk::path::format_path_for_display(dir);
    eprintln!(
        "{}",
        info_message(format!("Verbose logging to {dir_display}/"))
    );
}

#[cfg(test)]
mod tests {
    use ansi_str::AnsiStr;

    use super::{
        WT_TRACE_TARGET, WtTraceFields, effective_log_max_level, event_json, format_wt_trace,
        label_for_thread_index, parse_verbose_level, style_stderr_line,
    };

    /// The level-hint footgun this module's `target_filter` exists to dodge:
    /// a bare `filter_fn` reports no `max_level_hint`, and `Filter::and`
    /// collapses to `None` (its `cmp::min` sorts `None` below any `Some`),
    /// erasing the level bound. A `None` hint propagates to the global
    /// `LevelFilter`, pinning it at TRACE so every `tracing::debug!` builds
    /// its fields even at `-v0`. `target_filter` tags the predicate with a
    /// TRACE hint so the `LevelFilter`/`EnvFilter` side survives the `and`.
    #[test]
    fn target_filter_preserves_level_bound_through_and() {
        use tracing::level_filters::LevelFilter;
        use tracing_subscriber::Registry;
        use tracing_subscriber::filter::{FilterExt, filter_fn};
        use tracing_subscriber::layer::Filter;

        // The tagged target filter passes all levels (TRACE), but ANDing it
        // onto an `OFF` level filter keeps the `OFF` bound.
        let target = super::target_filter(|meta| meta.target() != "x");
        assert_eq!(
            Filter::<Registry>::max_level_hint(&target),
            Some(LevelFilter::TRACE)
        );
        let bounded = LevelFilter::OFF.and(target);
        assert_eq!(
            Filter::<Registry>::max_level_hint(&bounded),
            Some(LevelFilter::OFF),
            "the OFF bound must survive ANDing with the target filter"
        );

        // Regression guard: the bare `filter_fn` form collapses the AND hint
        // to `None` — the exact behavior that pinned the global level at TRACE.
        let bare = filter_fn(|meta| meta.target() != "x");
        let collapsed = LevelFilter::OFF.and(bare);
        assert_eq!(
            Filter::<Registry>::max_level_hint(&collapsed),
            None,
            "documents the footgun target_filter fixes"
        );
    }

    /// Branch coverage for `label_for_thread_index` — `thread_label` never
    /// hands it `n == 0` or `n > 52` in practice (Rust's `ThreadId`
    /// numbering starts at 1 and the main process won't spawn 53+ threads
    /// during the lifetime of the logger), but the branches are there for
    /// the day either invariant changes.
    #[test]
    fn label_covers_each_branch() {
        assert_eq!(label_for_thread_index(None), '?');
        assert_eq!(label_for_thread_index(Some(0)), '0');
        assert_eq!(label_for_thread_index(Some(1)), 'a');
        assert_eq!(label_for_thread_index(Some(26)), 'z');
        assert_eq!(label_for_thread_index(Some(27)), 'A');
        assert_eq!(label_for_thread_index(Some(52)), 'Z');
        assert_eq!(label_for_thread_index(Some(53)), '?');
        assert_eq!(label_for_thread_index(Some(9999)), '?');
    }

    /// Each shape `StderrFormat` recognises — verified ANSI-stripped so
    /// the assertions don't tangle with `cformat!`'s exact escape bytes.
    #[test]
    fn style_stderr_covers_each_shape() {
        let stripped = |t, msg| style_stderr_line(t, msg).ansi_strip().into_owned();

        // `$ cmd [ctx]` — git path with worktree context.
        assert_eq!(
            stripped('a', "$ git status [feature]"),
            "[a] $ git status [feature]"
        );

        // `$ cmd` with no `[ctx]` — standalone tools (gh, glab) emit this shape.
        assert_eq!(stripped('b', "$ gh pr list"), "[b] $ gh pr list");

        // `✓ cmd [ctx]  dur` — a finish line pairs with its `$` start line
        // (same `cmd [ctx]` rendering, command bolded under the new glyph).
        assert_eq!(
            stripped('c', "✓ git status [feature]  12.3ms"),
            "[c] ✓ git status [feature]  12.3ms"
        );

        // `✗ cmd [ctx]  dur  err` — a failed command; the duration and error
        // tail stay unbolded (command boundary is the earliest of ` [` / `  `).
        assert_eq!(
            stripped('d', "✗ git rev-list [.]  100ms  fatal: bad revision"),
            "[d] ✗ git rev-list [.]  100ms  fatal: bad revision"
        );

        // `✓ cmd  dur` with no `[ctx]` — boundary falls on the `  ` before dur.
        assert_eq!(
            stripped('e', "✓ gh pr list  45ms"),
            "[e] ✓ gh pr list  45ms"
        );

        // `  ! …` — subprocess stderr continuation, red-styled.
        assert_eq!(
            stripped('f', "  ! fatal: bad ref"),
            "[f]   ! fatal: bad ref"
        );

        // `· event` (instant) and plain text fall through with just the prefix.
        assert_eq!(stripped('g', "· Showed skeleton"), "[g] · Showed skeleton");
        assert_eq!(stripped('h', "hello"), "[h] hello");
    }

    /// Drive `WtTraceFields::Visit` end-to-end via a temporary subscriber:
    /// emit one event per field-type variant under [`WT_TRACE_TARGET`],
    /// capture the visitor output, and assert the field landed in the
    /// expected slot. Covers `record_debug` (`err = %display`) — the
    /// production path nothing else exercises — plus the unknown-name `_`
    /// arms in `record_u64` / `record_str`.
    #[test]
    fn wt_trace_fields_visit_records_every_type() {
        use std::sync::{Arc, Mutex};

        use tracing::Subscriber;
        use tracing_subscriber::Registry;
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

        struct Capture(Arc<Mutex<Vec<WtTraceFields>>>);
        impl<S: Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                if event.metadata().target() != WT_TRACE_TARGET {
                    return;
                }
                let mut fields = WtTraceFields::default();
                event.record(&mut fields);
                self.0.lock().unwrap().push(fields);
            }
        }
        let events: Arc<Mutex<Vec<WtTraceFields>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Capture(events.clone()));
        tracing::subscriber::with_default(subscriber, || {
            // u64 (dur_us) + ignored u64 (ts/tid are machine-only now, dropped
            // by the human visitor's non-`dur_us` arm)
            tracing::debug!(
                target: WT_TRACE_TARGET,
                dur_us = 12300u64,
                ts = 7u64,
            );
            // bool (ok)
            tracing::debug!(target: WT_TRACE_TARGET, ok = true);
            // str (cmd) + unknown_str → `_` arm in record_string
            tracing::debug!(
                target: WT_TRACE_TARGET,
                cmd = "git status",
                unknown_str = "ignored",
            );
            // Display-formatted value (err = %expr) → record_debug
            let msg = "fatal: bad ref".to_string();
            tracing::debug!(target: WT_TRACE_TARGET, err = %msg);
        });

        let captured = events.lock().unwrap();
        assert_eq!(captured[0].dur_us, Some(12300));
        assert_eq!(captured[1].ok, Some(true));
        assert_eq!(captured[2].cmd.as_deref(), Some("git status"));
        assert_eq!(captured[3].err.as_deref(), Some("fatal: bad ref"));
    }

    /// Lock the humanized line `format_wt_trace` produces for each `kind`.
    /// This is the human `trace.log` / stderr render; the machine grammar
    /// lives in `trace.jsonl` (`event_json`), parsed by `src/trace/parse.rs`.
    #[test]
    fn format_wt_trace_renders_each_kind() {
        // cmd_completed with context (ok=true → ✓), duration compacted by
        // Duration's Debug (12300µs → 12.3ms).
        let f = WtTraceFields {
            kind: Some("cmd_completed".into()),
            context: Some("worktree".into()),
            cmd: Some("git status".into()),
            dur_us: Some(12300),
            ok: Some(true),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "✓ git status [worktree]  12.3ms");

        // cmd_completed without context (ok=false → ✗)
        let f = WtTraceFields {
            kind: Some("cmd_completed".into()),
            cmd: Some("gh pr list".into()),
            dur_us: Some(45200),
            ok: Some(false),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "✗ gh pr list  45.2ms");

        // cmd_errored with context — the error message tails the line
        let f = WtTraceFields {
            kind: Some("cmd_errored".into()),
            context: Some("main".into()),
            cmd: Some("git merge-base".into()),
            dur_us: Some(100000),
            err: Some("fatal: no merge base".into()),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            "✗ git merge-base [main]  100ms  fatal: no merge base"
        );

        // cmd_errored without context (standalone tools like gh)
        let f = WtTraceFields {
            kind: Some("cmd_errored".into()),
            cmd: Some("gh pr list".into()),
            dur_us: Some(1000),
            err: Some("network down".into()),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "✗ gh pr list  1ms  network down");

        // instant (milestone) — `·`, no duration
        let f = WtTraceFields {
            kind: Some("instant".into()),
            event: Some("Showed skeleton".into()),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "· Showed skeleton");

        // span (in-process) — `◷` with duration
        let f = WtTraceFields {
            kind: Some("span".into()),
            span: Some("build_hook_context".into()),
            dur_us: Some(8200),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "◷ build_hook_context  8.2ms");

        // Defensive fallback: a future/unknown kind renders a visible line
        // rather than silently vanishing.
        let f = WtTraceFields {
            kind: Some("future_kind".into()),
            ..Default::default()
        };
        assert_eq!(format_wt_trace(&f), "· future_kind");
        assert_eq!(format_wt_trace(&WtTraceFields::default()), "· <unknown>");
    }

    /// `event_json` serializes whatever typed fields an event carries, then
    /// stamps `level` and fills `ts`/`tid` only when the event omitted them.
    /// Drives real events through a capture layer (the only way to obtain a
    /// `tracing::Event`). Keys land in serde_json's sorted order.
    #[test]
    fn event_json_serializes_structured_and_freeform_events() {
        use std::sync::{Arc, Mutex};

        use tracing::Subscriber;
        use tracing_subscriber::Registry;
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

        struct Capture(Arc<Mutex<Vec<String>>>);
        impl<S: Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                self.0.lock().unwrap().push(event_json(event));
            }
        }
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Capture(lines.clone()));
        tracing::subscriber::with_default(subscriber, || {
            // A full `[wt-trace]` cmd record: every field typed, ts/tid present.
            tracing::debug!(
                target: WT_TRACE_TARGET,
                kind = "cmd_completed",
                ts = 100u64,
                tid = 3u64,
                seq = 1u64,
                context = "worktree",
                cmd = "git status",
                dur_us = 12300u64,
                ok = true,
            );
            // The escaping win: a literal `"` in the command, where the
            // `key="value"` text grammar would truncate the parse. The
            // negative `exit` also drives `JsonFieldVisitor::record_i64`.
            tracing::debug!(
                target: WT_TRACE_TARGET,
                kind = "cmd_completed",
                ts = 100u64,
                tid = 3u64,
                seq = 5u64,
                exit = -1i64,
                cmd = r#"git commit -m "wip""#,
                dur_us = 1u64,
                ok = true,
            );
            // A free-form message with no fields and no ts/tid of its own.
            tracing::warn!("fsmonitor: skipping force-kill");
        });
        let lines = lines.lock().unwrap();

        // Structured record: rich, keys sorted, `level` stamped, ts/tid kept.
        assert_eq!(
            lines[0],
            r#"{"cmd":"git status","context":"worktree","dur_us":12300,"kind":"cmd_completed","level":"debug","ok":true,"seq":1,"tid":3,"ts":100}"#
        );

        // Embedded quote round-trips losslessly.
        let parsed: serde_json::Value = serde_json::from_str(&lines[1]).expect("valid JSON");
        assert_eq!(parsed["cmd"], r#"git commit -m "wip""#);
        assert_eq!(parsed["seq"], 5);
        assert_eq!(parsed["exit"], -1); // exercises record_i64

        // Free-form message: falls through as `{"message":...}`, with `level`
        // stamped and ts/tid injected from the runtime (so present, any value).
        let parsed: serde_json::Value = serde_json::from_str(&lines[2]).expect("valid JSON");
        assert_eq!(parsed["message"], "fsmonitor: skipping force-kill");
        assert_eq!(parsed["level"], "warn");
        assert!(parsed["ts"].is_u64() && parsed["tid"].is_u64());
        assert!(parsed.get("kind").is_none());
    }

    /// `effective_log_max_level` mirrors the layer filters: env wins when
    /// set, else the verbosity baseline. Driving it as a pure function
    /// lets us cover the env-set branch without mutating the process env
    /// (which races with parallel tests).
    #[test]
    fn effective_log_max_level_env_wins_when_set() {
        use log::LevelFilter::*;
        assert_eq!(effective_log_max_level(0, None), Warn);
        assert_eq!(effective_log_max_level(1, None), Info);
        assert_eq!(effective_log_max_level(2, None), Debug);
        // Env raises:
        assert_eq!(effective_log_max_level(0, Some(Debug)), Debug);
        // Env lowers (the env-wins-when-set contract — env can also
        // suppress, not just raise):
        assert_eq!(effective_log_max_level(2, Some(Warn)), Warn);
    }

    /// `WORKTRUNK_VERBOSE` parses like the `-v`/`-vv` count. Anything that
    /// isn't a clean integer (including the empty string a bare `export`
    /// leaves) falls back to `0` rather than erroring — a panic here would
    /// corrupt the completion candidate list.
    #[test]
    fn parse_verbose_level_is_lossy() {
        assert_eq!(parse_verbose_level(None), 0);
        assert_eq!(parse_verbose_level(Some("")), 0);
        assert_eq!(parse_verbose_level(Some("0")), 0);
        assert_eq!(parse_verbose_level(Some("1")), 1);
        assert_eq!(parse_verbose_level(Some("2")), 2);
        assert_eq!(parse_verbose_level(Some(" 2 ")), 2);
        // Higher counts pass through (treated like `-vvv`, which the layer
        // builders already collapse to the `>= 2` behavior).
        assert_eq!(parse_verbose_level(Some("3")), 3);
        // Garbage and out-of-range values are dropped, not errored.
        assert_eq!(parse_verbose_level(Some("abc")), 0);
        assert_eq!(parse_verbose_level(Some("-1")), 0);
        assert_eq!(parse_verbose_level(Some("999")), 0);
    }
}
