//! Durable finite provider-call budgets.

use metrics::counter;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Database;

/// Closed provider request class stored in the usage ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestClass {
    /// Single-use OAuth authorization-code exchange.
    CodeExchange,
    /// Account identity and type discovery.
    AccountDiscovery,
    /// Granted-permission discovery.
    PermissionDiscovery,
    /// Provider-supported token refresh.
    TokenRefresh,
    /// Optional provider-side token revocation.
    TokenRevoke,
}

impl RequestClass {
    /// Stable database and metric value.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::CodeExchange => "code_exchange",
            Self::AccountDiscovery => "account_discovery",
            Self::PermissionDiscovery => "permission_discovery",
            Self::TokenRefresh => "token_refresh",
            Self::TokenRevoke => "token_revoke",
        }
    }
}

/// Closed terminal classification without response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageOutcome {
    /// Provider answered successfully.
    Succeeded,
    /// Provider refused authentication or authorization.
    Authentication,
    /// Provider rejected the request grammar.
    Validation,
    /// Provider rate-limited the call.
    RateLimited,
    /// Provider failed with a server response.
    Server,
    /// Transport failed before a usable response.
    Network,
    /// Body exceeded the cap or violated its strict schema.
    ResponseRefused,
    /// Selected profile documents no such provider operation.
    ProviderUnsupported,
}

impl UsageOutcome {
    const fn wire_value(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Authentication => "authentication",
            Self::Validation => "validation",
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Network => "network",
            Self::ResponseRefused => "response_refused",
            Self::ProviderUnsupported => "provider_unsupported",
        }
    }
}

/// Bounded values from Meta's documented usage headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetaUsage {
    /// Percentage of call-count allowance consumed.
    pub call_count_percent: Option<u8>,
    /// Percentage of CPU allowance consumed.
    pub cpu_time_percent: Option<u8>,
    /// Percentage of total-time allowance consumed.
    pub total_time_percent: Option<u8>,
}

/// One committed attempt reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageReservation {
    /// Durable row identity.
    pub usage_id: Uuid,
    /// One-based ordinal in its operation.
    pub attempt_ordinal: u32,
    /// Bounded class reserved for this attempt.
    pub request_class: RequestClass,
}

/// Finite budget for one top-level operation.
#[derive(Debug)]
pub struct ProviderBudget {
    database: Database,
    operation_id: Uuid,
    account_id: Option<Uuid>,
    limit: u32,
    next_ordinal: u32,
}

impl ProviderBudget {
    /// Creates an in-process budget whose reservations are still durable.
    #[must_use]
    pub const fn new(
        database: Database,
        operation_id: Uuid,
        account_id: Option<Uuid>,
        limit: u32,
    ) -> Self {
        Self {
            database,
            operation_id,
            account_id,
            limit,
            next_ordinal: 1,
        }
    }

    /// Commits an attempt reservation before the caller invokes transport.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::Exhausted`] without inserting or invoking transport when no unit
    /// remains, or [`BudgetError::Database`] when reservation cannot become durable.
    pub async fn reserve(
        &mut self,
        request_class: RequestClass,
        started_at: OffsetDateTime,
    ) -> Result<UsageReservation, BudgetError> {
        if self.next_ordinal > self.limit {
            return Err(BudgetError::Exhausted);
        }
        let reservation = UsageReservation {
            usage_id: Uuid::now_v7(),
            attempt_ordinal: self.next_ordinal,
            request_class,
        };
        let stored_ordinal =
            i32::try_from(reservation.attempt_ordinal).map_err(|_| BudgetError::InvalidMetadata)?;
        sqlx::query(
            "insert into instagram_archive.provider_api_usage
             (usage_id, operation_id, account_id, request_class, attempt_ordinal, state,
              started_at)
             values ($1, $2, $3, $4, $5, 'started', $6)",
        )
        .bind(reservation.usage_id)
        .bind(self.operation_id)
        .bind(self.account_id)
        .bind(request_class.wire_value())
        .bind(stored_ordinal)
        .bind(started_at)
        .execute(self.database.pool())
        .await
        .map_err(BudgetError::Database)?;
        counter!(
            "instagram_provider_api_attempts_total",
            "request_class" => request_class.wire_value(),
            "state" => "reserved",
        )
        .increment(1);
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .ok_or(BudgetError::InvalidMetadata)?;
        Ok(reservation)
    }

    /// Marks a reservation terminal with bounded redacted metadata.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError`] when metadata is invalid or the update fails.
    pub async fn complete(
        &self,
        reservation: UsageReservation,
        outcome: UsageOutcome,
        http_status: Option<u16>,
        usage: MetaUsage,
        finished_at: OffsetDateTime,
    ) -> Result<(), BudgetError> {
        if http_status.is_some_and(|status| !(100..=599).contains(&status))
            || [
                usage.call_count_percent,
                usage.cpu_time_percent,
                usage.total_time_percent,
            ]
            .into_iter()
            .flatten()
            .any(|percentage| percentage > 100)
        {
            return Err(BudgetError::InvalidMetadata);
        }
        let result = sqlx::query(
            "update instagram_archive.provider_api_usage
             set state = 'completed', outcome = $3, http_status = $4,
                 call_count_percent = $5, cpu_time_percent = $6, total_time_percent = $7,
                 finished_at = $8
             where usage_id = $1 and operation_id = $2 and state = 'started'",
        )
        .bind(reservation.usage_id)
        .bind(self.operation_id)
        .bind(outcome.wire_value())
        .bind(http_status.map(i32::from))
        .bind(usage.call_count_percent.map(i16::from))
        .bind(usage.cpu_time_percent.map(i16::from))
        .bind(usage.total_time_percent.map(i16::from))
        .bind(finished_at)
        .execute(self.database.pool())
        .await
        .map_err(BudgetError::Database)?;
        if result.rows_affected() != 1 {
            return Err(BudgetError::InvalidMetadata);
        }
        counter!(
            "instagram_provider_api_attempts_total",
            "request_class" => reservation.request_class.wire_value(),
            "state" => outcome.wire_value(),
        )
        .increment(1);
        Ok(())
    }
}

/// Durable provider-budget failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BudgetError {
    /// No attempt remains; transport must not be invoked.
    #[error("provider call budget is exhausted")]
    Exhausted,
    /// A usage percentage or status cannot fit the bounded ledger.
    #[error("provider usage metadata is invalid")]
    InvalidMetadata,
    /// The redacted usage ledger could not be written.
    #[error("provider usage accounting failed")]
    Database(#[source] sqlx::Error),
}
