//! Tracing-subscriber setup for the `wt` binary.
//!
//! Three layered subscribers cooperate to give each verbosity level the
//! routing it needs. Filtering is structural (per-layer `Filter`), not done
//! after-the-fact in a format closure:
//!
//! | layer            | filter                                              | format            |
//! | ---------------- | --------------------------------------------------- | ----------------- |
//! | stderr           | `$RUST_LOG` or flag baseline (`Off`/`Info`/`Info`)  | styled with ANSI  |
//! | `trace.log`      | `-vv` only, excludes `SUBPROCESS_FULL_TARGET`       | plain text        |
//! | `subprocess.log` | `-vv` only, includes only `SUBPROCESS_FULL_TARGET`  | raw (no prefix)   |
//!
//! At `-vv` the stderr layer keeps its Info baseline — `-vv` is a strict
//! superset of `-v`, with Debug-level records (the noisy ones, including
//! the bounded subprocess preview) routed to the file layers only.
//!
//! The `log` crate calls (used throughout the codebase) are bridged into
//! `tracing` by [`tracing_log::LogTracer::init`] — every layer above sees
//! both native `tracing::*` events and forwarded `log::*` records.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

use color_print::cformat;
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, FilterExt, LevelFilter, filter_fn};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use worktrunk::shell_exec::SUBPROCESS_FULL_TARGET;
use worktrunk::styling::{eprintln, info_message};
use worktrunk::trace::WT_TRACE_TARGET;
use worktrunk::utils::escape_controls;

use crate::log_files::{self, SubprocessMakeWriter, TraceMakeWriter};
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
/// branches (`$ cmd [ctx]`, `$ cmd`, `  ! err`, plain) can be unit-tested
/// without standing up a `tracing` subscriber.
fn style_stderr_line(thread_num: char, msg: &str) -> String {
    if let Some(rest) = msg.strip_prefix("$ ") {
        // Standalone tools (gh, glab) emit no `[ctx]` suffix.
        let (command, worktree) = match rest.find(" [") {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, ""),
        };
        cformat!("<dim>[{thread_num}]</> $ <bold>{command}</>{worktree}")
    } else if msg.starts_with("  ! ") {
        cformat!("<dim>[{thread_num}]</> <red>{msg}</>")
    } else {
        cformat!("<dim>[{thread_num}]</> {msg}")
    }
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
        let msg = render_event_message(event);
        let line = style_stderr_line(thread_label(), &msg);
        writeln!(writer, "{line}")
    }
}

/// Render an event to its single-line text payload — `[wt-trace]` grammar
/// for events under [`WT_TRACE_TARGET`], the raw `message` field for
/// everything else. Shared between the stderr and `trace.log` formatters so
/// `[wt-trace]` records appear in both routes (`-vv` writes to the file;
/// `RUST_LOG=debug -v` surfaces them on stderr).
///
/// Control bytes are escaped here ([`escape_controls`]) — this is the single
/// chokepoint feeding both human-facing routes, so raw NUL/ESC from subprocess
/// output (e.g. the bounded preview of `git … -z`, or a `cmd=`/`err=` field
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
/// per event. Matches the on-disk layout pre-migration.
///
/// Events under [`WT_TRACE_TARGET`] are rendered via the dedicated
/// `[wt-trace] key=value` grammar in [`format_wt_trace`]; everything else
/// falls through to the legacy message-prefix shape. This is the only place
/// the `[wt-trace]` text format lives — emit sites in `trace::emit` carry
/// structured fields, not pre-formatted strings.
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

