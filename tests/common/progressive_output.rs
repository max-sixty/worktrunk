//! Progressive output testing utilities
//!
//! Tests commands that use progressive rendering (like `wt list --full`), where the table
//! structure appears first with placeholder dots (⋯), then data fills in incrementally using
//! ANSI cursor movements to update rows in-place.
//!
//! # The Problem
//!
//! Traditional test approaches capture stdout/stderr as linear byte streams. They cannot verify:
//! - Table structure appearing before data
//! - Placeholder dots being replaced with actual data over time
//! - ANSI cursor movements updating rows in-place
//!
//! # The Solution
//!
//! This module uses:
//! - **PTY (pseudo-terminal)** - Spawns commands in a real terminal
//! - **vt100 terminal emulator** - Interprets ANSI escape sequences
//! - **Progressive snapshots** - Captures screen state at intervals
//! - **Behavioral verification** - Tests progressive filling without exact snapshot matching
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use crate::common::progressive_output::{capture_progressive_output, ProgressiveCaptureOptions};
//! use crate::common::TestRepo;
//!
//! #[test]
//! fn test_progressive_rendering() {
//!     let mut repo = TestRepo::new();
//!     repo.commit("Initial");
//!     repo.add_worktree("feature");
//!
//!     // Capture using byte-based strategy (deterministic for behavioral tests)
//!     let output = capture_progressive_output(
//!         &repo,
//!         "list",
//!         &["--full", "--branches"],
//!         ProgressiveCaptureOptions::with_byte_interval(500),
//!     );
//!
//!     // Verify progressive filling behavior
//!     assert_eq!(output.exit_code, 0);
//!     assert!(output.stages.len() > 1, "Should capture multiple stages");
//!     output.verify_progressive_filling().unwrap();
//!
//!     // Verify header appears immediately
//!     assert!(output.initial().visible_text().contains("Branch"));
//!
//!     // Verify all data eventually appears
//!     assert!(output.final_output().contains("feature"));
//! }
//! ```
//!
//! # Byte-Based vs Time-Based Capture
//!
//! **Use `with_byte_interval(N)` for tests** - Captures at fixed byte counts (e.g., every 500 bytes).
//! This produces deterministic behavioral tests: stage counts and progressive filling verification
//! work reliably across runs.
//!
//! **Why not snapshot testing?** Even with byte-based capture, exact content at each threshold
//! varies due to PTY buffering timing. Behavioral assertions work; exact snapshots don't.
//!
//! # Common Testing Patterns
//!
//! **Verify structure appears first:**
//! ```rust,ignore
//! let initial = output.initial().visible_text();
//! assert!(initial.contains("Branch"));  // Header appears
//! assert!(initial.contains("⋯"));        // Data incomplete
//! ```
//!
//! **Verify progressive data filling:**
//! ```rust,ignore
//! output.verify_progressive_filling().unwrap();
//! // Or manually:
//! let dots = output.dots_per_stage();
//! assert!(dots[0] > dots[dots.len() - 1]);
//! ```
//!
//! **Verify all data eventually appears:**
//! ```rust,ignore
//! let final_text = output.final_output();
//! assert!(final_text.contains("feature-a"));
//! assert!(!final_text.contains("⋯"));  // No placeholders remain
//! ```
//!
//! # Key API Methods
//!
//! **ProgressiveOutput:**
//! - `initial()` - First snapshot (table structure)
//! - `final_output()` - Final text (complete data)
//! - `samples(n)` - Get n evenly-spaced snapshots
//! - `verify_progressive_filling()` - Assert dots decrease over time
//! - `dots_per_stage()` - Count placeholder dots in each stage
//!
//! **ProgressiveCaptureOptions:**
//! - `with_byte_interval(n)` - Capture every n bytes (deterministic)
//! - `default()` - Time-based capture every 50ms (less deterministic)

use crate::common::{ExponentialBackoff, TestRepo};
use insta_cmd::get_cargo_bin;
use portable_pty::CommandBuilder;
use std::io::Read;
use std::time::{Duration, Instant};

/// Polling interval when waiting for output or child process exit.
/// Short interval ensures tests complete quickly when data is available.
const OUTPUT_POLL_INTERVAL_MS: u64 = 10;

