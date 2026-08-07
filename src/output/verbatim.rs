//! Stdout for output whose ANSI is already decided.

/// Print a line to stdout byte-for-byte, exiting cleanly when the consumer
/// stops reading.
///
/// The companion to [`print_json`](super::print_json) for the output whose
/// pipe is a courier rather than the destination, so the escapes in it are
/// data: the statusline, which a shell prompt or Claude Code captures and
/// renders, and the `--help-page` document, whose escapes the docs pipeline
/// converts into HTML spans. Neither consumer is ever a tty, so anstream's
/// `println!` would strip both every time — and no test would notice, since
/// the suite sets `CLICOLOR_FORCE=1`. std's macros keep the bytes but panic
/// when a consumer closes the pipe, which is how `wt list statusline | head -1`
/// came to exit 101 on the surface a prompt redraws on.
///
/// Output a person reads is not this: `wt list` and the rest go through
/// anstream, which strips color when stdout isn't a terminal unless
/// `CLICOLOR_FORCE` says otherwise, and strips it on a terminal too under
/// `NO_COLOR`. The `wt list` table's default terminal rendering is the one
/// path that never reaches anstream — `RenderTarget::detect` hands every tty
/// that didn't pass `--no-progressive` to `ProgressiveTable`, which writes to
/// a raw `std::io::stdout()` with each row's escapes already baked in.
///
/// A `BrokenPipe` is dropped and any other write error still panics — the
/// same policy anstream's macros apply, so both printers fail the same way on
/// a full disk.
macro_rules! println_verbatim {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let mut stdout = ::std::io::stdout().lock();
        match ::std::writeln!(stdout, $($arg)*) {
            Err(e) if e.kind() != ::std::io::ErrorKind::BrokenPipe => {
                ::std::panic!("failed printing to stdout: {e}");
            }
            Err(_) | Ok(_) => {}
        }
    }};
}

pub(crate) use println_verbatim;
