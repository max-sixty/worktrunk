//! Worktrunk library - git worktree management.
//!
//! # Global State
//!
//! Global state is intentionally scattered to live near its domain.
//! All globals use safe patterns (`OnceLock`, `LazyLock`, `AtomicU8`, `thread_local!`).
//!
//! | Module | Global | Purpose |
//! |--------|--------|---------|
//! | [`shell_exec`] | `CMD_SEMAPHORE` | Limits concurrent command execution |
//! | [`shell_exec`] | `TRACE_EPOCH` | Monotonic base for trace timestamps |
//! | [`shell_exec`] | `SHELL_CONFIG` | Cached platform shell detection |
//! | [`shell_exec`] | `COMMAND_TIMEOUT` | Thread-local timeout for Rayon workers |
//! | [`styling`] | `VERBOSITY` | CLI verbosity level (`-v`, `-vv`) |
//! | [`config::deprecation`](config) | `WARNED_PATHS` | Deduplicates deprecation warnings |
//! | [`config::user`](config) | `CONFIG_PATH` | `--config` CLI override |
//! | [`git::repository`](git) | `BASE_PATH` | `-C` flag override |
//!
//! Binary-only globals (declared in `main.rs`, not part of library API):
//! - `src/output/global.rs`: `OUTPUT_STATE` - Shell integration directive file
//! - `src/verbose_log.rs`: `VERBOSE_LOG` - Verbose log file handle

pub mod config;
pub mod git;
pub mod path;
pub mod shell;
pub mod shell_exec;
pub mod styling;
pub mod sync;
pub mod trace;
pub mod utils;

// Re-export HookType for convenience
pub use git::HookType;
