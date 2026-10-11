//! The publisher health flag the relay feeds and readiness reads (XR-021 CONTRACTS.md R2-03).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::pump::{PassSummary, PassVerdict};

/// Whether the outbox relay can currently publish. Cheap to clone; every clone shares one flag.
#[derive(Debug, Clone, Default)]
pub struct PublisherHealth {
    failing: Arc<AtomicBool>,
}

impl PublisherHealth {
    /// A publisher that has not failed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies what a pass learned. A pass that found nothing due changes nothing.
    pub fn record(&self, summary: &PassSummary) {
        match summary.verdict {
            PassVerdict::Failing => self.failing.store(true, Ordering::Release),
            PassVerdict::Healthy => self.failing.store(false, Ordering::Release),
            PassVerdict::Waiting => {}
        }
    }

    /// Whether the last conclusive pass found the publisher failing.
    #[must_use]
    pub fn is_failing(&self) -> bool {
        self.failing.load(Ordering::Acquire)
    }
}
