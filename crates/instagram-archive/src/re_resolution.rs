//! Finite public re-resolution scheduling without privacy-terminal retries.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{Database, PersistenceError};

/// Last supported observation used by automatic retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorResolutionState {
    /// Previously resolved public content.
    Resolved,
    /// Supported resolver failed transiently.
    ResolverFailed,
    /// Provider or network was temporarily unavailable.
    TemporarilyUnavailable,
    /// Provider evidence established privacy or inaccessibility.
    PrivateOrInaccessible,
    /// Provider evidence established deletion.
    Deleted,
    /// The source is outside supported public resolution.
    Unsupported,
    /// A permanent unavailable classification other than privacy/deletion.
    PermanentlyUnavailable,
}

/// One owner-held persisted scheduling candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReResolutionCandidate {
    /// Stable capture identity.
    pub capture_id: Uuid,
    /// Original explicit-capture time used by the recency policy.
    pub captured_at: OffsetDateTime,
    /// Persisted due time.
    pub next_resolution_at: OffsetDateTime,
    /// Whether local privacy deletion already removed the library holding.
    pub locally_removed: bool,
    /// Last supported public observation class.
    pub prior_state: PriorResolutionState,
}

/// Why a candidate was deterministically skipped before network I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReResolutionSkipReason {
    /// Its policy deadline is in the future.
    NotDue,
    /// The original capture is outside the configured recency window.
    TooOld,
    /// Local privacy deletion is terminal for automatic work.
    LocallyRemoved,
    /// Private or deleted provider state requires a new explicit acquisition.
    PrivacyTerminal,
    /// The observation is outside retryable public resolution.
    Unsupported,
    /// The finite run item ceiling is exhausted.
    ItemBudget,
    /// The finite run request ceiling is exhausted.
    RequestBudget,
    /// The finite run response-byte ceiling cannot admit the lease.
    ByteBudget,
    /// The run deadline has passed.
    Deadline,
    /// The finite in-flight request ceiling is exhausted.
    Concurrency,
    /// The approved provider endpoint allowance is unavailable or exhausted.
    ProviderBudget,
}

/// Mutable counters and immutable finite ceilings for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReResolutionBudget {
    /// Maximum admitted items.
    pub max_items: u32,
    /// Already admitted items.
    pub items_admitted: u32,
    /// Maximum started requests.
    pub max_requests: u32,
    /// Already reserved requests.
    pub requests_reserved: u32,
    /// Maximum accepted or leased response bytes.
    pub max_response_bytes: u64,
    /// Already leased response bytes.
    pub response_bytes: u64,
    /// Maximum concurrent requests.
    pub max_concurrency: u32,
    /// Currently claimed requests.
    pub in_flight: u32,
    /// Run deadline.
    pub deadline_at: OffsetDateTime,
    /// Remaining provider endpoint requests; unknown fails closed.
    pub endpoint_remaining: Option<u32>,
}

/// Pre-I/O admission result for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReResolutionAttemptOutcome {
    /// Every counter was reserved before the resolver call began.
    Started,
    /// No request began and counters stayed unchanged.
    Skipped(ReResolutionSkipReason),
}

/// Result classification after a supported public response was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshClassification {
    /// Normalized source content changed and requires an update fact.
    Updated,
    /// Normalized source content is equal; raw observation evidence still appends.
    Unchanged,
}

/// Accounting contract shared by the worker and publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshAccounting {
    /// Equal-versus-changed classification.
    pub classification: RefreshClassification,
    /// Every accepted public observation appends evidence.
    pub evidence_appended: bool,
    /// Only changed normalized content emits an update fact.
    pub update_emitted: bool,
}

/// Classifies normalized digests after raw evidence has been accepted.
#[must_use]
pub fn classify_refresh(previous_digest: &[u8], current_digest: &[u8]) -> RefreshAccounting {
    let unchanged = previous_digest == current_digest;
    RefreshAccounting {
        classification: if unchanged {
            RefreshClassification::Unchanged
        } else {
            RefreshClassification::Updated
        },
        evidence_appended: true,
        update_emitted: !unchanged,
    }
}

/// Reserves every finite guard before invoking one resolver call.
pub fn attempt_with_budget<F>(
    budget: &mut ReResolutionBudget,
    expected_response_bytes: u64,
    now: OffsetDateTime,
    mut resolver: F,
) -> ReResolutionAttemptOutcome
where
    F: FnMut(),
{
    let refusal = if budget.max_items == 0 || budget.items_admitted >= budget.max_items {
        Some(ReResolutionSkipReason::ItemBudget)
    } else if budget.max_requests == 0 || budget.requests_reserved >= budget.max_requests {
        Some(ReResolutionSkipReason::RequestBudget)
    } else if budget
        .response_bytes
        .checked_add(expected_response_bytes)
        .is_none_or(|leased| leased > budget.max_response_bytes)
    {
        Some(ReResolutionSkipReason::ByteBudget)
    } else if now >= budget.deadline_at {
        Some(ReResolutionSkipReason::Deadline)
    } else if budget.max_concurrency == 0 || budget.in_flight >= budget.max_concurrency {
        Some(ReResolutionSkipReason::Concurrency)
    } else if budget
        .endpoint_remaining
        .is_none_or(|remaining| remaining == 0)
    {
        Some(ReResolutionSkipReason::ProviderBudget)
    } else {
        None
    };
    if let Some(reason) = refusal {
        return ReResolutionAttemptOutcome::Skipped(reason);
    }
    budget.items_admitted = budget.items_admitted.saturating_add(1);
    budget.requests_reserved = budget.requests_reserved.saturating_add(1);
    budget.response_bytes = budget
        .response_bytes
        .saturating_add(expected_response_bytes);
    budget.in_flight = budget.in_flight.saturating_add(1);
    budget.endpoint_remaining = budget.endpoint_remaining.map(|remaining| remaining - 1);
    resolver();
    budget.in_flight = budget.in_flight.saturating_sub(1);
    ReResolutionAttemptOutcome::Started
}

