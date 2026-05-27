//! `wallet_txs` table persistence (phase 6.2).
//!
//! Two-phase write pattern:
//!   1. Pre-send: INSERT with `status='submitted'`, returning the row id.
//!      A signing-path crash between INSERT and the actual RPC dispatch
//!      leaves a `submitted` row with the right nonce and tx_hash but no
//!      confirmation — caught by W7 reconciliation.
//!   2. Post-receipt: UPDATE to `status='confirmed'` (or `'reverted'`) with
//!      `gas_used` and `fee_usdc`.
//!
//! Schema (`crates/db/migrations/0001_init.up.sql`):
//!
//! ```sql
//! CREATE TABLE wallet_txs (
//!   id            BIGSERIAL PRIMARY KEY,
//!   chain_id      BIGINT NOT NULL,
//!   nonce         BIGINT,
//!   to_addr       BYTEA NOT NULL,
//!   selector      BYTEA NOT NULL,
//!   value_wei     NUMERIC(40, 0) NOT NULL DEFAULT 0,
//!   gas_used      BIGINT,
//!   fee_usdc      NUMERIC(20, 6),
//!   tx_hash       BYTEA UNIQUE,
//!   status        TEXT NOT NULL CHECK (status IN ('queued','submitted','confirmed','reverted','rejected')),
//!   reject_reason TEXT,
//!   api_key_id    UUID REFERENCES api_keys(id),
//!   created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
//! );
//! ```

use alloy_primitives::{Address, B256, U256};
use bigdecimal::BigDecimal;
use sqlx::PgPool;
use std::str::FromStr;
use uuid::Uuid;

/// Status transitions the column accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletTxStatus {
    Submitted,
    Confirmed,
    Reverted,
    Rejected,
}

impl WalletTxStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            WalletTxStatus::Submitted => "submitted",
            WalletTxStatus::Confirmed => "confirmed",
            WalletTxStatus::Reverted => "reverted",
            WalletTxStatus::Rejected => "rejected",
        }
    }
}

/// Pre-send INSERT. Returns the auto-assigned row id so the post-receipt
/// UPDATE can target it precisely (avoids `WHERE tx_hash = $1` races if a
/// caller ever submits with a duplicate hash — should be impossible, but
/// the UNIQUE constraint makes the INSERT itself the gate).
#[allow(clippy::too_many_arguments)]
pub async fn insert_submitted(
    pool: &PgPool,
    chain_id: u64,
    nonce: u64,
    to_addr: Address,
    selector: [u8; 4],
    value_wei: U256,
    tx_hash: B256,
    api_key_id: Option<Uuid>,
) -> Result<i64, sqlx::Error> {
    let value_str = value_wei.to_string();
    let value_dec = BigDecimal::from_str(&value_str)
        .map_err(|e| sqlx::Error::Decode(Box::new(std::io::Error::other(e.to_string()))))?;
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO wallet_txs \
           (chain_id, nonce, to_addr, selector, value_wei, tx_hash, status, api_key_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'submitted', $7) \
         RETURNING id",
    )
    .bind(i64::try_from(chain_id).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
    .bind(i64::try_from(nonce).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
    .bind(to_addr.as_slice())
    .bind(selector.as_slice())
    .bind(value_dec)
    .bind(tx_hash.as_slice())
    .bind(api_key_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Pre-send INSERT for a tx the rails rejected. Status='rejected', no
/// tx_hash. Captures the denial code so the audit ledger lines up with the
/// `denials` row written by the same call site.
pub async fn insert_rejected(
    pool: &PgPool,
    chain_id: u64,
    to_addr: Address,
    selector: [u8; 4],
    value_wei: U256,
    reject_reason: &str,
    api_key_id: Option<Uuid>,
) -> Result<i64, sqlx::Error> {
    let value_dec = BigDecimal::from_str(&value_wei.to_string())
        .map_err(|e| sqlx::Error::Decode(Box::new(std::io::Error::other(e.to_string()))))?;
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO wallet_txs \
           (chain_id, to_addr, selector, value_wei, status, reject_reason, api_key_id) \
         VALUES ($1, $2, $3, $4, 'rejected', $5, $6) \
         RETURNING id",
    )
    .bind(i64::try_from(chain_id).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
    .bind(to_addr.as_slice())
    .bind(selector.as_slice())
    .bind(value_dec)
    .bind(reject_reason)
    .bind(api_key_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Post-receipt update — transitions a submitted row to `confirmed` (or
/// `reverted`) and writes the actual gas + fee figures.
pub async fn finalize_receipt(
    pool: &PgPool,
    id: i64,
    status: WalletTxStatus,
    gas_used: u64,
    fee_usdc_cents: i64,
) -> Result<(), sqlx::Error> {
    // fee_usdc is NUMERIC(20, 6) — micro-USDC. Convert cents (1¢ = 10_000
    // micro-USDC) so the column carries the right scale.
    let fee_micro = i128::from(fee_usdc_cents) * 10_000;
    let fee_dec = BigDecimal::from_str(&format!("0.{:06}", fee_micro.unsigned_abs() % 1_000_000))
        .map_err(|e| sqlx::Error::Decode(Box::new(std::io::Error::other(e.to_string()))))?;
    // For values >= $1, prefer integer construction.
    let fee_dec = if fee_micro.unsigned_abs() >= 1_000_000 {
        BigDecimal::from_str(&format!(
            "{}.{:06}",
            fee_micro / 1_000_000,
            fee_micro.unsigned_abs() % 1_000_000
        ))
        .unwrap_or(fee_dec)
    } else {
        fee_dec
    };

    sqlx::query(
        "UPDATE wallet_txs SET status = $1, gas_used = $2, fee_usdc = $3 WHERE id = $4",
    )
    .bind(status.as_str())
    .bind(i64::try_from(gas_used).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
    .bind(fee_dec)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Helper for tests / debugging — returns the count of `wallet_txs` rows
/// matching `(status, chain_id)`.
pub async fn count_by_status(
    pool: &PgPool,
    status: WalletTxStatus,
    chain_id: u64,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM wallet_txs WHERE status = $1 AND chain_id = $2")
            .bind(status.as_str())
            .bind(i64::try_from(chain_id).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
            .fetch_one(pool)
            .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_strings_match_check_constraint() {
        // CHECK (status IN ('queued','submitted','confirmed','reverted','rejected'))
        for s in [
            WalletTxStatus::Submitted,
            WalletTxStatus::Confirmed,
            WalletTxStatus::Reverted,
            WalletTxStatus::Rejected,
        ] {
            let str_v = s.as_str();
            assert!(
                ["queued", "submitted", "confirmed", "reverted", "rejected"].contains(&str_v),
                "{str_v} not in CHECK constraint"
            );
        }
    }
}
