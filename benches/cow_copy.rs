// Benchmarks for `wt step copy-ignored` COW directory copying
//
// Tests copy performance with realistic Rust target/ directory structures.
// Uses file-by-file reflink copying on all platforms.
//
// Run:
//   cargo bench --bench cow_copy

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use tempfile::TempDir;

/// Create a directory structure mimicking a Rust target/ directory.
///
/// Structure: target/debug/{deps,build,incremental}/... with .rlib, .rmeta, .d files
fn create_target_dir(file_count: usize) -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");

    // Create typical Rust target subdirectories
    let subdirs = [
        "debug/deps",
        "debug/build",
        "debug/incremental",
        "release/deps",
    ];
    for subdir in &subdirs {
        std::fs::create_dir_all(target.join(subdir)).unwrap();
    }

    // Distribute files across subdirectories (mostly in deps/)
    let mut created = 0;
    let deps_dir = target.join("debug/deps");

    while created < file_count {
        // Create .rlib files (larger, ~100KB simulated)
        let rlib = deps_dir.join(format!("libcrate_{:04}.rlib", created));
        std::fs::write(&rlib, vec![0u8; 100_000]).unwrap();
        created += 1;

        if created >= file_count {
            break;
        }

        // Create .rmeta files (smaller, ~10KB)
        let rmeta = deps_dir.join(format!("libcrate_{:04}.rmeta", created));
        std::fs::write(&rmeta, vec![0u8; 10_000]).unwrap();
        created += 1;

        if created >= file_count {
            break;
        }

        // Create .d dependency files (tiny, ~500 bytes)
        let dep = deps_dir.join(format!("libcrate_{:04}.d", created));
        std::fs::write(&dep, vec![0u8; 500]).unwrap();
        created += 1;
    }

    // Add some incremental compilation artifacts
    let incr = target.join("debug/incremental/crate_name-hash");
    std::fs::create_dir_all(&incr).unwrap();
    for i in 0..10 {
        std::fs::write(incr.join(format!("s-abc123-{}.lock", i)), "").unwrap();
    }

    temp
}

/// Copy directory using the same approach as `wt step copy-ignored`.
///
/// Uses file-by-file reflink on all platforms. We intentionally avoid atomic
/// directory cloning on macOS (via clonefile()) because it saturates disk I/O
/// and freezes interactive processes. See step_commands.rs for details.
fn copy_dir_cow(src: &Path, dest: &Path) -> std::io::Result<()> {
    copy_dir_recursive(src, dest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else if file_type.is_file() {
            reflink_copy::reflink_or_copy(src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn bench_copy_target(c: &mut Criterion) {
    let mut group = c.benchmark_group("copy_target");

    // Test various sizes typical of Rust projects
    for &file_count in &[100, 500, 1000, 2000] {
        let temp = create_target_dir(file_count);
        let src = temp.path().join("target");

        group.bench_with_input(BenchmarkId::new("files", file_count), &src, |b, src| {
            let mut iter = 0u64;
            b.iter(|| {
                let dest = temp.path().join(format!("target_copy_{}", iter));
                iter += 1;
                copy_dir_cow(src, &dest).unwrap();
                std::fs::remove_dir_all(&dest).ok();
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_copy_target);
criterion_main!(benches);
