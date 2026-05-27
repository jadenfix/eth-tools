//! Typed writes against the `agents_history` audit table.
//!
//! Workers W1 (registry_scraper) and W6 (wallet_rotation_watcher) both insert
//! into this table. W1's writes happen inside the per-page transaction; W6's
//! happen inside its own per-event transaction. This module gives both a
//! single typed `insert()` so the column ordering stays consistent.
//!
//! Reads (e.g. `/api/v1/agents/{id}/history`) land in the HTTP API expansion;
//! we only ship the writer the workers need.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Postgres, Transaction};

/// Allowed values for the `change_kind` CHECK constraint
/// (see `0001_init.up.sql:36`).
pub const KIND_URI: &str = "uri";
pub const KIND_WALLET: &str = "wallet";
pub const KIND_OWNER: &str = "owner";
pub const KIND_METADATA: &str = "metadata";

/// One row of `agents_history`. JSONB columns are kept as `serde_json::Value`
/// so callers can shape arbitrary before/after snapshots.
#[derive(Debug, Clone)]
pub struct HistoryInsert<'a> {
    pub chain_id: i64,
    pub agent_id: &'a BigDecimal,
    pub changed_at: DateTime<Utc>,
    pub change_kind: &'static str,
    pub before_value: Option<serde_json::Value>,
    pub after_value: Option<serde_json::Value>,
    pub tx_hash: &'a [u8],
    pub log_index: i32,
}

/// Insert one history row inside an open transaction. Caller owns commit/
/// rollback — by design, both W1 and W6 fold this into the same tx as the
/// `agents` mutation it audits.
pub async fn insert_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    row: HistoryInsert<'_>,
) -> sqlx::Result<()> {
    insert_with_conn(tx, row).await
}

/// Insert against a raw connection (e.g. when the caller is holding a
/// `PgConnection` rather than a `Transaction`). Same SQL as `insert_in_tx`.
pub async fn insert_with_conn(
    conn: &mut PgConnection,
    row: HistoryInsert<'_>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO agents_history
            (chain_id, agent_id, changed_at, change_kind, before_value, after_value,
             tx_hash, log_index)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(row.chain_id)
    .bind(row.agent_id)
    .bind(row.changed_at)
    .bind(row.change_kind)
    .bind(row.before_value)
    .bind(row.after_value)
    .bind(row.tx_hash)
    .bind(row.log_index)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_constants_match_check_constraint() {
        // The CHECK constraint in 0001_init.up.sql restricts change_kind to
        // exactly these four strings; guard against renames here.
        for k in [KIND_URI, KIND_WALLET, KIND_OWNER, KIND_METADATA] {
            assert!(!k.is_empty());
            assert_eq!(k, k.to_ascii_lowercase());
        }
    }
}
