//! Guard test to prevent stdout leaks anywhere in the crate
//!
//! stdout carries the command's answer — content the user may want to pipe,
//! redirect, or capture: data (JSON, tables), rendered views (`wt config show`,
//! `wt hook show`), `--dry-run` previews (the whole answer when nothing
//! mutates), and shell integration output. Interactive status, progress, and
//! errors go to stderr. When shell integration is active (directive env vars
//! set), directives are written to files, not stdout.
//!
//! This test enforces: **No accidental stdout writes anywhere under `src/`**
//!
//! Allowed:
//! - `eprintln!` / `eprint!` — stderr is not the answer stream, so writing to
//!   it is safe; *which* macro of that name is in scope is a separate rule,
//!   enforced by [`check_stderr_macros_come_from_styling`]
//! - `println!` / `print!` in files listed in `STDOUT_ALLOWED_PATHS`
//!
//! When adding stdout output:
//! - Use `styling`'s `println` — `worktrunk::styling` from the binary's modules,
//!   `crate::styling` from the library's. Where the consumer renders the escapes
//!   and is never a tty (the statusline, `--help-page`), declare that once at
//!   the top of the command with `ColorChoice::Always.write_global()` — the
//!   global anstream consults before tty detection — and print normally.
//! - Add the file path to `STDOUT_ALLOWED_PATHS` with a comment explaining why
//!
//! anstream's macros don't panic when the consumer closes the pipe, which
//! [`test_stdout_surfaces_survive_a_closed_consumer`] checks from the outside.
//!
//! The stderr counterpart is two tests: [`check_stderr_macros_come_from_styling`]
//! requires every bare `eprint!`/`eprintln!` under `src/` to resolve to
//! anstream's, statically and for every site; and
//! [`test_stderr_narration_strips_ansi_when_piped`] proves from the outside
//! what that buys — narration redirected to a file carries no escapes.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;

use path_slash::PathExt as _;
use rstest::rstest;

use crate::common::source_scan::visit_files;
use crate::common::{TestRepo, repo};

/// Paths (relative to src/) that are allowed to use println!/print! for stdout.
/// These intentionally output data to stdout for scripting/piping.
const STDOUT_ALLOWED_PATHS: &[&str] = &[
    // Shell integration code for: eval "$(wt config shell init bash)"
    "commands/init.rs",
    // Table and summary output for wt list
    "commands/list/collect/mod.rs",
    // State data output (branch names, previous worktree, etc.)
    "commands/config/state.rs",
    // Hint list output
    "commands/config/hints.rs",
    // Alias introspection output (show / dry-run), intended to be pipeable
    "commands/config/alias.rs",
    // Template evaluation output for scripting
    "commands/eval.rs",
    // LLM prompt output for wt step commit --show-prompt and squash --show-prompt
    "commands/step/commit.rs",
    "commands/step/squash.rs",
    // wt step copy-ignored dry-run plan (human preview + --format=json)
    "commands/step/copy_ignored.rs",
    // wt step prune dry-run plan (human preview + --format=json)
    "commands/step/prune.rs",
    // wt step relocate dry-run human preview (show_dry_run_preview)
    "commands/relocate.rs",
    // wt config shell install/uninstall --dry-run preview (the interactive
    // `?` re-preview still goes to stderr)
    "commands/configure_shell.rs",
    // JSON output for wt switch --format=json
    "commands/worktree/switch.rs",
    // Migrated TOML output for wt config update --print (pipeable)
    "commands/config/update.rs",
    // Hook listing for wt hook show (paged), and the wt hook --dry-run preview
    "commands/hook_commands.rs",
    // The statusline itself — a single line a shell prompt or Claude Code captures
    "commands/statusline.rs",
    // The --format=json answer for every command that has one
    "output/json.rs",
    // The doc entry points (--help-page, --help-md, --help-description) and
    // --version: help text is the command's answer, on stdout by POSIX
    // convention, and a generator or `version=$(wt --version)` reads it
    "help.rs",
    // wt --help / -h itself, whenever no pager takes the text (short help, no
    // pager configured, stdout not a tty, or the pager failed to spawn)
    "help_pager.rs",
    // The mock commands the suite puts on PATH under names like gh and glab:
    // this stdout is the mocked tool's output, read by the wt under test
    "testing/mock_stub.rs",
];

