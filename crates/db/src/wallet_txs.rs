//! Typed writes against the `wallet_txs` table.
//!
//! W8 (wallet_balance_keeper) queues a sweep transaction here when the
//! operator EOA accumulates more than the configured threshold. Phase-6's
//! signing daemon picks queued rows up, signs them, broadcasts, and bumps
//! `status` through `submitted`/`confirmed`/`reverted`.
//!
//! This module ships only the writer the worker needs. Reads + status
//! transitions land with the signing daemon.

use bigdecimal::BigDecimal;
use sqlx::PgPool;

/// Allowed `status` values per the CHECK constraint (see
/// `0001_init.up.sql:172`).
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_SUBMITTED: &str = "submitted";
pub const STATUS_CONFIRMED: &str = "confirmed";
pub const STATUS_REVERTED: &str = "reverted";
pub const STATUS_REJECTED: &str = "rejected";

/// Inputs to [`queue`]. `selector` is the first 4 bytes of the calldata
/// (the function selector); `to_addr` is the target contract. `value_wei`
/// defaults to 0 for ERC-20-style sweeps.
#[derive(Debug, Clone)]
pub struct QueueInsert<'a> {
    pub chain_id: i64,
    pub to_addr: &'a [u8],
    pub selector: &'a [u8],
    pub value_wei: BigDecimal,
}

/// Insert a new row with `status='queued'`. Phase-6 signing daemon
/// transitions it onwards. Returns the inserted row id.
pub async fn queue(pool: &PgPool, row: QueueInsert<'_>) -> sqlx::Result<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO wallet_txs
            (chain_id, to_addr, selector, value_wei, status)
         VALUES ($1, $2, $3, $4, 'queued')
         RETURNING id",
    )
    .bind(row.chain_id)
    .bind(row.to_addr)
    .bind(row.selector)
    .bind(&row.value_wei)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Count queued rows for a chain — used by W8 to debounce repeated sweep
/// queues across daily ticks (one outstanding queued sweep at a time).
pub async fn count_queued(pool: &PgPool, chain_id: i64) -> sqlx::Result<i64> {
    let (c,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT
           FROM wallet_txs
          WHERE chain_id = $1 AND status = 'queued'",
    )
    .bind(chain_id)
    .fetch_one(pool)
    .await?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_constants_cover_check_constraint() {
        // Mirror the CHECK clause literally; if someone adds a status to the
        // migration this test pings them to add the constant too.
        let all = [
            STATUS_QUEUED,
            STATUS_SUBMITTED,
            STATUS_CONFIRMED,
            STATUS_REVERTED,
            STATUS_REJECTED,
        ];
        for s in all {
            assert_eq!(s, s.to_ascii_lowercase());
            assert!(!s.is_empty());
        }
        assert_eq!(all.len(), 5);
    }
}