/// Number of consecutive "no data" reads required before considering the stream truly empty.
/// On Linux, the PTY may return EOF before all data has arrived (kernel scheduling).
/// Uses default exponential backoff (10ms → 500ms cap, 5s timeout) for reliability.
const STABLE_READ_THRESHOLD: u32 = 4;

/// Minimum time to wait during drain before accepting stable EOF.
/// On slow CI systems (especially Ubuntu), the PTY may return spurious EOFs before
/// all data has been flushed from the kernel buffer. This ensures we wait long enough
/// for the kernel to complete its PTY buffer flush, regardless of how many EOFs we've seen.
/// 1000ms provides enough margin for slow CI runners under load with many worktrees.
const MIN_DRAIN_WAIT_MS: u64 = 1000;

/// Strategy for capturing progressive output snapshots
#[derive(Debug, Clone)]
pub enum CaptureStrategy {
    /// Capture snapshots at time intervals (less deterministic, captures real-time behavior)
    TimeInterval(Duration),
    /// Capture snapshots at byte count milestones (more deterministic than time-based,
    /// suitable for behavioral testing but not exact snapshot matching)
    ByteInterval(usize),
}

/// Options for capturing progressive output
#[derive(Debug, Clone)]
pub struct ProgressiveCaptureOptions {
    /// Strategy for when to capture snapshots
    pub strategy: CaptureStrategy,
    /// Maximum time to wait for output (default: 10s)
    pub timeout: Duration,
    /// Terminal size in (rows, cols) (default: 48 rows x 150 cols - matches test suite conventions)
    pub terminal_size: (u16, u16),
    /// Minimum change required between snapshots
    /// - For TimeInterval: minimum time between different snapshots (default: 10ms)
    /// - For ByteInterval: minimum bytes between snapshots (default: 100 bytes)
    pub min_change_threshold: usize,
}

impl Default for ProgressiveCaptureOptions {
    fn default() -> Self {
        Self {
            strategy: CaptureStrategy::TimeInterval(Duration::from_millis(50)),
            timeout: Duration::from_secs(10),
            terminal_size: (48, 150),
            min_change_threshold: 100, // 100 bytes minimum change
        }
    }
}

impl ProgressiveCaptureOptions {
    /// Create options with byte-based capture (more deterministic than time-based, for behavioral testing)
    pub fn with_byte_interval(byte_interval: usize) -> Self {
        Self {
            strategy: CaptureStrategy::ByteInterval(byte_interval),
            timeout: Duration::from_secs(10),
            terminal_size: (48, 150),
            min_change_threshold: 100,
        }
    }
}

/// A single snapshot of terminal output at a point in time
#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    /// Time elapsed since command started
    pub timestamp: Duration,
    /// The visible text on screen (what user would see)
    visible_text: String,
}

impl OutputSnapshot {
    /// Get the visible text (with ANSI codes stripped, formatted for humans)
    pub fn visible_text(&self) -> &str {
        &self.visible_text
    }
}

/// Collection of progressive output snapshots
#[derive(Debug)]
pub struct ProgressiveOutput {
    /// Individual snapshots captured during execution
    pub stages: Vec<OutputSnapshot>,
    /// Exit code of the command
    pub exit_code: i32,
    /// Total execution time
    pub total_duration: Duration,
}

impl ProgressiveOutput {
    /// Get the initial snapshot (what appeared first)
    pub fn initial(&self) -> &OutputSnapshot {
        self.stages.first().unwrap()
    }

    /// Get the final snapshot (complete output)
    pub fn final_snapshot(&self) -> &OutputSnapshot {
        self.stages.last().unwrap()
    }

    /// Get the final visible text (convenience for final_snapshot().visible_text())
    pub fn final_output(&self) -> &str {
        self.final_snapshot().visible_text()
    }

    /// Get a snapshot at approximately the given timestamp
    pub fn snapshot_at(&self, target: Duration) -> &OutputSnapshot {
        self.stages
            .iter()
            .min_by_key(|s| s.timestamp.abs_diff(target))
            .unwrap()
    }

