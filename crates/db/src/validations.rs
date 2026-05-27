//! Typed read queries + W5 worker upserts against the `validations` table.
//!
//! Phase-5 surface: lookup-by-request-hash (powers MCP `read_validation`).
//! Phase-4 W5 surface: request insert + response update. The table PK is
//! `(chain_id, request_hash)` — we keep only the latest-status row per
//! request per plan §3 W5 ("latest status only").

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};

/// One row of the `validations` table. `response` is the validator's
/// 0–100 score (nullable until the validator responds).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ValidationRow {
    pub chain_id: i64,
    pub request_hash: Vec<u8>,
    pub validator_addr: Vec<u8>,
    pub agent_id: BigDecimal,
    pub request_uri: String,
    pub response: Option<i16>,
    pub response_uri: Option<String>,
    pub response_hash: Option<Vec<u8>>,
    pub tag: Option<String>,
    pub last_update: DateTime<Utc>,
}

/// Look up a single validation by its `(chain_id, request_hash)` primary key.
/// The request hash is the keccak256 of the off-chain request payload; clients
/// pass it as `0x…` hex.
pub async fn get_one(
    pool: &PgPool,
    chain_id: i64,
    request_hash: &[u8],
) -> Result<Option<ValidationRow>, sqlx::Error> {
    sqlx::query_as::<_, ValidationRow>(
        "SELECT chain_id, request_hash, validator_addr, agent_id,
                request_uri, response, response_uri, response_hash,
                tag, last_update
         FROM validations
         WHERE chain_id = $1 AND request_hash = $2",
    )
    .bind(chain_id)
    .bind(request_hash)
    .fetch_optional(pool)
    .await
}

/// Upsert a `ValidationRequest` event. On conflict (same chain+hash):
/// refresh the request fields but preserve any `response*` columns already
/// set by a previously processed ValidationResponse — out-of-order events
/// can ship the Response page before the Request page if the worker first
/// scrape window straddles them in odd ways. Idempotent on re-scan.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_request(
    executor: &mut PgConnection,
    chain_id: i64,
    request_hash: &[u8],
    validator_addr: &[u8],
    agent_id: &BigDecimal,
    request_uri: &str,
    last_update: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO validations
            (chain_id, request_hash, validator_addr, agent_id,
             request_uri, response, response_uri, response_hash, tag,
             last_update)
         VALUES ($1, $2, $3, $4, $5, NULL, NULL, NULL, NULL, $6)
         ON CONFLICT (chain_id, request_hash) DO UPDATE
           SET validator_addr = EXCLUDED.validator_addr,
               agent_id       = EXCLUDED.agent_id,
               request_uri    = EXCLUDED.request_uri,
               last_update    = EXCLUDED.last_update",
    )
    .bind(chain_id)
    .bind(request_hash)
    .bind(validator_addr)
    .bind(agent_id)
    .bind(request_uri)
    .bind(last_update)
    .execute(executor)
    .await
    .map(|_| ())
}

/// Apply a `ValidationResponse` to the matching row (latest-status-only
/// per plan §3 W5). If the row doesn't yet exist (Response observed before
/// Request) we insert a stub so the latest known state isn't lost; the
/// Request scan on the next tick refreshes the missing fields.
///
/// Returns `Ok(true)` if a row was created or updated.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_response(
    executor: &mut PgConnection,
    chain_id: i64,
    request_hash: &[u8],
    validator_addr: &[u8],
    agent_id: &BigDecimal,
    response: i16,
    response_uri: Option<&str>,
    response_hash: Option<&[u8]>,
    tag: Option<&str>,
    last_update: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    // Single INSERT…ON CONFLICT DO UPDATE — guarantees latest-status-only
    // even if Response shows up before Request.
    let result = sqlx::query(
        "INSERT INTO validations
            (chain_id, request_hash, validator_addr, agent_id,
             request_uri, response, response_uri, response_hash, tag,
             last_update)
         VALUES ($1, $2, $3, $4, '', $5, $6, $7, $8, $9)
         ON CONFLICT (chain_id, request_hash) DO UPDATE
           SET response       = EXCLUDED.response,
               response_uri   = EXCLUDED.response_uri,
               response_hash  = EXCLUDED.response_hash,
               tag            = EXCLUDED.tag,
               last_update    = EXCLUDED.last_update",
    )
    .bind(chain_id)
    .bind(request_hash)
    .bind(validator_addr)
    .bind(agent_id)
    .bind(response)
    .bind(response_uri)
    .bind(response_hash)
    .bind(tag)
    .bind(last_update)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() >= 1)
}