/// Captured fields from a single `WT_TRACE_TARGET` event. The visitor reads
/// each field by name and stores its value typed — the layer renderer then
/// composes the final `[wt-trace] …` line in the exact field order
/// downstream parsers expect.
///
/// Unknown fields are dropped; the wt-trace grammar is closed (every key
/// has a fixed meaning).
#[derive(Default)]
struct WtTraceFields {
    kind: Option<String>,
    ts: Option<u64>,
    tid: Option<u64>,
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
        match field.name() {
            "ts" => self.ts = Some(value),
            "tid" => self.tid = Some(value),
            "dur_us" => self.dur_us = Some(value),
            _ => {}
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

/// Render structured fields as the `[wt-trace] key=value …` text wt-perf
/// and the integration tests parse. The field order per `kind` is the
/// contract; see `src/trace/parse.rs` for the consumer.
///
/// A malformed event (missing required fields, unknown `kind`) renders the
/// best-effort string `[wt-trace] kind=<…> <repr>` so a future-added kind
/// produces a noticeable but parseable line rather than silently vanishing.
fn format_wt_trace(f: &WtTraceFields) -> String {
    // `ts` and `tid` are required for every kind; default to 0 to keep the
    // line shape valid if a future emit site forgets one — the parser will
    // still accept it, and the missing field shows up as `0` in the
    // timeline rather than disappearing.
    let ts = f.ts.unwrap_or(0);
    let tid = f.tid.unwrap_or(0);

    match f.kind.as_deref() {
        Some("cmd_completed") => {
            let cmd = f.cmd.as_deref().unwrap_or("");
            let dur_us = f.dur_us.unwrap_or(0);
            let ok = f.ok.unwrap_or(false);
            match &f.context {
                Some(ctx) => format!(
                    r#"[wt-trace] ts={ts} tid={tid} context={ctx} cmd="{cmd}" dur_us={dur_us} ok={ok}"#
                ),
                None => {
                    format!(r#"[wt-trace] ts={ts} tid={tid} cmd="{cmd}" dur_us={dur_us} ok={ok}"#)
                }
            }
        }
        Some("cmd_errored") => {
            let cmd = f.cmd.as_deref().unwrap_or("");
            let dur_us = f.dur_us.unwrap_or(0);
            let err = f.err.as_deref().unwrap_or("");
            match &f.context {
                Some(ctx) => format!(
                    r#"[wt-trace] ts={ts} tid={tid} context={ctx} cmd="{cmd}" dur_us={dur_us} err="{err}""#
                ),
                None => format!(
                    r#"[wt-trace] ts={ts} tid={tid} cmd="{cmd}" dur_us={dur_us} err="{err}""#
                ),
            }
        }
        Some("instant") => {
            let event = f.event.as_deref().unwrap_or("");
            format!(r#"[wt-trace] ts={ts} tid={tid} event="{event}""#)
        }
        Some("span") => {
            let name = f.span.as_deref().unwrap_or("");
            let dur_us = f.dur_us.unwrap_or(0);
            format!(r#"[wt-trace] ts={ts} tid={tid} span="{name}" dur_us={dur_us}"#)
        }
        other => format!(
            r#"[wt-trace] ts={ts} tid={tid} kind={kind:?}"#,
            kind = other.unwrap_or("<unknown>")
        ),
    }
}

/// `subprocess.log` formatter: the message verbatim. Subprocess bodies are
/// already prefixed (`  …` / `  ! …`) by `shell_exec::format_stream_full`.
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
    let subprocess_layer = build_subprocess_layer(verbose_level);

    // `try_init` fails only if a subscriber is already installed (the
    // single-call-per-process contract). `wt`'s `main` runs `logging::init`
    // exactly once, so the error is just defensive — discard it. The
    // `LogTracer::init` below has the same shape for the same reason.
    let _ = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(trace_layer)
        .with(subprocess_layer)
        .try_init();

    // Forward `log::*` macros into `tracing`. Must come after subscriber
    // init: `LogTracer::enabled` consults the tracing dispatcher.
    //
    // The builder's `with_max_level` caps `log::max_level()` — the static
    // gate `log_enabled!` checks before format args are evaluated. Mirror
    // the env-wins-when-set semantics the layer filters use (PR #2901):
    // if `RUST_LOG` is set, its level wins; otherwise the verbosity flag
    // baseline applies. Without an explicit cap, the default
    // `LevelFilter::max()` would always pass the static check, forcing
    // every `log::debug!(…)` site to evaluate its format args — exposing
    // arithmetic that's safe today only because the macro short-circuits
    // (e.g. `now_secs - cached.checked_at` in `list/ci_status` is fine
    // under monotonic-ish clocks but panics when args are evaluated
    // against a clock-skewed fixture).
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
    let exclude_full = filter_fn(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
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
    let exclude_full = filter_fn(|meta| meta.target() != SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(TraceMakeWriter)
        .with_ansi(false)
        .event_format(TraceFileFormat)
        .with_filter(env_filter.and(exclude_full));
    Some(layer)
}

/// `subprocess.log` layer: only `SUBPROCESS_FULL_TARGET` records, raw passthrough.
fn build_subprocess_layer<S>(verbose_level: u8) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if verbose_level < 2 || !log_files::SUBPROCESS.is_active() {
        return None;
    }
    let only_full = filter_fn(|meta| meta.target() == SUBPROCESS_FULL_TARGET);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(SubprocessMakeWriter)
        .with_ansi(false)
        .event_format(SubprocessFileFormat)
        .with_filter(only_full);
    Some(layer)
}

/// Print a one-line stderr pointer at `-vv` so users know where the noisy
/// log pipeline output went. Silent if `trace.log` couldn't be opened
/// (outside a git repo, permission error) — there's nothing meaningful to
/// point at.
fn announce_trace_destination() {
    // TRACE and SUBPROCESS open independently — `LogSink::init` succeeds per
    // file. The (Some, None) case (trace.log open, subprocess.log failed) is
    // rare but real (path-type mismatch, fs quota); the reverse is
    // possible too but `subprocess.log` alone has no `$ cmd` context, so we
    // stay silent there. `diagnostic.md` is named even though it's written
    // at exit (not init) — by the time the user reads the pointer and looks
    // for files, all three will be there.
    let Some(trace_path) = log_files::TRACE.path() else {
        return;
    };
    // trace.log is always at `<git>/wt/logs/trace.log` (see `log_files::try_create`),
    // so the parent is structurally guaranteed.
    let dir = trace_path.parent().expect("trace.log path has a parent");
    let dir_display = worktrunk::path::format_path_for_display(dir);
    let msg = match log_files::SUBPROCESS.path() {
        Some(_) => cformat!(
            "Writing to <underline>{dir_display}/</> — trace.log, subprocess.log, diagnostic.md"
        ),
        None => cformat!(
            "Writing to <underline>{dir_display}/</> — trace.log, diagnostic.md (subprocess.log unavailable)"
        ),
    };
    eprintln!("{}", info_message(msg));
}

#[cfg(test)]
mod tests {
    use ansi_str::AnsiStr;

    use super::{
        WT_TRACE_TARGET, WtTraceFields, effective_log_max_level, format_wt_trace,
        label_for_thread_index, parse_verbose_level, style_stderr_line,
    };

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
        // `$ cmd [ctx]` — git path with worktree context.
        let cmd_ctx = style_stderr_line('a', "$ git status [feature]")
            .ansi_strip()
            .into_owned();
        assert_eq!(cmd_ctx, "[a] $ git status [feature]");

        // `$ cmd` with no `[ctx]` — standalone tools (gh, glab) emit this
        // shape; was line 109 in the codecov gap.
        let cmd_no_ctx = style_stderr_line('b', "$ gh pr list")
            .ansi_strip()
            .into_owned();
        assert_eq!(cmd_no_ctx, "[b] $ gh pr list");

        // `  ! …` — subprocess stderr continuation, red-styled.
        let err = style_stderr_line('c', "  ! fatal: bad ref")
            .ansi_strip()
            .into_owned();
        assert_eq!(err, "[c]   ! fatal: bad ref");

        // Plain — everything else falls through with just the thread prefix.
        let plain = style_stderr_line('d', "hello").ansi_strip().into_owned();
        assert_eq!(plain, "[d] hello");
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
            // u64 (ts/tid/dur_us) + unknown_u64 → `_` arm in record_u64
            tracing::debug!(
                target: WT_TRACE_TARGET,
                ts = 7u64,
                tid = 3u64,
                unknown_u64 = 42u64,
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
        assert_eq!(captured[0].ts, Some(7));
        assert_eq!(captured[0].tid, Some(3));
        assert_eq!(captured[1].ok, Some(true));
        assert_eq!(captured[2].cmd.as_deref(), Some("git status"));
        assert_eq!(captured[3].err.as_deref(), Some("fatal: bad ref"));
    }

    /// Lock the `[wt-trace]` wire grammar produced by `format_wt_trace`.
    /// wt-perf and the integration suite parse these lines; any drift here
    /// breaks downstream tooling, so each `kind` gets a fixture assertion.
    #[test]
    fn format_wt_trace_renders_each_kind() {
        // cmd_completed with context
        let f = WtTraceFields {
            kind: Some("cmd_completed".into()),
            ts: Some(100),
            tid: Some(3),
            context: Some("worktree".into()),
            cmd: Some("git status".into()),
            dur_us: Some(12300),
            ok: Some(true),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 context=worktree cmd="git status" dur_us=12300 ok=true"#
        );

        // cmd_completed without context
        let f = WtTraceFields {
            kind: Some("cmd_completed".into()),
            ts: Some(100),
            tid: Some(3),
            cmd: Some("gh pr list".into()),
            dur_us: Some(45200),
            ok: Some(false),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 cmd="gh pr list" dur_us=45200 ok=false"#
        );

        // cmd_errored with context
        let f = WtTraceFields {
            kind: Some("cmd_errored".into()),
            ts: Some(100),
            tid: Some(3),
            context: Some("main".into()),
            cmd: Some("git merge-base".into()),
            dur_us: Some(100000),
            err: Some("fatal: ...".into()),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 context=main cmd="git merge-base" dur_us=100000 err="fatal: ...""#
        );

        // cmd_errored without context (standalone tools like gh)
        let f = WtTraceFields {
            kind: Some("cmd_errored".into()),
            ts: Some(100),
            tid: Some(3),
            cmd: Some("gh pr list".into()),
            dur_us: Some(1000),
            err: Some("network down".into()),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 cmd="gh pr list" dur_us=1000 err="network down""#
        );

        // instant
        let f = WtTraceFields {
            kind: Some("instant".into()),
            ts: Some(100),
            tid: Some(3),
            event: Some("Showed skeleton".into()),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 event="Showed skeleton""#
        );

        // span
        let f = WtTraceFields {
            kind: Some("span".into()),
            ts: Some(100),
            tid: Some(3),
            span: Some("build_hook_context".into()),
            dur_us: Some(8200),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 span="build_hook_context" dur_us=8200"#
        );

        // Defensive fallback: a future kind not yet known to the renderer
        // emits a parseable record rather than silently vanishing.
        let f = WtTraceFields {
            kind: Some("future_kind".into()),
            ts: Some(100),
            tid: Some(3),
            ..Default::default()
        };
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=100 tid=3 kind="future_kind""#
        );
        let f = WtTraceFields::default();
        assert_eq!(
            format_wt_trace(&f),
            r#"[wt-trace] ts=0 tid=0 kind="<unknown>""#
        );
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
