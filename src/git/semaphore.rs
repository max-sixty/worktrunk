//! Simple semaphore for limiting concurrent git operations.
//!
//! Used to prevent mmap thrash when many parallel git commands access shared
//! resources like commit-graph and pack files.

use std::sync::{Arc, Condvar, Mutex};

/// A counting semaphore for limiting concurrency.
#[derive(Clone)]
pub struct Semaphore {
    state: Arc<(Mutex<usize>, Condvar)>,
}

/// RAII guard that releases a semaphore permit on drop.
pub struct SemaphoreGuard {
    state: Arc<(Mutex<usize>, Condvar)>,
}

impl Semaphore {
    /// Create a new semaphore with the given number of permits.
    pub fn new(permits: usize) -> Self {
        Self {
            state: Arc::new((Mutex::new(permits), Condvar::new())),
        }
    }

    /// Acquire a permit, blocking until one is available.
    ///
    /// Returns a guard that releases the permit when dropped.
    pub fn acquire(&self) -> SemaphoreGuard {
        let (lock, cvar) = &*self.state;
        let mut available = lock.lock().unwrap();

        // Wait until a permit is available
        while *available == 0 {
            available = cvar.wait(available).unwrap();
        }

        // Take a permit
        *available -= 1;

        SemaphoreGuard {
            state: Arc::clone(&self.state),
        }
    }
}

impl Drop for SemaphoreGuard {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.state;
        let mut available = lock.lock().unwrap();
        *available += 1;
        cvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_semaphore_limits_concurrency() {
        let sem = Semaphore::new(2);
        let counter = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];

        for _ in 0..10 {
            let sem = sem.clone();
            let counter = Arc::clone(&counter);
            let max_concurrent = Arc::clone(&max_concurrent);

            let handle = thread::spawn(move || {
                let _guard = sem.acquire();

                // Increment counter
                let current = counter.fetch_add(1, Ordering::SeqCst) + 1;

                // Track max concurrent
                max_concurrent.fetch_max(current, Ordering::SeqCst);

                // Simulate work
                thread::sleep(Duration::from_millis(10));

                // Decrement counter
                counter.fetch_sub(1, Ordering::SeqCst);
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should never have more than 2 threads running concurrently
        assert!(max_concurrent.load(Ordering::SeqCst) <= 2);
    }
}
