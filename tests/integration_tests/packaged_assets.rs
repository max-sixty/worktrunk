//! Guard test: files embedded at compile time must ship in the packaged crate.
//!
//! Several macros read a file from disk *when the crate compiles*, not at
//! runtime:
//!
//! - `include_str!` / `include_bytes!` — embed `dev/*.example.toml`,
//!   `dev/opencode-plugin.ts`, `gemini-extension.json`.
//! - askama's `#[template(path = "…")]` — a proc-macro that reads
//!   `templates/*` from disk while expanding the derive.
//!
//! If any of these files is missing from the build environment, the crate
//! fails to *compile* — the same failure mode as #3123, just via a different
//! macro. The danger is that the build environment differs from the dev
//! checkout: a plain `cargo build` / `cargo test` always sees the files
//! (they're in the working tree), so it can never catch a packaging drop. The
//! gap only surfaces where the file is absent:
//!
//! - `cargo install worktrunk` — builds the crates.io source archive. A file
//!   not listed by `cargo package` simply isn't in the archive.
//! - the Nix build (`flake.nix`) — its source filter allowlists specific
//!   directories; a file outside them is filtered out before the build sees
//!   it.
//!
//! Both paths are outer-loop: a missing asset passes every PR check, then
//! breaks Nix that night and `cargo install` at the next release. This test
//! pulls the whole class inner-loop. It scans `src/` for every embedded path,
//! resolves each, and asserts the file is (a) present in `cargo package
//! --list` output (so it ships to crates.io) and (b) covered by the
//! `flake.nix` source filter (so it survives the Nix build).
//!
//! Adding a new `include_str!("../../assets/foo")` or `#[template(path =
//! "…")]` whose directory isn't packaged will fail here, on the PR, instead of
//! after publish.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use path_slash::PathExt as _;

/// askama resolves `#[template(path = "…")]` relative to `templates/` at the
/// crate root (the default when there is no `askama.toml`).
const ASKAMA_TEMPLATE_DIR: &str = "templates";

#[test]
fn embedded_assets_ship_in_package() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");

    // 1. Discover every compile-time-embedded path under `src/`, as a
    //    repo-root-relative forward-slash string.
    let mut assets = BTreeSet::new();
    scan_directory(&src_dir, manifest_dir, &mut assets);
    assert!(
        !assets.is_empty(),
        "scanned src/ but found no embedded assets — the scanner is likely broken"
    );

    // 2. The set of files `cargo publish` would put in the crates.io archive.
    let packaged = cargo_package_list(manifest_dir);

    // 3. The `flake.nix` source filter, read as text — its allowlist is what
    //    the Nix build sees.
    let flake_nix = fs::read_to_string(manifest_dir.join("flake.nix")).expect("read flake.nix");

    let mut violations = Vec::new();
    for asset in &assets {
        if !packaged.contains(asset) {
            violations.push(format!(
                "{asset}: embedded at compile time but NOT in `cargo package --list` \
                 — it would be absent from the crates.io archive, so `cargo install` \
                 fails to compile (the #3123 class)"
            ));
        }
        if !flake_covers(&flake_nix, asset) {
            violations.push(format!(
                "{asset}: embedded at compile time but not covered by the `flake.nix` \
                 source filter — it would be filtered out of the Nix build"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Embedded assets missing from a packaged build:\n\n{}\n\n\
         Each file above is read at compile time (include_str!/include_bytes!/askama \
         #[template]) but won't be present in at least one packaged build environment. \
         Add its directory to `Cargo.toml` packaging (it ships by default unless \
         include/exclude says otherwise) and to the `flake.nix` source filter.",
        violations.join("\n")
    );
}

fn scan_directory(dir: &Path, manifest_dir: &Path, assets: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_directory(&path, manifest_dir, assets);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            scan_file(&path, manifest_dir, assets);
        }
    }
}

fn scan_file(path: &Path, manifest_dir: &Path, assets: &mut BTreeSet<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    let source_dir = path.parent().unwrap_or(manifest_dir);

    for line in contents.lines() {
        // `include_str!("…")` / `include_bytes!("…")` — path relative to the
        // source file's directory.
        for macro_name in ["include_str!(\"", "include_bytes!(\""] {
            for literal in literals_after(line, macro_name) {
                if let Some(rel) = repo_relative(&source_dir.join(&literal), manifest_dir) {
                    assets.insert(rel);
                }
            }
        }

        // askama `#[template(path = "…")]` — path relative to `templates/`.
        for literal in literals_after(line, "template(path = \"") {
            let resolved = manifest_dir.join(ASKAMA_TEMPLATE_DIR).join(&literal);
            if let Some(rel) = repo_relative(&resolved, manifest_dir) {
                assets.insert(rel);
            }
        }
    }
}

/// Every string literal that immediately follows `prefix` on `line`, read up to
/// the next `"`. Handles multiple matches on one line.
fn literals_after(line: &str, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(idx) = rest.find(prefix) {
        let after = &rest[idx + prefix.len()..];
        if let Some(end) = after.find('"') {
            out.push(after[..end].to_string());
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out
}

/// Normalize a path (collapsing `.`/`..` lexically, no filesystem access) and
/// express it relative to the crate root with forward slashes. Returns `None`
/// if it escapes the crate root.
fn repo_relative(path: &Path, manifest_dir: &Path) -> Option<String> {
    let normalized = lexical_normalize(path);
    let relative = normalized.strip_prefix(manifest_dir).ok()?;
    Some(relative.to_slash_lossy().into_owned())
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if the `flake.nix` source filter would keep `asset`. The filter
/// allowlists whole directories by infix (`/dev/`, `/templates/`) and a few
/// root files by basename (`gemini-extension.json`). We map each asset to the
/// token the filter must mention: `/<top-dir>/` for a file in a subdirectory,
/// or the bare basename for a root-level file.
fn flake_covers(flake_nix: &str, asset: &str) -> bool {
    match asset.split_once('/') {
        Some((top, _)) => flake_nix.contains(&format!("/{top}/")),
        None => flake_nix.contains(asset),
    }
}

/// Run `cargo package --list` and collect the repo-relative paths it would
/// ship. This is the authoritative crates.io membership check — it does not
/// build, so it stays fast enough for every PR. `--allow-dirty` tolerates
/// local edits while iterating on a branch.
fn cargo_package_list(manifest_dir: &Path) -> BTreeSet<String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(&cargo)
        .args(["package", "--list", "-p", "worktrunk", "--allow-dirty"])
        .current_dir(manifest_dir)
        .output()
        .expect("spawn cargo package --list");
    assert!(
        output.status.success(),
        "`cargo package --list` failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}