/// Substrings that mark a line where the token is text rather than a call
/// (inside a string literal, a comment, or a reference to a test file)
const ALLOWED_LINE_PATTERNS: &[&str] = &[
    // The `println!` inside llm.rs's SYNTHETIC_DIFF string literal is a line of
    // the fake diff a commit-message prompt is built from, not a call
    r#"println!("Hello, world!");"#,
];

/// How to name the `styling` module, for a failure message a contributor pastes
/// straight back into the file that tripped it.
///
/// `src/` holds both crates: `src/lib.rs` declares the library's modules (`git`,
/// `config`, `shell_exec`, …) and `src/main.rs` the binary's (`cli`, `commands`,
/// `display`, …). `worktrunk::` resolves only in the second, so a message naming
/// it alone hands a library file code that cannot compile — and neither scan can
/// catch that, since both accept any call prefixed with `styling::`.
const STYLING_PATH_BY_CRATE: &str = "`worktrunk::styling` in the binary's modules (the ones `src/main.rs` declares), `crate::styling` in the library's (the ones `src/lib.rs` declares)";

/// No file under `src/` writes to stdout unless it is listed as a surface that
/// deliberately does. The scan covers the whole crate rather than just
/// `src/commands/`: `src/help.rs` carries the answer for `--help-page` and
/// `--version`, and while it was outside the scanned tree its std `println!`s
/// panicked on a closed consumer unnoticed.
#[test]
fn check_no_unexpected_stdout_writes() {
    let project_root = env!("CARGO_MANIFEST_DIR");
    let src_dir = Path::new(project_root).join("src");

    // Forbidden tokens that write to stdout
    let stdout_tokens = ["print!", "println!"];

    let mut violations = Vec::new();

    // Recursively scan all .rs files under src/
    let scanned = visit_files(&src_dir, "rs", "stdout scan", &mut |path, contents| {
        check_file(path, contents, &stdout_tokens, &mut violations, &src_dir)
    });
    assert!(
        scanned > 0,
        "scanned no files under {} — this test asserts absence, so it would have \
         passed over nothing",
        src_dir.display()
    );

    if !violations.is_empty() {
        panic!(
            "Unexpected stdout writes:\n\n{}\n\n\
             stdout is reserved for data output (JSON, tables).\n\
             Use the `println` from `styling` for stdout, and its `eprintln` — not color_print's ceprintln! — for stderr.\n\
             Spell the path for the crate the file belongs to: {STYLING_PATH_BY_CRATE}.\n\
             Add file path to STDOUT_ALLOWED_PATHS if stdout is intentional.",
            violations.join("\n")
        );
    }
}

fn check_file(
    path: &Path,
    contents: &str,
    tokens: &[&str],
    violations: &mut Vec<String>,
    scan_root: &Path,
) {
    // Get path relative to src/ for matching against STDOUT_ALLOWED_PATHS
    let relative_path = path
        .strip_prefix(scan_root)
        .map(|p| p.to_slash_lossy())
        .unwrap_or_default();

    // Skip files that are allowed to use stdout
    if STDOUT_ALLOWED_PATHS.contains(&relative_path.as_ref()) {
        return;
    }

    let relative_path = path
        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or(path)
        .display();

    for (line_num, line) in contents.lines().enumerate() {
        // Skip lines with allowed patterns
        if ALLOWED_LINE_PATTERNS
            .iter()
            .any(|pattern| line.contains(pattern))
        {
            continue;
        }

        for token in tokens {
            if let Some(pos) = line.find(token) {
                // The token also sits inside the stderr macros eprint!/eprintln!,
                // which are safe. Walk back over the identifier the token ends
                // and let that prefix through. color_print's ceprint!/ceprintln!
                // write through std's stderr directly, bypassing anstream's
                // resolved color choice, so they stay violations — as does a
                // longer identifier like some_eprintln!.
                let ident_start = line[..pos]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
                    .map_or(0, |(i, c)| i + c.len_utf8());
                if &line[ident_start..pos] == "e" {
                    continue;
                }

                // Skip if the token is in a comment
                if let Some(comment_pos) = line.find("//")
                    && comment_pos < pos
                {
                    continue;
                }

                violations.push(format!(
                    "{}:{}: {}",
                    relative_path,
                    line_num + 1,
                    line.trim()
                ));
                break; // Only report once per line
            }
        }
    }
}

