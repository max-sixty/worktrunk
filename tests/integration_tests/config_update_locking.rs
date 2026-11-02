//! Migration must not overwrite a completed cooperating user-config mutation.

#![cfg(target_os = "linux")]

use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread::JoinHandle;

use ansi_str::AnsiStr;
use fs2::FileExt;
use worktrunk::config::UserConfig;

use crate::common::{TestRepo, repo, test_tempdir, wait_for, wt_command};
use rstest::rstest;

struct MigrationProcess {
    child: Option<Child>,
    mutation: Option<JoinHandle<()>>,
}

impl Drop for MigrationProcess {
    fn drop(&mut self) {
        // Release the child's config lock before joining a queued mutation.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mutation) = self.mutation.take() {
            let _ = mutation.join();
        }
    }
}

fn child_has_file_open(pid: u32, path: &Path) -> bool {
    let expected = fs::metadata(path).unwrap();
    let Ok(mut entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    entries.any(|entry| {
        // Unrelated descriptors can close during enumeration. The held
        // approvals lock cannot disappear before this test releases it.
        entry
            .and_then(|entry| fs::metadata(entry.path()))
            .is_ok_and(|actual| actual.dev() == expected.dev() && actual.ino() == expected.ino())
    })
}

fn mutate_config(path: &Path) {
    UserConfig::default()
        .set_skip_shell_integration_prompt(path)
        .unwrap();
}

#[test]
fn test_config_update_preserves_a_cooperating_mutation() {
    cooperating_mutation("config.toml", false);
}

#[test]
fn test_config_update_serializes_a_cooperating_mutation() {
    cooperating_mutation("aaa.toml", true);
}

fn cooperating_mutation(filename: &str, expected_config_lock: bool) {
    let directory = test_tempdir();
    let config = directory.path().join(filename);
    fs::write(
        &config,
        "[projects.fixture]\napproved-commands = [\"echo fixture\"]\n",
    )
    .unwrap();
    let approvals_lock_path = directory.path().join("approvals.toml.lock");
    let approvals_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&approvals_lock_path)
        .unwrap();
    FileExt::lock_exclusive(&approvals_lock).unwrap();

    let child = wt_command()
        .arg("--config")
        .arg(&config)
        .args(["config", "update", "--yes"])
        .current_dir(directory.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut migration = MigrationProcess {
        child: Some(child),
        mutation: None,
    };
    let pid = migration.child.as_ref().unwrap().id();
    wait_for("migration to open its blocking approvals lock", || {
        assert!(
            migration
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none()
        );
        child_has_file_open(pid, &approvals_lock_path)
    });

    let config_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.with_extension("toml.lock"))
        .unwrap();
    let migration_holds_config_lock = match FileExt::try_lock_exclusive(&config_lock) {
        Ok(()) => {
            FileExt::unlock(&config_lock).unwrap();
            false
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => true,
        Err(error) => panic!("probe config lock: {error}"),
    };
    assert_eq!(migration_holds_config_lock, expected_config_lock);
    let mut mutation_snapshot = None;
    if migration_holds_config_lock {
        // A serialized implementation legitimately queues this mutation.
        let path = config.clone();
        migration.mutation = Some(std::thread::spawn(move || mutate_config(&path)));
        println!("migration holds the config lock; ordinary mutation scheduled");
    } else {
        // With no config lock, complete a real public mutation before the
        // migration can proceed. This is the lost-update interleaving.
        mutate_config(&config);
        let current: UserConfig = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert!(current.skip_shell_integration_prompt);
        mutation_snapshot = Some(fs::read_to_string(&config).unwrap());
        println!("ordinary mutation completed while migration waited for approvals");
    }
    FileExt::unlock(&approvals_lock).unwrap();
    wait_for("migration to finish", || {
        migration
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_some()
    });
    let output = migration.child.take().unwrap().wait_with_output().unwrap();
    if let Some(mutation) = migration.mutation.take() {
        mutation.join().unwrap();
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let contents = fs::read_to_string(&config).unwrap();
    let updated: UserConfig = toml::from_str(&contents).unwrap();
    assert!(
        updated.skip_shell_integration_prompt,
        "migration lost a completed cooperating config mutation:\n{contents}"
    );
    if migration_holds_config_lock {
        assert!(output.status.success(), "{stderr}");
        assert!(!contents.contains("approved-commands"));
        assert!(
            fs::read_to_string(directory.path().join("approvals.toml"))
                .unwrap()
                .contains("echo fixture")
        );
    } else {
        // A complete sorted lock set can wait on approvals before taking the
        // config lock. The completed mutation invalidates its exact preview:
        // only the existing, specific refusal is safe, never an arbitrary error.
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        let plain = stderr.ansi_strip();
        assert!(plain.contains("User config changed @ "), "{plain}");
        assert!(
            plain.contains(
                "since the preview; to migrate the current contents, re-run wt config update"
            ),
            "{plain}"
        );
        assert_eq!(contents, mutation_snapshot.unwrap());
        assert!(!directory.path().join("approvals.toml").exists());
    }
}

#[test]
fn test_config_update_preserves_overlapping_approvals_source() {
    overlapping_approvals_source(false);
}

#[test]
fn test_config_update_output_preserves_overlapping_approvals_source() {
    overlapping_approvals_source(true);
}

fn overlapping_approvals_source(explicit_output: bool) {
    let directory = test_tempdir();
    let config = directory.path().join("approvals.toml");
    let original = "[projects.fixture]\napproved-commands = [\"echo fixture\"]\n";
    fs::write(&config, original).unwrap();
    let mut command = wt_command();
    command
        .arg("--config")
        .arg(&config)
        .args(["config", "update", "--yes"]);
    if explicit_output {
        command.arg("--output").arg(&config);
    }
    let child = command
        .current_dir(directory.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut migration = MigrationProcess {
        child: Some(child),
        mutation: None,
    };
    wait_for("same-file migration to finish", || {
        migration
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_some()
    });
    let output = migration.child.take().unwrap().wait_with_output().unwrap();
    println!(
        "same-file migration completed with status {:?}",
        output.status.code()
    );
    let contents = fs::read_to_string(&config).unwrap();
    println!("same-file source bytes after migration: {}", contents.len());
    assert_eq!(
        contents, original,
        "config migration discarded its own approvals source"
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .ansi_strip()
            .contains("is also the approvals destination")
    );
}

#[test]
fn test_config_update_deduplicates_distinct_data_with_a_shared_lock() {
    for filename in ["approvals.conf", "approvals"] {
        let directory = test_tempdir();
        let config = directory.path().join(filename);
        fs::write(
            &config,
            "[projects.fixture]\napproved-commands = [\"echo fixture\"]\n",
        )
        .unwrap();
        let child = wt_command()
            .arg("--config")
            .arg(&config)
            .args(["config", "update", "--yes"])
            .current_dir(directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut migration = MigrationProcess {
            child: Some(child),
            mutation: None,
        };
        wait_for("shared-lock migration to finish", || {
            migration
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_some()
        });
        let output = migration.child.take().unwrap().wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !fs::read_to_string(&config)
                .unwrap()
                .contains("approved-commands")
        );
        assert!(
            fs::read_to_string(directory.path().join("approvals.toml"))
                .unwrap()
                .contains("echo fixture")
        );
    }
}

#[rstest]
fn test_project_migration_does_not_add_an_uncoordinated_lock(repo: TestRepo) {
    repo.write_project_config("pre-start = \"echo {{ main_worktree }}\"\n");
    repo.commit("Add deprecated project config");
    let output = repo
        .wt_command()
        .args(["config", "update", "--yes"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = repo.root_path().join(".config/wt.toml");
    assert!(fs::read_to_string(&path).unwrap().contains("{{ repo }}"));
    assert!(!path.with_extension("toml.lock").exists());
}

#[test]
fn test_migration_preflights_all_sources_before_creating_locks() {
    use worktrunk::config::{ConfigFileKind, ConfigMigrationLocks, ConfigMigrationTarget};
    let directory = test_tempdir();
    let first = directory.path().join("first/config.toml");
    let second = directory.path().join("second/approvals.toml");
    let original = "[projects.fixture]\napproved-commands = [\"echo fixture\"]\n";
    for path in [&first, &second] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, original).unwrap();
    }
    let targets = [
        ConfigMigrationTarget {
            path: &first,
            kind: ConfigFileKind::User,
            moves_approvals: true,
        },
        ConfigMigrationTarget {
            path: &second,
            kind: ConfigFileKind::User,
            moves_approvals: true,
        },
    ];
    let error = ConfigMigrationLocks::acquire(&targets)
        .err()
        .expect("overlapping source must be refused");
    assert!(
        error
            .to_string()
            .contains("is also the approvals destination")
    );
    for path in [&first, &second] {
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert!(!path.with_extension("toml.lock").exists());
    }
    assert!(!first.with_file_name("approvals.toml").exists());
}