    /// Get snapshots at regular intervals (useful for showing progression)
    pub fn samples(&self, count: usize) -> Vec<&OutputSnapshot> {
        if self.stages.is_empty() || count == 0 {
            return vec![];
        }

        if count >= self.stages.len() {
            return self.stages.iter().collect();
        }

        let step = (self.stages.len() - 1) as f64 / (count - 1) as f64;
        (0..count)
            .map(|i| {
                let index = ((i as f64 * step).round() as usize).min(self.stages.len() - 1);
                &self.stages[index]
            })
            .collect()
    }

    /// Count how many times dots (⋯) appear in each snapshot
    /// Useful for verifying progressive data filling
    pub fn dots_per_stage(&self) -> Vec<usize> {
        self.stages
            .iter()
            .map(|s| s.visible_text.matches('⋯').count())
            .collect()
    }

    /// Verify that dots decrease over time (data is filling in)
    ///
    /// This handles the typical progressive rendering pattern:
    /// 1. Header appears (0 dots)
    /// 2. Rows appear with placeholders (dots increase)
    /// 3. Data fills in (dots decrease)
    ///
    /// Verification succeeds if dots eventually decrease from their peak,
    /// indicating progressive data filling.
    pub fn verify_progressive_filling(&self) -> Result<(), String> {
        let dots = self.dots_per_stage();
        if dots.is_empty() {
            return Err("No snapshots captured".to_string());
        }

        if dots.len() < 2 {
            return Err("Need at least 2 snapshots to verify progressive filling".to_string());
        }

        // Find the peak (maximum) dots
        let (peak_index, peak_dots) = dots
            .iter()
            .enumerate()
            .max_by_key(|(_, count)| *count)
            .unwrap();
        let peak_dots = *peak_dots;

        // Check if any dots were observed
        if peak_dots == 0 {
            return Err(format!(
                "Progressive filling verification failed: no placeholder dots observed in any snapshot. \
                 This suggests data filled too quickly to observe progressive rendering, or the command \
                 doesn't use progressive rendering. Dots progression: {:?}",
                dots
            ));
        }

        // Verify dots decrease after peak
        if peak_index == dots.len() - 1 {
            return Err(format!(
                "Progressive filling verification failed: peak dots ({}) at final snapshot. \
                 Expected dots to decrease after peak. Dots progression: {:?}",
                peak_dots, dots
            ));
        }

        // Check that final dots are less than peak
        let final_dots = dots[dots.len() - 1];
        if final_dots >= peak_dots {
            return Err(format!(
                "Progressive filling verification failed: final dots ({}) >= peak dots ({}). \
                 Expected decrease after peak. Dots progression: {:?}",
                final_dots, peak_dots, dots
            ));
        }

        Ok(())
    }
}

