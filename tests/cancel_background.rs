//! Cancellation of background commands (`worktrunk::shell_exec`).
//!
//! This lives in its own test binary because `cancel_background_commands`
//! latches process-wide state and signals every background PID in the process.
//! Folded into `integration`, it would take out whatever ran beside it —
//! `TestRepo`'s helpers drive git through the same `Cmd` path — and under a
//! shared-process runner that is a real failure, not a theoretical one. One
//! test, alone in its process, has nothing to collide with.

#![cfg(unix)]

use std::io::ErrorKind;
use std::time::{Duration, Instant};

use worktrunk::shell_exec::{Cmd, cancel_background_commands, uninterruptible};

/// Cancelling reaches a command that is already running and refuses one that
/// has not started yet — the two halves of the guarantee, since a sweep over
/// live PIDs can't see a task still queued behind the command semaphore — while
/// leaving an `uninterruptible` thread's commands alone in both directions.
///
/// The first half is also what pins PID registration: the sweep can only reach
/// a running command that registered itself, so if registration regressed this
/// test's `sleep` would run to completion. Asserting on the registry directly
/// instead would be weaker — it is process-global, so under a shared-process
/// runner the assertion can be satisfied by some other test's command.
///
/// Every command runs on a spawned thread: the foreground thread is exempt (it
/// is the one doing the cancelling), and with `FOREGROUND_THREAD` unset under a
/// test harness, every other thread counts as background.
#[test]
fn cancel_stops_speculative_commands_and_spares_uninterruptible_ones() {
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("started");
    let marker = started.display().to_string();

    // `exec` so the sleep *is* the process we spawned. Left as a child of the
    // shell, it would survive the signal to its parent and keep the captured
    // stdout pipe open, blocking the wait for the full 30s.
    let running = std::thread::spawn(move || {
        Cmd::new("sh")
            .arg("-c")
            .arg(format!("touch '{marker}'; exec sleep 30"))
            .run()
    });

    // Wait for the child to be genuinely running, so the sweep has a live PID
    // to find rather than racing the spawn.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::sleep(Duration::from_millis(10));
    }

    // A mutation the user already asked for — an `alt-x` removal — runs on a
    // thread that opts out, and must survive the sweep rather than be killed
    // between two git steps.
    let protected_marker = dir.path().join("protected");
    let protected_path = protected_marker.display().to_string();
    let protected = std::thread::spawn(move || {
        uninterruptible(|| {
            Cmd::new("sh")
                .arg("-c")
                .arg(format!("touch '{protected_path}'; exec sleep 2"))
                .run()
        })
    });
    while !protected_marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the exempt command never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let cancelled_at = Instant::now();
    cancel_background_commands();

    let output = running
        .join()
        .unwrap()
        .expect("a signalled command still yields its Output");
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(25),
        "the sleep ran to completion instead of being signalled"
    );
    assert!(
        !output.status.success(),
        "a signalled command should report failure"
    );

    // Anything starting afterwards is refused before it spawns a process.
    let refused = std::thread::spawn(|| Cmd::new("sh").arg("-c").arg("sleep 30").run())
        .join()
        .unwrap();
    assert_eq!(
        refused
            .expect_err("a cancelled command must not spawn")
            .kind(),
        ErrorKind::Interrupted
    );

    // Same for the two-process pipeline path, which spawns on its own.
    let piped = std::thread::spawn(|| {
        Cmd::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .pipe_into(Cmd::new("cat"))
    })
    .join()
    .unwrap();
    assert_eq!(
        piped
            .expect_err("a cancelled pipeline must not spawn")
            .kind(),
        ErrorKind::Interrupted
    );

    // The exempt command ran to completion despite the sweep.
    let protected_output = protected
        .join()
        .unwrap()
        .expect("an uninterruptible command runs");
    assert!(
        protected_output.status.success(),
        "cancellation must not signal a command the user asked for"
    );

    // And the latch doesn't refuse its *later* commands either — a removal is
    // several git calls, not one, and the ones after the sweep must still run.
    let after = std::thread::spawn(|| uninterruptible(|| Cmd::new("true").run()))
        .join()
        .unwrap()
        .expect("an uninterruptible command still spawns after cancellation");
    assert!(after.status.success());
}
