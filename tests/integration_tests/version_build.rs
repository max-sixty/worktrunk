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

use std::fs;
use std::path::Path;

use path_slash::PathExt as _;

#[test]
fn vergen_env_vars_are_read_optionally() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut violations = Vec::new();
    scan_directory(&src_dir, &src_dir, &mut violations);

    assert!(
        violations.is_empty(),
        "Found `env!` on a build-script `VERGEN_*` variable:\n\n{}\n\n\
         These variables are absent when building the crates.io source archive \
         (no `.git`), so `env!` fails to compile and breaks `cargo install` \
         (#3123). Use `option_env!` and fall back to a default instead.",
        violations.join("\n")
    );
}

fn scan_directory(dir: &Path, src_dir: &Path, violations: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_directory(&path, src_dir, violations);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            check_file(&path, src_dir, violations);
        }
    }
}

fn check_file(path: &Path, src_dir: &Path, violations: &mut Vec<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
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
