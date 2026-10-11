//! The outbox relay loop (XR-021 CONTRACTS.md S02 rule 3, R2-03).
//!
//! It lives in the library, not the binary, so a test can drive the real loop against a real
//! broker and a real [`crate::RuntimeState`].

use std::time::Duration;

use ratatoskr_instagram_archive::Database;
use ratatoskr_instagram_archive::publishing::{EventTransport, run_once};

/// Drains the outbox forever, one bounded pass per interval.
pub async fn relay_outbox<T: EventTransport>(
    database: Database,
    transport: T,
    interval: Duration,
    batch_size: u32,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match run_once(database.pool(), &transport, batch_size).await {
            Ok(summary) if summary.failed > 0 => {
                tracing::warn!(
                    failed = summary.failed,
                    remaining = summary.remaining,
                    "outbox pass completed with failures"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "outbox pass could not run"),
        }
    }
}
