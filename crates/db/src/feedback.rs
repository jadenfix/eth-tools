//! Typed read queries against the `feedback` table + W4 worker upserts.
//!
//! Phase-5 surface: list-by-agent only (powers MCP `read_feedback`).
//! Phase-4 W4 surface: upsert + revoke + response append, each takes a
//! `&mut PgConnection` so worker bodies can fold them into the same
//! transaction as the cursor advance (plan §3 invariant 5).

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};

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

/// Per-feedback rolling summary aggregated by W4 — count + arithmetic mean
/// of `value` over the **majority-decimals** subset, plus that decimals
/// value itself. Multi-decimals handling (the "mode-of-decimals" question
/// from the W4 spec): we compute the mode of `value_decimals` across all
/// non-revoked feedback for the agent, then average only the rows whose
/// `value_decimals` equals the mode. Rationale in the worker docs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSummary {
    pub chain_id: i64,
    pub agent_id: BigDecimal,
    pub count: i64,
    pub mode_decimals: i16,
    pub mean_value: BigDecimal,
}

/// Compute the rolling summary for one agent from the canonical `feedback`
/// rows. Uses a single SQL round-trip so a 1000-feedback agent doesn't
/// stream rows back to Rust just to mean them. Excludes revoked feedback.
///
/// Returns `Ok(None)` if the agent has zero non-revoked feedback rows.
pub async fn compute_summary(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
) -> Result<Option<AgentSummary>, sqlx::Error> {
    // Pick the mode of value_decimals among non-revoked feedback. Tie
    // break: largest decimals wins (preserves precision when bimodal).
    // Then average value over only those rows. AVG returns NULL if no
    // rows match; the outer SELECT collapses to NULL across the board.
    let row: Option<(Option<i64>, Option<i16>, Option<BigDecimal>)> = sqlx::query_as(
        "WITH per_decimals AS (
             SELECT value_decimals, COUNT(*) AS n
             FROM feedback
             WHERE chain_id = $1 AND agent_id = $2 AND is_revoked = FALSE
             GROUP BY value_decimals
         ),
         mode_row AS (
             SELECT value_decimals
             FROM per_decimals
             ORDER BY n DESC, value_decimals DESC
             LIMIT 1
         )
         SELECT
             (SELECT COUNT(*) FROM feedback
                WHERE chain_id = $1 AND agent_id = $2 AND is_revoked = FALSE)
                ::BIGINT AS count,
             (SELECT value_decimals FROM mode_row)::SMALLINT AS mode_decimals,
             (SELECT AVG(value) FROM feedback
                WHERE chain_id = $1
                  AND agent_id = $2
                  AND is_revoked = FALSE
                  AND value_decimals = (SELECT value_decimals FROM mode_row))
                ::NUMERIC AS mean_value",
    )
    .bind(chain_id)
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((Some(count), Some(mode_decimals), Some(mean_value))) if count > 0 => {
            Ok(Some(AgentSummary {
                chain_id,
                agent_id: agent_id.clone(),
                count,
                mode_decimals,
                mean_value,
            }))
        }
        _ => Ok(None),
    }
}

/// Upsert a single `NewFeedback` event row. Idempotent on the composite key
/// `(chain_id, agent_id, client_address, feedback_index)` — re-running the
/// worker over the same range refreshes the row in place rather than
/// duplicating. Preserves `is_revoked = TRUE` if a prior FeedbackRevoked
/// event for the same key was processed earlier in the same tick.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_new_feedback(
    executor: &mut PgConnection,
    chain_id: i64,
    agent_id: &BigDecimal,
    client_address: &[u8],
    feedback_index: i64,
    value: &BigDecimal,
    value_decimals: i16,
    tag1: Option<&str>,
    tag2: Option<&str>,
    endpoint: Option<&str>,
    feedback_uri: Option<&str>,
    feedback_hash: Option<&[u8]>,
    tx_hash: &[u8],
    block_number: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO feedback (chain_id, agent_id, client_address, feedback_index,
                               value, value_decimals, tag1, tag2, endpoint,
                               feedback_uri, feedback_hash, is_revoked,
                               tx_hash, block_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, FALSE, $12, $13)
         ON CONFLICT (chain_id, agent_id, client_address, feedback_index) DO UPDATE
           SET value          = EXCLUDED.value,
               value_decimals = EXCLUDED.value_decimals,
               tag1           = EXCLUDED.tag1,
               tag2           = EXCLUDED.tag2,
               endpoint       = EXCLUDED.endpoint,
               feedback_uri   = EXCLUDED.feedback_uri,
               feedback_hash  = EXCLUDED.feedback_hash,
               tx_hash        = EXCLUDED.tx_hash,
               block_number   = EXCLUDED.block_number",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(client_address)
    .bind(feedback_index)
    .bind(value)
    .bind(value_decimals)
    .bind(tag1)
    .bind(tag2)
    .bind(endpoint)
    .bind(feedback_uri)
    .bind(feedback_hash)
    .bind(tx_hash)
    .bind(block_number)
    .execute(executor)
    .await
    .map(|_| ())
}

/// Mark a feedback row revoked. No-op (Ok(false)) if the row doesn't exist
/// — out-of-order events: the Revoked log was processed before NewFeedback.
/// Returns `Ok(true)` if exactly one row was updated.
pub async fn mark_revoked(
    executor: &mut PgConnection,
    chain_id: i64,
    agent_id: &BigDecimal,
    client_address: &[u8],
    feedback_index: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE feedback
            SET is_revoked = TRUE
          WHERE chain_id = $1 AND agent_id = $2
            AND client_address = $3 AND feedback_index = $4",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(client_address)
    .bind(feedback_index)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Upsert a `ResponseAppended` row into `feedback_responses`. The PK
/// includes `(responder, block_number, log_index)` so multiple appends from
/// the same responder land as distinct rows in on-chain order; idempotent
/// on re-scan because the PK collides exactly on the same on-chain event.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_response_appended(
    executor: &mut PgConnection,
    chain_id: i64,
    agent_id: &BigDecimal,
    client_address: &[u8],
    feedback_index: i64,
    responder: &[u8],
    block_number: i64,
    log_index: i32,
    response_uri: Option<&str>,
    response_hash: Option<&[u8]>,
    tx_hash: &[u8],
    appended_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO feedback_responses
            (chain_id, agent_id, client_address, feedback_index, responder,
             block_number, log_index, response_uri, response_hash, tx_hash,
             appended_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (chain_id, agent_id, client_address, feedback_index,
                      responder, block_number, log_index) DO UPDATE
           SET response_uri  = EXCLUDED.response_uri,
               response_hash = EXCLUDED.response_hash,
               tx_hash       = EXCLUDED.tx_hash,
               appended_at   = EXCLUDED.appended_at",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(client_address)
    .bind(feedback_index)
    .bind(responder)
    .bind(block_number)
    .bind(log_index)
    .bind(response_uri)
    .bind(response_hash)
    .bind(tx_hash)
    .bind(appended_at)
    .execute(executor)
    .await
    .map(|_| ())
}
