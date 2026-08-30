//! Operator-only repair for rows falsely credited by the retired logging transport.

/// Requeues the three supported `SocialSource` fact types that the logging transport
/// marked published without an external carrier.
///
/// # Errors
///
/// Returns [`sqlx::Error`] when the atomic repair cannot complete.
pub async fn repair_logging_outbox(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "select pg_advisory_xact_lock( \
         hashtextextended('ratatoskr-instagram:repair-logging-outbox', 0))",
    )
    .execute(&mut *transaction)
    .await?;
    let repaired = sqlx::query(
        "update instagram_archive.outbox_events \
         set published_at = null, next_attempt_at = transaction_timestamp() \
         where published_at is not null \
           and event_type in ( \
             'social.source.captured.v1', \
             'social.source.updated.v1', \
             'social.source.removed.v1' \
           )",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    Ok(repaired)
}