/// Paths (relative to `src/`) allowed to reach std's `eprint!`/`eprintln!`.
///
/// Every other file that writes to stderr must have the macro of that name in
/// scope from `styling`, or qualify the call — see
/// [`check_stderr_macros_come_from_styling`], and [`STYLING_PATH_BY_CRATE`] for
/// which path spells it from where.
///
/// An entry exempts the **whole file**, not the call its comment names, so an
/// entry added for one narrow site also covers whatever that file grows later.
/// Keep the list to files where std's macro is right throughout.
const STD_STDERR_ALLOWED_PATHS: &[&str] = &[
    // Relays a stub's captured stderr verbatim; the bytes are the fixture's
    // data, so stripping their escapes would change what the test replays.
    "testing/mock_stub.rs",
    // A `#[cfg(test)]` skip diagnostic, not narration a user ever redirects.
    "remove_dir.rs",
];

/// Every allowlist entry names a file that still exists.
///
/// Both allowlists are matched by path string, so a rename leaves a dead entry
/// behind rather than failing. That is worse than clutter: the exemption stays
/// armed at the old path, so a *new* file arriving there — `src/help.rs` is the
/// shape, a name a future refactor could plausibly reuse — inherits a
/// permission nobody granted it, and the guard stays green while it writes to
/// stdout. Pinning existence turns the rename into a failing test at the moment
/// it happens, when the reviewer still knows whether the exemption should move
/// with the file or go.
#[test]
fn allowlisted_paths_still_exist() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let stale: Vec<String> = [
        ("STDOUT_ALLOWED_PATHS", STDOUT_ALLOWED_PATHS),
        ("STD_STDERR_ALLOWED_PATHS", STD_STDERR_ALLOWED_PATHS),
    ]
    .iter()
    .flat_map(|(list_name, paths)| {
        paths
            .iter()
            .filter(|relative| !src_dir.join(relative).is_file())
            .map(move |relative| format!("{list_name}: src/{relative}"))
    })
    .collect();

    assert!(
        stale.is_empty(),
        "allowlist entries naming files that no longer exist:\n\n{}\n\n\
         The entry exempts whatever file later occupies that path, so a stale one\n\
         silently pre-approves stdout (or std's stderr macros) for code nobody reviewed.\n\
         Move the entry to the file's new path, or drop it.",
        stale.join("\n")
    );
}

/// Narration on stderr goes out through anstream, at every site.
///
/// `worktrunk::styling`'s `eprint!`/`eprintln!` are anstream's and strip ANSI
/// when stderr isn't a terminal; std's prelude macros of the same name keep it.
/// The two are one import apart and indistinguishable at the call site, so a
/// file that imported `eprintln` but not `eprint` printed a warning and the
/// hint beneath it through different printers — one line of `wt list 2>log`
/// carrying escapes and the next not.
///
/// No runtime test can cover this: it is a property of every stderr write in
/// the binary, and the suite's own `CLICOLOR_FORCE=1` forces color on both
/// printers, so a snapshot agrees no matter which macro is in scope. Hence a
/// static scan. A call may satisfy it either way — importing the macro from
/// `styling`, or qualifying the call as `styling::eprintln!(…)`.
#[test]
fn check_stderr_macros_come_from_styling() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    let scanned = visit_files(
        &src_dir,
        "rs",
        "stderr-macro scan",
        &mut |path, contents| {
            check_stderr_macros_in_file(path, contents, &src_dir, &mut violations)
        },
    );
    assert!(
        scanned > 0,
        "scanned no files under {} — this test asserts absence, so it would have \
         passed over nothing",
        src_dir.display()
    );
    violations.sort();

    assert!(
        violations.is_empty(),
        "stderr writes that resolve to std's macro instead of anstream's:\n\n{}\n\n\
         std's `eprint!`/`eprintln!` keep ANSI escapes when stderr is redirected, so a\n\
         file mixing them with anstream's emits color on some lines of a log and not others.\n\
         Import the macro from `styling`, or qualify the call as `styling::eprintln!(…)`.\n\
         Spell the path for the crate the file belongs to: {STYLING_PATH_BY_CRATE}.\n\
         Or add the file to STD_STDERR_ALLOWED_PATHS with a comment explaining why\n\
         std's macro is the right one there.",
        violations.join("\n")
    );
}

