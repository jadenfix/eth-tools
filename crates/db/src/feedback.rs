//! Typed reads against the `feedback` table (ERC-8004 reputation rows).
//!
//! Phase-4 write path lives in `crates/workers`; the API exposes only the
//! per-agent list for `/api/v1/reputation/:chain/:id` and a single-row insert
//! for the stubbed `POST /reputation/give` (the real on-chain write goes
//! through the wallet rails on `feat/phase-6-wallet-rails`).

use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// One row of the `feedback` table. Hex/byte fields stay raw; the API DTO
/// hex-encodes them.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FeedbackRow {
    pub chain_id: i64,
    pub agent_id: BigDecimal,
    pub client_address: Vec<u8>,
    pub feedback_index: i64,
    pub value: BigDecimal,
    pub value_decimals: i16,
    pub tag1: Option<String>,
    pub tag2: Option<String>,
    pub endpoint: Option<String>,
    pub feedback_uri: Option<String>,
    pub feedback_hash: Option<Vec<u8>>,
    pub is_revoked: bool,
    pub tx_hash: Vec<u8>,
    pub block_number: i64,
}

/// Read every feedback row for a given (chain, agent), newest block first.
/// Caller is the read handler `GET /api/v1/reputation/:chain/:id`.
pub async fn list_for_agent(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
    limit: i64,
) -> Result<Vec<FeedbackRow>, sqlx::Error> {
    let limit = limit.clamp(1, 500);
    sqlx::query_as::<_, FeedbackRow>(
        "SELECT chain_id, agent_id, client_address, feedback_index, value,
                value_decimals, tag1, tag2, endpoint, feedback_uri,
                feedback_hash, is_revoked, tx_hash, block_number
         FROM feedback
         WHERE chain_id = $1 AND agent_id = $2
         ORDER BY block_number DESC, feedback_index DESC
         LIMIT $3",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}
