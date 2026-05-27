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

/// MCP-facing filtered search. ILIKE on `agent_uri` is a deliberate "good
/// enough for v1" choice — Postgres trigram or full-text lands when an MCP
/// user files a relevance bug. Owner is matched as a raw byte array
/// (already lower-cased hex bytes in the table).
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub query: Option<String>,
    pub chain_id: Option<i64>,
    pub owner: Option<Vec<u8>>,
    pub has_manifest: Option<bool>,
    pub has_endpoint: Option<bool>,
    pub limit: i64,
}

pub async fn search(pool: &PgPool, p: SearchParams) -> Result<Vec<AgentRow>, sqlx::Error> {
    let limit = p.limit.clamp(1, 200);
    // Build a single parameterized statement; NULL placeholders skip filters.
    // `has_endpoint` is a Phase-6 column on `agent_endpoints` so we proxy it
    // through `agent_wallet IS NOT NULL` for now (TODO: replace once the
    // endpoint_prober worker populates the real table in Phase-7).
    let sql = "
        SELECT chain_id, agent_id, owner, agent_uri, agent_wallet,
               registered_at, updated_at
        FROM agents
        WHERE ($1::TEXT IS NULL OR agent_uri ILIKE '%' || $1 || '%')
          AND ($2::BIGINT IS NULL OR chain_id = $2)
          AND ($3::BYTEA IS NULL OR owner = $3)
          AND ($4::BOOL IS NULL
               OR ($4 = TRUE  AND agent_uri IS NOT NULL)
               OR ($4 = FALSE AND agent_uri IS NULL))
          AND ($5::BOOL IS NULL
               OR ($5 = TRUE  AND agent_wallet IS NOT NULL)
               OR ($5 = FALSE AND agent_wallet IS NULL))
        ORDER BY updated_at DESC, chain_id ASC, agent_id ASC
        LIMIT $6
    ";
    sqlx::query_as::<_, AgentRow>(sql)
        .bind(p.query.as_deref())
        .bind(p.chain_id)
        .bind(p.owner)
        .bind(p.has_manifest)
        .bind(p.has_endpoint)
        .bind(limit)
        .fetch_all(pool)
        .await
}