/// Capture progressive output from a wt command
///
/// Executes the command in a PTY and captures multiple snapshots of the terminal
/// screen as output is rendered. Uses `vt100` terminal emulator to properly
/// handle ANSI escape sequences (cursor movements, clear line, colors, etc.).
///
/// # Arguments
///
/// * `repo` - Test repository
/// * `subcommand` - wt subcommand (e.g., "list")
/// * `args` - Arguments to the subcommand
/// * `options` - Capture options (interval, timeout, terminal size)
///
/// # Returns
///
/// `ProgressiveOutput` containing all captured snapshots and exit code
pub fn capture_progressive_output(
    repo: &TestRepo,
    subcommand: &str,
    args: &[&str],
    options: ProgressiveCaptureOptions,
) -> ProgressiveOutput {
    let start_time = Instant::now();

    let pair = super::open_pty_with_size(options.terminal_size.0, options.terminal_size.1);

    // Build command
    let mut cmd = CommandBuilder::new(get_cargo_bin("wt"));
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(repo.root_path());

    // Set test environment
    configure_pty_environment(&mut cmd, repo);

    // Spawn command in PTY
    let mut child = pair.slave.spawn_command(cmd).unwrap_or_else(|_| {
        panic!(
            "Failed to spawn 'wt {}' in PTY at {}",
            subcommand,
            repo.root_path().display()
        )
    });
    drop(pair.slave);

    // Create terminal emulator
    let mut parser = vt100::Parser::new(
        options.terminal_size.0,
        options.terminal_size.1,
        0, // No scrollback
    );

    // Read output and capture snapshots
    let mut reader = pair.master.try_clone_reader().unwrap();

    let mut snapshots = Vec::new();
    let mut last_snapshot_time = Instant::now();
    let mut last_snapshot_bytes = 0;
    let mut last_snapshot_text = String::new();
    let mut total_bytes = 0;

    loop {
        let mut temp_buf = [0u8; 4096];

        // Helper closure to run the drain logic - extracted to avoid duplication
        // between the EOF case and the WouldBlock+child-exited case.
        let drain_and_capture_final = |parser: &mut vt100::Parser,
                                       reader: &mut Box<dyn std::io::Read + Send>,
                                       snapshots: &mut Vec<OutputSnapshot>,
                                       last_snapshot_text: &str,
                                       total_bytes: &mut usize,
                                       start_time: Instant| {
            let backoff = ExponentialBackoff::default();
            let mut attempt = 0u32;
            let mut consecutive_no_data = 0u32;
            let drain_start = Instant::now();

            loop {
                if drain_start.elapsed() > backoff.timeout {
                    break;
                }

                let mut buf = [0u8; 4096];
                match reader.read(&mut buf) {
                    Ok(0) => {
                        consecutive_no_data += 1;
                        let min_wait_elapsed =
                            drain_start.elapsed() >= Duration::from_millis(MIN_DRAIN_WAIT_MS);
                        if consecutive_no_data >= STABLE_READ_THRESHOLD && min_wait_elapsed {
                            break;
                        }
                        backoff.sleep(attempt);
                        attempt += 1;
                    }
                    Ok(n) => {
                        consecutive_no_data = 0;
                        attempt = 0;
                        *total_bytes += n;
                        parser.process(&buf[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        backoff.sleep(attempt);
                        attempt += 1;
                    }
                    Err(_) => break,
                }
            }

            // Capture final snapshot
            let screen = parser.screen();
            let final_text = screen.contents();
            if final_text != last_snapshot_text {
                snapshots.push(OutputSnapshot {
                    timestamp: start_time.elapsed(),
                    visible_text: final_text,
                });
            }
        };

        match reader.read(&mut temp_buf) {
            Ok(0) => {
                // EOF - on Linux PTYs may signal EOF before all buffered data is available.
                // Run drain logic to capture everything before breaking.
                drain_and_capture_final(
                    &mut parser,
                    &mut reader,
                    &mut snapshots,
                    &last_snapshot_text,
                    &mut total_bytes,
                    start_time,
                );
                break;
            }
            Ok(n) => {
                total_bytes += n;

                // Feed bytes to terminal emulator
                parser.process(&temp_buf[..n]);

                // Check if we should take a snapshot based on strategy
                let should_snapshot = match options.strategy {
                    CaptureStrategy::TimeInterval(interval) => {
                        last_snapshot_time.elapsed() >= interval
                    }
                    CaptureStrategy::ByteInterval(byte_interval) => {
                        total_bytes >= last_snapshot_bytes + byte_interval
                    }
                };

                if should_snapshot {
                    let screen = parser.screen();
                    let current_text = screen.contents();

                    // Only snapshot if output changed and meets minimum threshold
                    let meets_threshold = match options.strategy {
                        CaptureStrategy::TimeInterval(_) => {
                            last_snapshot_time.elapsed().as_millis()
                                >= options.min_change_threshold as u128
                        }
                        CaptureStrategy::ByteInterval(_) => {
                            total_bytes >= last_snapshot_bytes + options.min_change_threshold
                        }
                    };

                    if current_text != last_snapshot_text && meets_threshold {
                        snapshots.push(OutputSnapshot {
                            timestamp: start_time.elapsed(),
                            visible_text: current_text.clone(),
                        });

                        last_snapshot_text = current_text;
                        last_snapshot_time = Instant::now();
                        last_snapshot_bytes = total_bytes;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Check if child exited
                if let Ok(Some(_)) = child.try_wait() {
                    drain_and_capture_final(
                        &mut parser,
                        &mut reader,
                        &mut snapshots,
                        &last_snapshot_text,
                        &mut total_bytes,
                        start_time,
                    );
                    break;
                }

                if start_time.elapsed() > options.timeout {
                    panic!(
                        "Timeout waiting for command to complete after {:?}. \
                         Captured {} snapshots, last at {:?}, {} total bytes",
                        options.timeout,
                        snapshots.len(),
                        snapshots.last().map(|s| s.timestamp),
                        total_bytes
                    );
                }

                std::thread::sleep(Duration::from_millis(OUTPUT_POLL_INTERVAL_MS));
            }
            Err(e) => panic!("Failed to read PTY output: {}", e),
        }
    }

    // Wait for exit code
    let exit_status = child
        .wait()
        .unwrap_or_else(|_| panic!("Failed to wait for 'wt {}' to exit", subcommand));
    let exit_code = exit_status.exit_code() as i32;
    let total_duration = start_time.elapsed();

    ProgressiveOutput {
        stages: snapshots,
        exit_code,
        total_duration,
    }
}

/// Configure PTY command with test environment variables
fn configure_pty_environment(cmd: &mut CommandBuilder, repo: &TestRepo) {
    // Clear environment
    cmd.env_clear();

    // Basic environment
    cmd.env(
        "HOME",
        home::home_dir().unwrap().to_string_lossy().to_string(),
    );
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );

    // Test environment (from TestRepo::test_env_vars)
    for (key, value) in repo.test_env_vars() {
        cmd.env(key, value);
    }

    // Pass through LLVM coverage profiling environment for subprocess coverage collection.
    // When running under cargo-llvm-cov, spawned binaries need LLVM_PROFILE_FILE to record
    // their coverage data.
    for key in [
        "LLVM_PROFILE_FILE",
        "CARGO_LLVM_COV",
        "CARGO_LLVM_COV_TARGET_DIR",
    ] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progressive_capture_options_default() {
        let opts = ProgressiveCaptureOptions::default();
        assert!(matches!(opts.strategy, CaptureStrategy::TimeInterval(_)));
        assert_eq!(opts.timeout, Duration::from_secs(10));
        assert_eq!(opts.terminal_size, (48, 150));
        assert_eq!(opts.min_change_threshold, 100);
    }

    #[test]
    fn test_progressive_capture_options_byte_interval() {
        let opts = ProgressiveCaptureOptions::with_byte_interval(500);
        assert!(matches!(opts.strategy, CaptureStrategy::ByteInterval(500)));
        assert_eq!(opts.timeout, Duration::from_secs(10));
        assert_eq!(opts.terminal_size, (48, 150));
        assert_eq!(opts.min_change_threshold, 100);
    }

    #[test]
    fn test_progressive_output_samples() {
        let stages = vec![
            OutputSnapshot {
                timestamp: Duration::from_millis(0),
                visible_text: "stage 0".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(50),
                visible_text: "stage 1".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(100),
                visible_text: "stage 2".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(150),
                visible_text: "stage 3".to_string(),
            },
        ];

        let output = ProgressiveOutput {
            stages,
            exit_code: 0,
            total_duration: Duration::from_millis(150),
        };

        // Test samples
        let samples = output.samples(2);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].visible_text, "stage 0");
        assert_eq!(samples[1].visible_text, "stage 3");

        let samples = output.samples(3);
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].visible_text, "stage 0");
        assert_eq!(samples[1].visible_text, "stage 2");
        assert_eq!(samples[2].visible_text, "stage 3");
    }

    #[test]
    fn test_dots_decrease_verification() {
        let stages = vec![
            OutputSnapshot {
                timestamp: Duration::from_millis(0),
                visible_text: "⋯ ⋯ ⋯ ⋯ ⋯".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(50),
                visible_text: "data ⋯ ⋯ ⋯".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(100),
                visible_text: "data data ⋯".to_string(),
            },
            OutputSnapshot {
                timestamp: Duration::from_millis(150),
                visible_text: "data data data".to_string(),
            },
        ];

        let output = ProgressiveOutput {
            stages,
            exit_code: 0,
            total_duration: Duration::from_millis(150),
        };

        // Should verify successfully
        assert!(output.verify_progressive_filling().is_ok());

        // Check dots count
        let dots = output.dots_per_stage();
        assert_eq!(dots, vec![5, 3, 1, 0]);
    }
}
