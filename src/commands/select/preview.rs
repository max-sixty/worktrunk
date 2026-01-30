//! Preview mode and layout management.
//!
//! Handles preview state persistence and layout auto-detection for the interactive selector.

use std::fs;
use std::path::PathBuf;

/// Preview modes for the interactive selector
///
/// Each mode shows a different aspect of the worktree:
/// 1. WorkingTree: Uncommitted changes (git diff HEAD --stat)
/// 2. Log: Commit history since diverging from the default branch (git log with merge-base)
/// 3. BranchDiff: Line diffs since the merge-base with the default branch (git diff --stat DEFAULT…)
/// 4. UpstreamDiff: Diff vs upstream tracking branch (ahead/behind)
///
/// Loosely aligned with `wt list` columns, though not a perfect match:
/// - Tab 1 corresponds to "HEAD±" column
/// - Tab 2 shows commits (related to "main↕" counts)
/// - Tab 3 corresponds to "main…± (--full)" column
/// - Tab 4 corresponds to "Remote⇅" column
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewMode {
    WorkingTree = 1,
    Log = 2,
    BranchDiff = 3,
    UpstreamDiff = 4,
}

impl PreviewMode {
    pub(super) fn from_u8(n: u8) -> Self {
        match n {
            2 => Self::Log,
            3 => Self::BranchDiff,
            4 => Self::UpstreamDiff,
            _ => Self::WorkingTree,
        }
    }
}

/// Typical terminal character aspect ratio (width/height).
///
/// Terminal characters are taller than wide - typically around 0.5 (twice as tall as wide).
/// This varies by font, but 0.5 is a reasonable default for monospace fonts.
const CHAR_ASPECT_RATIO: f64 = 0.5;

/// Skim uses this percentage of terminal height.
pub(super) const SKIM_HEIGHT_PERCENT: usize = 90;

/// Maximum number of list items visible in down layout before scrolling.
pub(super) const MAX_VISIBLE_ITEMS: usize = 12;

/// Lines reserved for skim chrome (header + prompt/margins).
pub(super) const LIST_CHROME_LINES: usize = 4;

/// Minimum preview lines to keep usable even with many items.
pub(super) const MIN_PREVIEW_LINES: usize = 5;

/// Preview layout orientation for the interactive selector
///
/// Preview window position (auto-detected at startup based on terminal dimensions)
///
/// - Right: Preview on the right side (50% width) - better for wide terminals
/// - Down: Preview below the list - better for tall/vertical monitors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum PreviewLayout {
    #[default]
    Right,
    Down,
}

impl PreviewLayout {
    /// Auto-detect layout based on terminal dimensions.
    ///
    /// Terminal dimensions are in characters, not pixels. Since characters are
    /// typically twice as tall as wide (~0.5 aspect ratio), we correct for this
    /// when calculating the effective aspect ratio.
    ///
    /// Example: 180 cols × 136 rows
    /// - Raw ratio: 180/136 = 1.32 (appears landscape)
    /// - Effective: 1.32 × 0.5 = 0.66 (actually portrait!)
    ///
    /// Returns Down for portrait (effective ratio < 1.0), Right for landscape.
    pub(super) fn auto_detect() -> Self {
        let (cols, rows) = terminal_size::terminal_size()
            .map(|(terminal_size::Width(w), terminal_size::Height(h))| (w as f64, h as f64))
            .unwrap_or((80.0, 24.0));

        // Effective aspect ratio accounting for character shape
        let effective_ratio = (cols / rows) * CHAR_ASPECT_RATIO;

        if effective_ratio < 1.0 {
            Self::Down
        } else {
            Self::Right
        }
    }

