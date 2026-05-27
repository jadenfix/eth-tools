//! Typed queries against the `manifests` table + the
//! `agents.last_manifest_fetch` staleness column.
//!
//! Used by the W2 `manifest_fetcher` cron (every 15 minutes). Reads are
//! also surfaced by the MCP `inspect_agent` and `validate_manifest` tools
//! through the `latest_for_agent` helper.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// One row from the `agents` table reduced to what W2 needs to fan-out a
/// fetch. The full agents read-path lives in `crates/db/src/agents.rs`;
/// this is a worker-targeted projection (cheap select, no joins).
#[derive(Debug, Clone, FromRow)]
pub struct StaleAgent {
    pub chain_id: i64,
    pub agent_id: BigDecimal,
    pub agent_uri: String,
}

/// Validation outcome stored on each `manifests` row. Maps to the CHECK
/// constraint on `manifests.validation_status` in `0001_init.up.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    Valid,
    Invalid,
    /// Network failure, SSRF rejection, oversize body, bad content-type,
    /// etc. The row is still written so the dashboard can surface "we
    /// tried this URI and it didn't work", with `raw_bytes_sha256 =
    /// sha256("")` as a deterministic PK sentinel (see `upsert_manifest`).
    Unreachable,
}

impl ValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Unreachable => "unreachable",
        }
    }
}

/// Returns up to `limit` agents whose manifest is stale: either never
/// fetched (`last_manifest_fetch IS NULL`) or last fetched more than
/// `stale_after_hours` hours ago. Filters out rows where `agent_uri` is
/// NULL (nothing to fetch).
///
/// Ordering: `last_manifest_fetch NULLS FIRST` so the never-fetched agents
/// drain first; covered by `idx_agents_manifest_due`.
pub async fn list_stale_agents(
    pool: &PgPool,
    stale_after_hours: i32,
    limit: i32,
) -> sqlx::Result<Vec<StaleAgent>> {
    let limit = limit.clamp(1, 500);
    let stale_after_hours = stale_after_hours.max(1);
    sqlx::query_as::<_, StaleAgent>(
        // The interval is composed inside the query so the column is
        // index-scanned cleanly. `make_interval` keeps this parameterized
        // (vs. string-concat'ing an interval literal which sqlx wouldn't
        // bind safely).
        "SELECT chain_id, agent_id, agent_uri
           FROM agents
          WHERE agent_uri IS NOT NULL
            AND (
                  last_manifest_fetch IS NULL
                  OR last_manifest_fetch < NOW() - make_interval(hours => $1::INT)
                )
          ORDER BY last_manifest_fetch NULLS FIRST, chain_id, agent_id
          LIMIT $2",
    )
    .bind(stale_after_hours)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
}

/// UPSERT a manifest row keyed on (chain_id, agent_id, raw_bytes_sha256).
/// For `unreachable` rows we use `sha256("")` (`e3b0c4…`) as the PK
/// sentinel so re-attempts for the same agent collide and we update
/// `fetched_at` in place — otherwise a flaky URI would pollute the table
/// with one row per attempt.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_manifest(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
    source_uri: &str,
    raw_bytes_sha256: &[u8],
    raw_keccak256: &[u8],
    parsed: Option<&serde_json::Value>,
    validation_status: ValidationStatus,
    validation_errors: Option<&serde_json::Value>,
    blob_url: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO manifests (chain_id, agent_id, fetched_at, source_uri,
                                raw_bytes_sha256, raw_keccak256, parsed,
                                validation_status, validation_errors, blob_url)
         VALUES ($1, $2, NOW(), $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (chain_id, agent_id, raw_bytes_sha256) DO UPDATE SET
              fetched_at        = EXCLUDED.fetched_at,
              source_uri        = EXCLUDED.source_uri,
              raw_keccak256     = EXCLUDED.raw_keccak256,
              parsed            = EXCLUDED.parsed,
              validation_status = EXCLUDED.validation_status,
              validation_errors = EXCLUDED.validation_errors,
              blob_url          = EXCLUDED.blob_url",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(source_uri)
    .bind(raw_bytes_sha256)
    .bind(raw_keccak256)
    .bind(parsed)
    .bind(validation_status.as_str())
    .bind(validation_errors)
    .bind(blob_url)
    .execute(pool)
    .await?;
    Ok(())
}

/// Bump `agents.last_manifest_fetch = NOW()`. Called for EVERY agent W2
/// attempts (valid / invalid / unreachable) — otherwise an unreachable
/// URI would be retried every 15 minutes for eternity, defeating the
/// staleness throttle.
pub async fn touch_last_fetch(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agents SET last_manifest_fetch = NOW()
          WHERE chain_id = $1 AND agent_id = $2",
    )
    .bind(chain_id)
    .bind(agent_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// One row reduced to the columns the MCP `inspect_agent` tool surfaces.
/// Kept here (next to the writer) so the column set stays in sync.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ManifestRow {
    pub chain_id: i64,
    pub agent_id: BigDecimal,
    pub fetched_at: DateTime<Utc>,
    pub source_uri: String,
    pub raw_bytes_sha256: Vec<u8>,
    pub raw_keccak256: Vec<u8>,
    pub validation_status: String,
    pub validation_errors: Option<serde_json::Value>,
    pub blob_url: Option<String>,
}

pub async fn latest_for_agent(
    pool: &PgPool,
    chain_id: i64,
    agent_id: &BigDecimal,
) -> sqlx::Result<Option<ManifestRow>> {
    sqlx::query_as::<_, ManifestRow>(
        "SELECT chain_id, agent_id, fetched_at, source_uri,
                raw_bytes_sha256, raw_keccak256,
                validation_status, validation_errors, blob_url
           FROM manifests
          WHERE chain_id = $1 AND agent_id = $2
          ORDER BY fetched_at DESC
          LIMIT 1",
    )
    .bind(chain_id)
    .bind(agent_id)
    .fetch_optional(pool)
    .await
}