fn check_stderr_macros_in_file(
    path: &Path,
    contents: &str,
    src_dir: &Path,
    violations: &mut Vec<String>,
) {
    let relative_path = path
        .strip_prefix(src_dir)
        .map(|p| p.to_slash_lossy())
        .unwrap_or_default();
    if STD_STDERR_ALLOWED_PATHS.contains(&relative_path.as_ref()) {
        return;
    }

    let imported = styling_imports(contents);

    for (line_num, line) in contents.lines().enumerate() {
        // A doc comment quoting `eprintln!` is prose, not a write.
        let code = match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        };

        for macro_name in ["eprint", "eprintln"] {
            if imported.contains(macro_name) {
                continue;
            }
            let token = format!("{macro_name}!");
            for (pos, _) in code.match_indices(&token) {
                // `eprintln!` never matches the `eprint!` token — the `!` is
                // part of it — so only a leading identifier char can be a
                // false positive here.
                let preceded_by_ident = pos > 0
                    && (code.as_bytes()[pos - 1].is_ascii_alphanumeric()
                        || code.as_bytes()[pos - 1] == b'_');
                // A qualified `styling::eprintln!(…)` picks anstream's
                // regardless of what the file imported.
                if preceded_by_ident || code[..pos].ends_with("styling::") {
                    continue;
                }
                violations.push(format!(
                    "src/{relative_path}:{}: {}",
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }
}

/// Names a file imports from `worktrunk::styling` / `crate::styling`.
///
/// Only `use` statements count. A qualified call site (`styling::eprintln!(…)`)
/// carries the same `styling::` prefix but binds nothing, so counting it would
/// let a file's *bare* calls silently fall through to std's macro.
///
/// **Only *top-level* `use` statements count** — ones at column 0. The caller
/// tests coverage per file while Rust resolves imports per scope, so a
/// function-local `use worktrunk::styling::eprintln;` would otherwise mark
/// every other function in the file as covered when none of them is. Every
/// `eprint`/`eprintln` import in `src/` is top-level, so the narrower rule
/// costs nothing; a future function-local one fails this scan and gets
/// hoisted. (Function-local styling imports of *other* names do exist —
/// `commands/remove.rs`, `commands/configure_shell.rs` — so a caller that
/// checked a third macro could not assume the same.)
fn styling_imports(contents: &str) -> HashSet<String> {
    let mut imports = HashSet::new();
    let mut statement = String::new();

    for line in contents.lines() {
        if statement.is_empty() && !(line.starts_with("use ") || line.starts_with("pub use ")) {
            continue;
        }
        let trimmed = line.trim();
        statement.push_str(trimmed);
        if !trimmed.ends_with(';') {
            continue;
        }

        // `use worktrunk::styling::{a, b as c};` or `use crate::styling::a;`
        if let Some(pos) = statement.find("styling::") {
            let tail = &statement[pos + "styling::".len()..];
            let names = match tail.strip_prefix('{') {
                Some(braced) => braced.split_once('}').map(|(inner, _)| inner).unwrap_or(""),
                // The single-import form ends at the statement's `;`.
                None => tail.trim_end_matches(';'),
            };
            for name in names.split(',') {
                // `foo as bar` binds `bar`, which is not the macro this checks.
                let name = name.trim().split(" as ").next().unwrap_or("").trim();
                if !name.is_empty() {
                    imports.insert(name.to_string());
                }
            }
        }
        statement.clear();
    }

    imports
}

/// Every stdout surface exits cleanly when its consumer stops reading.
///
/// std's `print!`/`println!` panic on a `BrokenPipe`, so a command whose
/// answer went out through them died with exit 101 and a Rust panic message
/// the moment the reader left — `wt list | head -3`, and `wt list statusline`
/// on the surface a shell prompt runs on every redraw. anstream's macros drop
/// that error instead, and they now carry every answer this binary writes to
/// stdout.
///
/// How a surface resolves color — the process-global `ColorChoice` or tty
/// detection — is a separate axis, so the cases below span both.
///
/// The child's stdout pipe is closed before it is waited on, so its first
/// write has no reader. `--version` writes a few dozen bytes and still trips
/// it: `EPIPE` is about whether a reader is attached, not about filling the
/// pipe buffer.
#[rstest]
fn test_stdout_surfaces_survive_a_closed_consumer(repo: TestRepo) {
    for args in [
        &["--version"][..],
        &["merge", "--help-page"][..],
        &["merge", "--help-description"][..],
        &["merge", "--help-md"][..],
        &["list"][..],
        &["list", "--full"][..],
        &["list", "statusline"][..],
        &["list", "--format=json"][..],
        &["config", "update", "--print"][..],
    ] {
        let mut command = repo.wt_command();
        let mut child = command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn wt {args:?}: {e}"));

        drop(child.stdout.take().expect("stdout was piped"));

        let output = child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("failed to wait for wt {args:?}: {e}"));
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "wt {args:?} exited {:?} with its consumer gone; stderr: {stderr}",
            output.status.code()
        );
        assert!(
            !stderr.contains("panicked"),
            "wt {args:?} panicked with its consumer gone; stderr: {stderr}"
        );
    }
}

/// Narration on stderr strips its color when stderr isn't a terminal.
///
/// The stdout rule above has a stderr counterpart, and it is the same mistake:
/// `worktrunk::styling`'s `eprint!` is anstream's and strips ANSI off a
/// non-tty, std's prelude macro of the same name keeps it. A file that
/// imported `eprintln` from `styling` but not `eprint` got one of each, so a
/// deprecation warning and the hint printed directly beneath it disagreed
/// about whether `wt list 2>log` should carry escapes.
///
/// The suite's own `CLICOLOR_FORCE=1` is what hides this — it forces color on
/// both printers, so every snapshot agrees no matter which macro is in scope.
/// This test clears it and sets `NO_COLOR`, which only anstream honors.
///
/// The trigger is a deprecated `[ci]` block, whose warning is emitted by the
/// pre-deserialization deprecation pass on every command that loads config.
#[rstest]
fn test_stderr_narration_strips_ansi_when_piped(repo: TestRepo) {
    repo.write_project_config("[ci]\nplatform = \"github\"\n");

    let output = repo
        .wt_command()
        .arg("list")
        .env_remove("CLICOLOR_FORCE")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run wt list");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("deprecated"),
        "expected the [ci] deprecation warning on stderr; got: {stderr:?}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "wt narration redirected to a file must not contain ANSI escapes; got: {stderr:?}"
    );
}