    /// Calculate the preview window spec for skim
    ///
    /// For Right layout: always 50%
    /// For Down layout: dynamically sized based on item count - list gets
    /// up to MAX_VISIBLE_ITEMS lines, preview gets the rest (min 5 lines)
    pub(super) fn to_preview_window_spec(self, num_items: usize) -> String {
        match self {
            Self::Right => "right:50%".to_string(),
            Self::Down => {
                let height = terminal_size::terminal_size()
                    .map(|(_, terminal_size::Height(h))| h as usize)
                    .unwrap_or(24);

                let available = height * SKIM_HEIGHT_PERCENT / 100;
                let list_lines = LIST_CHROME_LINES + num_items.min(MAX_VISIBLE_ITEMS);
                // Ensure preview doesn't exceed available space while trying to maintain minimum
                let remaining = available.saturating_sub(list_lines);
                let preview_lines = remaining.max(MIN_PREVIEW_LINES).min(available);

                format!("down:{}", preview_lines)
            }
        }
    }
}

/// Preview state persistence (mode only, layout auto-detected)
///
/// State file format: Single digit representing preview mode (1-4)
pub(super) struct PreviewStateData;

impl PreviewStateData {
    pub(super) fn state_path() -> PathBuf {
        // Use per-process temp file to avoid race conditions when running multiple instances
        std::env::temp_dir().join(format!("wt-select-state-{}", std::process::id()))
    }

    /// Read current preview mode from state file
    pub(super) fn read_mode() -> PreviewMode {
        let state_path = Self::state_path();
        fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree)
    }

    pub(super) fn write_mode(mode: PreviewMode) {
        let state_path = Self::state_path();
        let _ = fs::write(&state_path, format!("{}", mode as u8));
    }
}

/// RAII wrapper for preview state file lifecycle management
pub(super) struct PreviewState {
    pub(super) path: PathBuf,
    pub(super) initial_layout: PreviewLayout,
}

impl PreviewState {
    pub(super) fn new() -> Self {
        let path = PreviewStateData::state_path();
        PreviewStateData::write_mode(PreviewMode::WorkingTree);
        Self {
            path,
            initial_layout: PreviewLayout::auto_detect(),
        }
    }
}

impl Drop for PreviewState {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_mode_from_u8() {
        assert_eq!(PreviewMode::from_u8(1), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(2), PreviewMode::Log);
        assert_eq!(PreviewMode::from_u8(3), PreviewMode::BranchDiff);
        assert_eq!(PreviewMode::from_u8(4), PreviewMode::UpstreamDiff);
        // Invalid values default to WorkingTree
        assert_eq!(PreviewMode::from_u8(0), PreviewMode::WorkingTree);
        assert_eq!(PreviewMode::from_u8(99), PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_layout_to_preview_window_spec() {
        // Right is always 50%
        assert_eq!(PreviewLayout::Right.to_preview_window_spec(10), "right:50%");

        // Down calculates based on item count
        let spec = PreviewLayout::Down.to_preview_window_spec(5);
        assert!(spec.starts_with("down:"));
    }

    #[test]
    fn test_preview_state_data_read_default() {
        // Use unique path to avoid interference from parallel tests
        let state_path = std::env::temp_dir().join("wt-test-read-default");
        let _ = fs::remove_file(&state_path);

        // When state file doesn't exist, read returns default
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::WorkingTree);
    }

    #[test]
    fn test_preview_state_data_roundtrip() {
        // Use unique path to avoid interference from parallel tests
        let state_path = std::env::temp_dir().join("wt-test-roundtrip");

        // Write and read back various modes
        let _ = fs::write(&state_path, "1");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::WorkingTree);

        let _ = fs::write(&state_path, "2");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::Log);

        let _ = fs::write(&state_path, "3");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::BranchDiff);

        let _ = fs::write(&state_path, "4");
        let mode = fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .map(PreviewMode::from_u8)
            .unwrap_or(PreviewMode::WorkingTree);
        assert_eq!(mode, PreviewMode::UpstreamDiff);

        // Cleanup
        let _ = fs::remove_file(&state_path);
    }
}
