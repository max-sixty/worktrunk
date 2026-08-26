//! Guard test: build-script env vars must be read with `option_env!`, never `env!`
//!
//! `cargo install worktrunk` builds the crates.io source archive, which has no
//! `.git`. The `vergen_gitcl` build script can only set `VERGEN_GIT_DESCRIBE`
//! when `git describe` succeeds, so on that archive the variable is undefined
//! at compile time.
//!
//! `env!("VERGEN_GIT_DESCRIBE")` then **fails to compile** ("environment
//! variable not defined at compile time"), which broke `cargo install`
//! (#3123). `option_env!` yields `None` instead, letting `version_str()` fall
//! back to the cargo package version.
//!
//! This couldn't be caught by a build test that compiles inside the repo:
//! `git describe` searches ancestors, so a `cargo package` / `cargo publish`
//! verify build run from the worktree finds the *outer* `.git` and succeeds —
//! exactly why the broken release shipped. The failure only reproduces where
//! no ancestor is a git repo (a user's `~/.cargo/registry`). Rather than
//! recreate that (slow, requires building outside the tree), this guards the
//! mechanism directly: forbid `env!` on any build-script-provided `VERGEN_*`
//! variable, since none are guaranteed to exist on the crates.io archive.

use std::path::Path;
use std::process::Command;

use crate::common::source_scan::visit_files;
use path_slash::PathExt as _;

#[test]
fn vergen_env_vars_are_read_optionally() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut violations = Vec::new();
    let scanned = visit_files(&src_dir, "rs", "vergen scan", &mut |path, contents| {
        check_file(path, contents, &src_dir, &mut violations)
    });
    assert!(
        scanned > 0,
        "scanned no files under {} — this test asserts absence, so it would have \
         passed over nothing",
        src_dir.display()
    );

    assert!(
        violations.is_empty(),
        "Found `env!` on a build-script `VERGEN_*` variable:\n\n{}\n\n\
         These variables are absent when building the crates.io source archive \
         (no `.git`), so `env!` fails to compile and breaks `cargo install` \
         (#3123). Use `option_env!` and fall back to a default instead.",
        violations.join("\n")
    );
}

fn check_file(path: &Path, contents: &str, src_dir: &Path, violations: &mut Vec<String>) {
    let relative = path.strip_prefix(src_dir).unwrap_or(path).to_slash_lossy();

    for (i, line) in contents.lines().enumerate() {
        if has_bare_env_vergen(line) {
            violations.push(format!("{relative}:{}: {}", i + 1, line.trim()));
        }
    }
}

/// True if the line reads a `VERGEN_*` variable with `env!` but not the safe
/// `option_env!` form. Matching `env!("VERGEN_` alone would also flag every
/// `option_env!("VERGEN_` since it ends in that substring, so we require the
/// `env!` to not be the tail of `option_env!`.
fn has_bare_env_vergen(line: &str) -> bool {
    line.match_indices("env!(\"VERGEN_")
        .any(|(idx, _)| !line[..idx].ends_with("option_"))
}

/// End-to-end reproduction of #3123: build the *actual* crates.io source
/// archive in a directory with no ancestor `.git`, and confirm the `wt` binary
/// both compiles and reports the cargo package version (the fallback).
///
/// The sibling `vergen_env_vars_are_read_optionally` guard is fast and runs on
/// every PR, but it only checks the *mechanism* (`env!` vs `option_env!`) by
/// scanning source. This test is the faithful integration version the
/// maintainer asked for: it packages the crate exactly as `cargo publish`
/// would, extracts it where `git describe` can find no repository, and builds
/// it — the precise condition of a user's `~/.cargo/registry`. Before the fix
/// this failed to *compile* (`VERGEN_GIT_DESCRIBE` not defined); after it, the
/// build succeeds and `wt --version` falls back to `CARGO_PKG_VERSION`.
///
/// `#[ignore]` because it runs a full from-scratch crate build (several
/// minutes, downloads every dependency). CI runs it in the `crate-build`
/// nightly job via `cargo test … -- --ignored`. Linux/macOS only — it shells
/// out to `tar` and relies on `GIT_CEILING_DIRECTORIES` path semantics.
#[test]
#[ignore = "slow: full from-scratch crate build; run in the nightly workflow via --ignored"]
fn crate_io_archive_builds_and_versions_without_git() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // `CARGO` points at the cargo that launched the test; fall back for manual
    // `--ignored` runs that don't set it.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let pkg_version = env!("CARGO_PKG_VERSION");

    // 1. Produce the crates.io source archive exactly as `cargo publish` does.
    //    `--no-verify`: the default verify build compiles *in-tree*, where
    //    `git describe` ascends to the outer repo's `.git` and masks the bug —
    //    the very reason the broken release shipped. We run our own no-git
    //    build below instead. `--allow-dirty`: tolerate local edits while
    //    iterating on the branch.
    let status = Command::new(&cargo)
        .args(["package", "-p", "worktrunk", "--no-verify", "--allow-dirty"])
        .current_dir(manifest_dir)
        .status()
        .expect("spawn cargo package");
    assert!(status.success(), "cargo package failed");

    let crate_file = manifest_dir
        .join("target/package")
        .join(format!("worktrunk-{pkg_version}.crate"));
    assert!(crate_file.exists(), "expected archive at {crate_file:?}");

    // 2. Extract into a temp dir that has no ancestor `.git`.
    let temp = tempfile::tempdir().expect("create tempdir");
    let untar = Command::new("tar")
        .arg("xzf")
        .arg(&crate_file)
        .arg("-C")
        .arg(temp.path())
        .status()
        .expect("spawn tar");
    assert!(untar.success(), "tar extraction failed");
    let extracted = temp.path().join(format!("worktrunk-{pkg_version}"));
    assert!(extracted.is_dir(), "extracted dir missing: {extracted:?}");

    // 3. Build `wt` (default features → `cli` is on) with no `.git` reachable.
    //    `GIT_CEILING_DIRECTORIES` stops `git describe` from ascending past the
    //    temp dir, so the build sees the same no-repository state as
    //    `~/.cargo/registry` even if the temp root happened to sit under one.
    //    `VERGEN_GIT_DESCRIBE` is removed so a packager-style override (the
    //    workaround Homebrew/conda-forge/nix all ship) can't mask the failure.
    let build = Command::new(&cargo)
        .args(["build", "--bin", "wt"])
        .current_dir(&extracted)
        .env("GIT_CEILING_DIRECTORIES", temp.path())
        .env_remove("VERGEN_GIT_DESCRIBE")
        .status()
        .expect("spawn cargo build");
    assert!(
        build.success(),
        "building the crates.io archive without `.git` failed (this is #3123)"
    );

    // 4. The built binary must report the cargo package version — proof the
    //    `option_env!` fallback fired rather than baking a git-describe string.
    let wt = extracted.join("target/debug/wt");
    let version = Command::new(&wt)
        .arg("--version")
        .output()
        .expect("spawn wt --version");
    assert!(version.status.success(), "`wt --version` exited non-zero");
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(
        stdout.contains(pkg_version),
        "expected `wt --version` to fall back to {pkg_version}, got: {stdout:?}"
    );
}
