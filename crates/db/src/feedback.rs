//! Typed read queries against the `feedback` table.
//!
//! Phase-5 surface: list-by-agent only (powers MCP `read_feedback`). Worker
//! upserts and the trust-score aggregator land in Phase-6/7.

use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// One row of the `feedback` table — wire shape mirrors the on-chain event.
/// `value` is kept as `BigDecimal` because the on-chain field is a `uint128`
/// (40 decimal digits) that JSON numbers cannot represent losslessly.
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

/// List all (non-revoked by default) feedback for a single agent. Ordered by
/// `(client_address, feedback_index)` so pagination is stable and tests are
/// deterministic. Phase-5 MCP doesn't need pagination — agents typically have
/// <100 feedback rows — so we return the full set capped at `LIMIT` for safety.
pub async fn list_for_agent(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
    include_revoked: bool,
) -> Result<Vec<FeedbackRow>, sqlx::Error> {
    let sql = "
        SELECT chain_id, agent_id, client_address, feedback_index,
               value, value_decimals, tag1, tag2, endpoint,
               feedback_uri, feedback_hash, is_revoked, tx_hash, block_number
        FROM feedback
        WHERE chain_id = $1
          AND agent_id = $2
          AND ($3::BOOL OR is_revoked = FALSE)
        ORDER BY client_address ASC, feedback_index ASC
        LIMIT 500
    ";
    sqlx::query_as::<_, FeedbackRow>(sql)
        .bind(chain_id)
        .bind(agent_id)
        .bind(include_revoked)
        .fetch_all(pool)
        .await
}
