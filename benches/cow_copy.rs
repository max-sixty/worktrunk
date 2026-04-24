// Benchmarks for `wt step copy-ignored` COW directory copying
//
// Compares serial vs the production parallel (rayon) recursive directory copying.
// Two directory layouts test different parallelism profiles:
//
// - deep: Rust target/ with most files in a single deps/ dir (narrow tree)
// - wide: Files spread across many subdirectories (wide tree)
//
// Run:
//   cargo bench --bench cow_copy
//   cargo bench --bench cow_copy -- serial   # serial only
//   cargo bench --bench cow_copy -- parallel # parallel only
//   cargo bench --bench cow_copy -- wide     # wide tree only

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::Path;
use tempfile::TempDir;
use worktrunk::copy::copy_dir_recursive;
use worktrunk::copy_progress::CopyProgress;

/// Create a narrow directory structure mimicking a Rust target/ directory.
///
/// Most files concentrate in debug/deps/ — exercises parallelism within a single
/// large directory.
fn create_deep_target(file_count: usize) -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");

    let subdirs = [
        "debug/deps",
        "debug/build",
        "debug/incremental",
        "release/deps",
    ];
    for subdir in &subdirs {
        std::fs::create_dir_all(target.join(subdir)).unwrap();
    }

    let mut created = 0;
    let deps_dir = target.join("debug/deps");

    while created < file_count {
        let rlib = deps_dir.join(format!("libcrate_{:04}.rlib", created));
        std::fs::write(&rlib, vec![0u8; 100_000]).unwrap();
        created += 1;

        if created >= file_count {
            break;
        }

        let rmeta = deps_dir.join(format!("libcrate_{:04}.rmeta", created));
        std::fs::write(&rmeta, vec![0u8; 10_000]).unwrap();
        created += 1;

        if created >= file_count {
            break;
        }

        let dep = deps_dir.join(format!("libcrate_{:04}.d", created));
        std::fs::write(&dep, vec![0u8; 500]).unwrap();
        created += 1;
    }

    let incr = target.join("debug/incremental/crate_name-hash");
    std::fs::create_dir_all(&incr).unwrap();
    for i in 0..10 {
        std::fs::write(incr.join(format!("s-abc123-{}.lock", i)), "").unwrap();
    }

    temp
}

/// Create a wide directory tree with files distributed across many subdirectories.
///
/// Exercises parallelism across sibling directories — the primary benefit of
/// rayon in copy_dir_recursive.
fn create_wide_target(file_count: usize) -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");

    let files_per_dir = 10;
    let dir_count = file_count / files_per_dir;
    let mut created = 0;

    for d in 0..dir_count {
        let subdir = target.join(format!("pkg_{:04}", d));
        std::fs::create_dir_all(&subdir).unwrap();

        for f in 0..files_per_dir {
            if created >= file_count {
                return temp;
            }
            let file = subdir.join(format!("file_{:02}.dat", f));
            std::fs::write(&file, vec![0u8; 50_000]).unwrap();
            created += 1;
        }
    }

    temp
}

/// Serial baseline: sequential file-by-file copy without rayon.
fn copy_dir_serial(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_serial(&src_path, &dest_path)?;
        } else if file_type.is_file() {
            reflink_copy::reflink_or_copy(src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn bench_helper(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    label: &str,
    file_count: usize,
    create_fn: fn(usize) -> TempDir,
) {
    let temp = create_fn(file_count);
    let src = temp.path().join("target");

    group.bench_with_input(
        BenchmarkId::new(format!("serial/{label}"), file_count),
        &src,
        |b, src| {
            let mut iter = 0u64;
            b.iter(|| {
                let dest = temp.path().join(format!("copy_s_{}", iter));
                iter += 1;
                copy_dir_serial(src, &dest).unwrap();
                std::fs::remove_dir_all(&dest).ok();
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new(format!("parallel/{label}"), file_count),
        &src,
        |b, src| {
            let mut iter = 0u64;
            b.iter(|| {
                let dest = temp.path().join(format!("copy_p_{}", iter));
                iter += 1;
                copy_dir_recursive(src, &dest, false, &CopyProgress::disabled()).unwrap();
                std::fs::remove_dir_all(&dest).ok();
            });
        },
    );
}

fn bench_copy_target(c: &mut Criterion) {
    let mut group = c.benchmark_group("copy_target");

    for &file_count in &[100, 500, 1000] {
        bench_helper(&mut group, "deep", file_count, create_deep_target);
        bench_helper(&mut group, "wide", file_count, create_wide_target);
    }

    group.finish();
}

criterion_group!(benches, bench_copy_target);
criterion_main!(benches);
