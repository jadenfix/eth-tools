//! Typed reads against the `validations` table.
//!
//! The API exposes `GET /api/v1/validation/:chain/:hash` (single row).
//! Worker-side ingest from ValidationRegistry events lives in `crates/workers`.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

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

pub async fn get_one(
    pool: &PgPool,
    chain_id: i64,
    request_hash: &[u8],
) -> Result<Option<ValidationRow>, sqlx::Error> {
    sqlx::query_as::<_, ValidationRow>(
        "SELECT chain_id, request_hash, validator_addr, agent_id, request_uri,
                response, response_uri, response_hash, tag, last_update
         FROM validations
         WHERE chain_id = $1 AND request_hash = $2",
    )
    .bind(chain_id)
    .bind(request_hash)
    .fetch_optional(pool)
    .await
}
