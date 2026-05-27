//! Typed writes against the `alerts` table.
//!
//! W8 (wallet_balance_keeper + watchdog) is the only producer today — it
//! raises three classes of alert:
//!   - `wallet.low`     (severity=warn)  — operator EOA below threshold
//!   - `worker.stale`   (severity=crit)  — a worker's last `ok=true` row
//!     is older than 3× that worker's cadence
//!   - `cursor.lag`     (severity=warn)  — a scrape cursor is more than
//!     100 blocks behind the chain head
//!
//! Reads (dashboard "open alerts" panel) land in the HTTP API expansion;
//! this module ships only the writer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// Allowed values for the `severity` CHECK constraint
/// (see `0001_init.up.sql:195`).
pub const SEV_INFO: &str = "info";
pub const SEV_WARN: &str = "warn";
pub const SEV_CRIT: &str = "crit";

/// One row of the `alerts` table — mirrors the DDL one-for-one.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AlertRow {
    pub id: i64,
    pub raised_at: DateTime<Utc>,
    pub severity: String,
    pub source: String,
    pub body: serde_json::Value,
    pub acknowledged: bool,
    pub ack_by: Option<String>,
    pub ack_at: Option<DateTime<Utc>>,
}

/// Raise one alert. `severity` SHOULD be one of [`SEV_INFO`], [`SEV_WARN`],
/// [`SEV_CRIT`] — anything else trips the table CHECK constraint and
/// surfaces as an `sqlx::Error::Database`.
///
/// Returns the row id so callers can correlate (e.g. log the alert id in
/// the same span that raised it).
pub async fn insert(
    pool: &PgPool,
    severity: &str,
    source: &str,
    body: serde_json::Value,
) -> sqlx::Result<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO alerts (severity, source, body)
         VALUES ($1, $2, $3)
         RETURNING id",
    )
    .bind(severity)
    .bind(source)
    .bind(body)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Count open (un-acknowledged) alerts for a given source — used by tests
/// and by the watchdog's "did we already raise this?" debouncer.
pub async fn count_open_by_source(pool: &PgPool, source: &str) -> sqlx::Result<i64> {
    let (c,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM alerts WHERE source = $1 AND acknowledged = FALSE")
            .bind(source)
            .fetch_one(pool)
            .await?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_constants_match_check_constraint() {
        for s in [SEV_INFO, SEV_WARN, SEV_CRIT] {
            assert!(!s.is_empty());
            assert_eq!(s, s.to_ascii_lowercase());
        }
    }
}
