//! Test utilities for jj module.
//!
//! This module provides helpers for setting up test jj repositories.

use std::path::Path;
use std::process::Command;

/// Initialize a new jj repository at the given path.
pub fn init_jj_repo(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;

    let output = Command::new("jj")
        .args(["git", "init"])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to init jj repo: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Create a commit with a message.
pub fn create_commit(path: &Path, message: &str) -> anyhow::Result<()> {
    let output = Command::new("jj")
        .args(["commit", "-m", message])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to create commit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Create a file with content.
pub fn create_file(repo_path: &Path, filename: &str, content: &str) -> anyhow::Result<()> {
    std::fs::write(repo_path.join(filename), content)?;
    Ok(())
}
