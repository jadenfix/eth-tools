//! Typed queries against the `agents` table.
//!
//! Reads only (Phase 1). Worker-side upserts land with `crates/workers` in
//! Phase 4.
//!
//! Pagination is keyset on `(updated_at DESC, chain_id, agent_id)` so result
//! pages are stable under concurrent worker writes — OFFSET pagination would
//! drop or duplicate rows when the scraper bumps `updated_at`.

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

/// Stable keyset pointer. Compared lexicographically against
/// `(updated_at DESC, chain_id, agent_id)` so a row with the same
/// `updated_at` as the cursor but a higher (chain_id, agent_id) still
/// appears on the next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysetCursor {
    pub updated_at: DateTime<Utc>,
    pub chain_id: i64,
    pub agent_id: BigDecimal,
}

pub async fn list(pool: &PgPool, params: ListParams) -> Result<Vec<AgentRow>, sqlx::Error> {
    let limit = params.limit.clamp(1, 200);

    // Build the keyset filter conditionally; bind values with `bind` so the
    // SQL stays a single prepared statement regardless of params present.
    // Postgres compares the row tuple (a, b, c) < (x, y, z) lexicographically.
    let base_sql = "
        SELECT chain_id, agent_id, owner, agent_uri, agent_wallet,
               registered_at, updated_at
        FROM agents
        WHERE ($1::BIGINT IS NULL OR chain_id = $1)
          AND (
                $2::TIMESTAMPTZ IS NULL
                OR (updated_at, chain_id, agent_id) < ($2, $3, $4)
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
