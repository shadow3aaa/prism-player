use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Inner {
    offset_seconds: f64,
    last_instant: Instant,
    paused: bool,
}

/// GlobalClock provides a shared playback clock in seconds.
#[derive(Clone)]
pub struct GlobalClock {
    inner: Arc<RwLock<Inner>>,
}

impl GlobalClock {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                offset_seconds: 0.0,
                last_instant: Instant::now(),
                paused: false,
            })),
        }
    }

    /// Returns current clock time in seconds.
    pub fn now(&self) -> f64 {
        let i = self.inner.read();
        if i.paused {
            i.offset_seconds
        } else {
            let elapsed = Instant::now().duration_since(i.last_instant);
            i.offset_seconds + duration_to_secs(elapsed)
        }
    }

    pub fn pause(&self) {
        let mut i = self.inner.write();
        if !i.paused {
            // Update offset to current now
            let elapsed = Instant::now().duration_since(i.last_instant);
            i.offset_seconds += duration_to_secs(elapsed);
            i.paused = true;
        }
    }

    pub fn is_paused(&self) -> bool {
        let i = self.inner.read();
        i.paused
    }

    pub fn resume(&self) {
        let mut i = self.inner.write();
        if i.paused {
            i.last_instant = Instant::now();
            i.paused = false;
        }
    }
}

fn duration_to_secs(d: Duration) -> f64 {
    d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9
}

impl Default for GlobalClock {
    fn default() -> Self {
        Self::new()
    }
}
