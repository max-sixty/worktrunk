//! One recursive walk for the guards that read source as text.
//!
//! Those guards assert *absence*, which is what makes a swallowed read
//! dangerous here in a way it isn't for code that acts on what it finds — see
//! "Guards that scan source text" in `tests/CLAUDE.md` for the shape of that
//! failure and the rules it sets.
//!
//! Every read below panics rather than skipping, so the walk either covers the
//! tree or says which read stopped it.
//!
use std::fs;
use std::path::Path;

/// Visit every `.{extension}` file under `dir`, recursively, passing each
/// file's path and contents to `f`.
///
/// `scan` names this walk in panic messages (`"src/ scan"`, `"snapshot
/// scan"`), so a failure says which guard was running as well as which read
/// failed. The three reads are distinguished: a directory that can't be
/// listed, an entry within a listed directory, and a file's contents. Naming
/// only the path would leave the first two printing the same line, and the
/// entry case is the one where checking the directory finds it healthy and
/// sends you back to the start.
pub fn visit_files(
    dir: &Path,
    extension: &str,
    scan: &str,
    f: &mut impl FnMut(&Path, &str),
) -> usize {
    let mut visited = 0usize;
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} unreadable during the {scan}: {e}", dir.display()));

    for entry in entries {
        // `flatten()` here would drop a per-entry error — a file removed
        // mid-walk, a failing `stat` — and skip a file without saying so.
        let entry = entry.unwrap_or_else(|e| {
            panic!(
                "an entry in {} unreadable during the {scan}: {e}",
                dir.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            visited += visit_files(&path, extension, scan, f);
        } else if path.extension().and_then(|s| s.to_str()) == Some(extension) {
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} unreadable during the {scan}: {e}", path.display()));
            f(&path, &contents);
            visited += 1;
        }
    }
    visited
}
