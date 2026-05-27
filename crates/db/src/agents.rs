//! Typed queries against the `agents` table.
//!
//! Reads only (Phase 1). Worker-side upserts land with `crates/workers` in
//! Phase 4.
//!
//! Pagination is keyset on `(updated_at DESC, chain_id ASC, agent_id ASC)` so
//! result pages are stable under concurrent worker writes — OFFSET pagination
//! would drop or duplicate rows when the scraper bumps `updated_at`.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// One row of the `agents` table. Hex/byte fields are kept as `Vec<u8>` here;
/// API DTOs serialize to `0x…` hex strings.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AgentRow {
    pub chain_id: i64,
    pub agent_id: BigDecimal,
    pub owner: Vec<u8>,
    pub agent_uri: Option<String>,
    pub agent_wallet: Option<Vec<u8>>,
    pub registered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Inputs to `list()`. All optional. `cursor` is opaque to callers — see
/// `Cursor::{encode,decode}` in the API layer.
#[derive(Debug, Clone, Default)]
pub struct ListParams {
    pub chain_id: Option<i64>,
    pub limit: i64,
    pub after: Option<KeysetCursor>,
}

/// Stable keyset pointer. The filter in `list()` re-derives the strict
/// "comes after" relation explicitly per column because Postgres tuple `<` is
/// purely lexicographic — it has no mixed-direction (DESC/ASC) semantics, so
/// using `(updated_at, chain_id, agent_id) < (…)` against an
/// `ORDER BY updated_at DESC, chain_id ASC, agent_id ASC` clause silently drops
/// rows whose `(chain_id, agent_id)` is *smaller* than the cursor on a tied
/// `updated_at`. The expanded boolean form below is correct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysetCursor {
    pub updated_at: DateTime<Utc>,
    pub chain_id: i64,
    pub agent_id: BigDecimal,
}

pub async fn list(pool: &PgPool, params: ListParams) -> Result<Vec<AgentRow>, sqlx::Error> {
    let limit = params.limit.clamp(1, 200);

    // Strict "after the cursor" predicate matching ORDER BY updated_at DESC,
    // chain_id ASC, agent_id ASC. See KeysetCursor doc above for why this
    // can't be expressed as a single tuple comparison.
    let base_sql = "
        SELECT chain_id, agent_id, owner, agent_uri, agent_wallet,
               registered_at, updated_at
        FROM agents
        WHERE ($1::BIGINT IS NULL OR chain_id = $1)
          AND (
                $2::TIMESTAMPTZ IS NULL
                OR updated_at < $2
                OR (updated_at = $2 AND chain_id > $3)
                OR (updated_at = $2 AND chain_id = $3 AND agent_id > $4)
              )
        ORDER BY updated_at DESC, chain_id ASC, agent_id ASC
        LIMIT $5
    ";

    sqlx::query_as::<_, AgentRow>(base_sql)
        .bind(params.chain_id)
        .bind(params.after.as_ref().map(|c| c.updated_at))
        .bind(params.after.as_ref().map(|c| c.chain_id))
        .bind(params.after.as_ref().map(|c| c.agent_id.clone()))
        .bind(limit)
        .fetch_all(pool)
        .await
}

pub async fn get_one(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
) -> Result<Option<AgentRow>, sqlx::Error> {
    sqlx::query_as::<_, AgentRow>(
        "SELECT chain_id, agent_id, owner, agent_uri, agent_wallet,
                registered_at, updated_at
         FROM agents
         WHERE chain_id = $1 AND agent_id = $2",
    )
    .bind(chain_id)
    .bind(agent_id)
    .fetch_optional(pool)
    .await
}

/// Count of rows; used by `/api/v1/health` to surface index liveness.
pub async fn count(pool: &PgPool, chain_id: Option<i64>) -> Result<i64, sqlx::Error> {
    let sql = "SELECT COUNT(*) AS c FROM agents WHERE ($1::BIGINT IS NULL OR chain_id = $1)";
    let (c,): (i64,) = sqlx::query_as(sql).bind(chain_id).fetch_one(pool).await?;
    Ok(c)
}

/// Per-chain row counts in a single query. Replaces the N+1 loop in
/// `/api/v1/health`. Returns `(chain_id, count)` for every chain that has at
/// least one row; chains with zero rows are absent and the caller fills 0.
pub async fn count_by_chain(pool: &PgPool) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT chain_id, COUNT(*)::BIGINT FROM agents GROUP BY chain_id")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}