/// Rechecks one selected capture immediately before finite budget admission.
///
/// # Errors
///
/// Returns a persistence failure when current owner-held state cannot be checked.
#[expect(
    clippy::too_many_arguments,
    reason = "the claim seam exposes every owner, timing, byte, budget, and I/O boundary used by deterministic tests"
)]
pub async fn claim_capture_for_resolution<F>(
    database: &Database,
    user_ref: Uuid,
    capture_id: Uuid,
    budget: &mut ReResolutionBudget,
    expected_response_bytes: u64,
    now: OffsetDateTime,
    recency_window: Duration,
    resolver: F,
) -> Result<ReResolutionAttemptOutcome, PersistenceError>
where
    F: FnMut(),
{
    let candidate: Option<(OffsetDateTime, Option<OffsetDateTime>, String, bool)> = sqlx::query_as(
        "select c.captured_at, c.next_resolution_at, c.status, \
                    exists(select 1 from instagram_archive.local_source_removals removal \
                           where removal.user_ref = c.user_ref \
                             and removal.capture_id = c.capture_id) \
             from instagram_archive.captures c \
             where c.capture_id = $1 and c.user_ref = $2",
    )
    .bind(capture_id)
    .bind(user_ref)
    .fetch_optional(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    let Some((captured_at, next_resolution_at, status, locally_removed)) = candidate else {
        return Ok(ReResolutionAttemptOutcome::Skipped(
            ReResolutionSkipReason::LocallyRemoved,
        ));
    };
    let reason = if locally_removed || status == "tombstoned" {
        Some(ReResolutionSkipReason::LocallyRemoved)
    } else if captured_at < now - recency_window {
        Some(ReResolutionSkipReason::TooOld)
    } else if next_resolution_at.is_none_or(|deadline| deadline > now) {
        Some(ReResolutionSkipReason::NotDue)
    } else if !matches!(status.as_str(), "resolved" | "unavailable" | "failed") {
        Some(ReResolutionSkipReason::Unsupported)
    } else {
        None
    };
    if let Some(reason) = reason {
        return Ok(ReResolutionAttemptOutcome::Skipped(reason));
    }
    Ok(attempt_with_budget(
        budget,
        expected_response_bytes,
        now,
        resolver,
    ))
}

/// Ordered selection result persisted by a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReResolutionSelection {
    /// Due recent retryable captures in deterministic order.
    pub admitted: Vec<Uuid>,
    /// Skipped captures with bounded reasons in deterministic order.
    pub skipped: Vec<(Uuid, ReResolutionSkipReason)>,
}

/// Selects retryable captures without making a provider request.
#[must_use]
pub fn select_candidates(
    mut candidates: Vec<ReResolutionCandidate>,
    now: OffsetDateTime,
    recency_window: Duration,
) -> ReResolutionSelection {
    candidates.sort_by_key(|candidate| (candidate.next_resolution_at, candidate.capture_id));
    let oldest_allowed = now - recency_window;
    let mut admitted = Vec::new();
    let mut skipped = Vec::new();
    for candidate in candidates {
        let reason = if candidate.locally_removed {
            Some(ReResolutionSkipReason::LocallyRemoved)
        } else if candidate.captured_at < oldest_allowed {
            Some(ReResolutionSkipReason::TooOld)
        } else if candidate.next_resolution_at > now {
            Some(ReResolutionSkipReason::NotDue)
        } else {
            match candidate.prior_state {
                PriorResolutionState::Resolved
                | PriorResolutionState::ResolverFailed
                | PriorResolutionState::TemporarilyUnavailable => None,
                PriorResolutionState::PrivateOrInaccessible | PriorResolutionState::Deleted => {
                    Some(ReResolutionSkipReason::PrivacyTerminal)
                }
                PriorResolutionState::Unsupported
                | PriorResolutionState::PermanentlyUnavailable => {
                    Some(ReResolutionSkipReason::Unsupported)
                }
            }
        };
        if let Some(reason) = reason {
            skipped.push((candidate.capture_id, reason));
        } else {
            admitted.push(candidate.capture_id);
        }
    }
    ReResolutionSelection { admitted, skipped }
}

/// Derives a stable spread-out retry deadline for one capture.
///
/// The same capture and policy inputs always produce the same finite jitter,
/// avoiding synchronized provider traffic without storing randomness.
#[must_use]
pub fn deterministic_next_resolution_at(
    capture_id: Uuid,
    observed_at: OffsetDateTime,
    base_interval: Duration,
    max_jitter: Duration,
) -> Option<OffsetDateTime> {
    let jitter_ceiling = u64::try_from(max_jitter.whole_seconds()).ok()?;
    let material = u64::from_be_bytes(capture_id.as_bytes().get(..8)?.try_into().ok()?);
    let jitter_seconds = material % jitter_ceiling.saturating_add(1);
    observed_at
        .checked_add(base_interval)?
        .checked_add(Duration::seconds(i64::try_from(jitter_seconds).ok()?))
}