/// Whether stdout carries ANSI escapes is the consumer's call, except where a
/// command declares otherwise.
///
/// Color resolves in one place — anstream's `AutoStream::choice`: the
/// process-global `ColorChoice` a command may write first, then tty detection
/// and the `NO_COLOR` / `CLICOLOR_FORCE` conventions. The rest of the suite
/// runs with `CLICOLOR_FORCE=1` (`STATIC_TEST_ENV_VARS`), which settles that
/// question before anstream reaches the interesting part, so no other test can
/// see which way it decides — the statusline losing its escapes to a strip was
/// invisible to a green suite. These cases clear both variables and read a
/// piped stdout, the environment a real consumer provides, and are the one
/// place the unforced contract is pinned.
#[rstest]
fn test_color_follows_the_consumer(repo: TestRepo) {
    // (args, extra env, stdout carries ANSI escapes, what the case pins)
    for (args, env, expect_escapes, pins) in [
        (
            &["list", "statusline"][..],
            None,
            true,
            "the statusline declares Always because its consumer renders the escapes",
        ),
        (
            &["list", "statusline"][..],
            Some(("NO_COLOR", "1")),
            true,
            "a declared Always outranks NO_COLOR, as --color=always conventionally does",
        ),
        (
            &["merge", "--help-page"][..],
            None,
            false,
            "web reference pages are portable Markdown; the site renderer owns syntax styling",
        ),
        (
            &["merge", "--help-page", "--plain"][..],
            None,
            false,
            "plain pages are the portable markdown skill reference files read",
        ),
        (
            &["list"][..],
            None,
            false,
            "human output strips when it is piped rather than displayed",
        ),
        (
            &["list"][..],
            Some(("CLICOLOR_FORCE", "1")),
            true,
            "force-on reaches a piped consumer that wants the color anyway",
        ),
    ] {
        let mut command = repo.wt_command();
        command
            .args(args)
            // The fixture forces color on for every other test; removing both
            // conventions is what leaves the decision to anstream.
            .env_remove("CLICOLOR_FORCE")
            .env_remove("NO_COLOR");
        if let Some((key, value)) = env {
            command.env(key, value);
        }

        // `output()` pipes stdout, so the child's consumer is not a terminal.
        let output = command
            .output()
            .unwrap_or_else(|e| panic!("failed to run wt {args:?}: {e}"));

        let case = match env {
            Some((key, value)) => format!("wt {} with {key}={value}", args.join(" ")),
            None => format!("wt {}", args.join(" ")),
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success(),
            "{case} exited {:?}; stderr: {stderr}",
            output.status.code()
        );
        // Without this an empty answer would satisfy every strip case.
        assert!(
            !stdout.is_empty(),
            "{case} wrote nothing to stdout; stderr: {stderr}"
        );
        assert_eq!(
            stdout.contains("\u{1b}["),
            expect_escapes,
            "{case}: expected stdout to carry ANSI escapes: {expect_escapes} — {pins}"
        );
    }
}
