use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEBUG_ENV_VAR: &str = "WT_LIST_DEBUG";
const DEBUG_ENV_VALUE: &str = "1";

/// Operation names for tracking timing
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum OpName {
    CommitDetails,
    AheadBehind,
    UpstreamStatus,
    GitStatus,
    WorkingTreeDiff,
    WorkingTreeDiffWithMain,
    BranchDiff,
    GitOperation,
    // TODO(timing): Add PrStatus timing when CI/PR status collection is instrumented
    PrStatus,
    MergeConflicts,
    UserMarker,
}

/// Collects timing data for list operations.
/// Thread-safe for use with parallel iterators (rayon).
#[derive(Clone)]
pub struct TimingCollector {
    inner: Option<Arc<Mutex<TimingData>>>,
}

#[derive(Default)]
struct TimingData {
    timings: HashMap<OpName, Vec<Duration>>,
}

impl TimingCollector {
    /// Create a new collector if WT_LIST_DEBUG=1 is set
    pub fn new() -> Self {
        let enabled = env::var(DEBUG_ENV_VAR).ok().as_deref() == Some(DEBUG_ENV_VALUE);
        Self {
            inner: if enabled {
                Some(Arc::new(Mutex::new(TimingData::default())))
            } else {
                None
            },
        }
    }

    /// Execute a closure and record its timing
    pub fn measure<F, R>(&self, op: OpName, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        if self.inner.is_none() {
            return f();
        }

        let start = Instant::now();
        let result = f();
        let duration = start.elapsed();

        if let Some(ref data) = self.inner {
            let mut guard = data.lock().unwrap();
            guard.timings.entry(op).or_default().push(duration);
        }

        result
    }

    /// Print timing summary to stderr
    pub fn print_summary(&self) {
        let Some(ref data) = self.inner else {
            return;
        };

        let guard = data.lock().unwrap();
        if guard.timings.is_empty() {
            return;
        }

        log::debug!("WT_LIST_DEBUG timing summary:");

        let mut ops: Vec<_> = guard.timings.iter().collect();
        ops.sort_by_key(|(op, _)| op.as_ref());

        for (op, durations) in ops {
            let stats = compute_stats(durations);
            log::debug!(
                "  {:<30} median: {:>6.1}ms  p95: {:>6.1}ms  count: {}",
                op.as_ref(),
                stats.median_ms,
                stats.p95_ms,
                durations.len()
            );
        }
    }
}

struct Stats {
    median_ms: f64,
    p95_ms: f64,
}

fn compute_stats(durations: &[Duration]) -> Stats {
    let mut sorted: Vec<_> = durations.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median_ms = if sorted.is_empty() {
        0.0
    } else {
        let mid = sorted.len() / 2;
        if sorted.len() % 2 == 0 {
            (sorted[mid - 1] + sorted[mid]) / 2.0
        } else {
            sorted[mid]
        }
    };

    let p95_ms = if sorted.is_empty() {
        0.0
    } else {
        let idx = ((sorted.len() - 1) as f64 * 0.95).round() as usize;
        sorted[idx]
    };

    Stats { median_ms, p95_ms }
}
